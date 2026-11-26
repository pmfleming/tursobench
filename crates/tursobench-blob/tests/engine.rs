//! The hand-written readers agree with the engine on values it generates.
use std::sync::atomic::AtomicBool;
use tursobench_blob::tree::{Limits, jsonb};

/// A small deterministic generator, so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Random JSON text: every scalar kind, escapes, and nesting to `depth`.
fn json(rng: &mut Rng, depth: u32) -> String {
    let choice = if depth == 0 {
        rng.below(6)
    } else {
        rng.below(8)
    };
    match choice {
        0 => "null".into(),
        1 => "true".into(),
        2 => "false".into(),
        3 => format!("{}", rng.next() as i64 >> rng.below(60)),
        4 => {
            let pick = ["0.5", "-1.25e-7", "3.0", "1e300", "-0.0", "123456.789"];
            pick[rng.below(pick.len() as u64) as usize].into()
        }
        5 => {
            let pick = [
                "plain",
                "quote \\\" and slash \\\\",
                "tab\\tnew\\nline",
                "λ Straße 😀",
                "\\u00e9 escaped",
                "",
                "ctrl \\u0001",
            ];
            format!("\"{}\"", pick[rng.below(pick.len() as u64) as usize])
        }
        6 => {
            let items: Vec<String> = (0..rng.below(4)).map(|_| json(rng, depth - 1)).collect();
            format!("[{}]", items.join(","))
        }
        _ => {
            let members: Vec<String> = (0..rng.below(4))
                .map(|i| format!("\"k{i}\":{}", json(rng, depth - 1)))
                .collect();
            format!("{{{}}}", members.join(","))
        }
    }
}

fn engine() -> turso::Connection {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let db = turso::Builder::new_local(":memory:").build().await.unwrap();
            db.connect().unwrap()
        })
}

/// One row of one query, with a text parameter.
fn query(c: &turso::Connection, sql: &str, text: &str) -> Vec<turso::Value> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let mut rows = c.query(sql, [text]).await.unwrap();
            let row = rows.next().await.unwrap().unwrap();
            (0..row.column_count())
                .map(|i| row.get_value(i).unwrap())
                .collect()
        })
}

#[test]
fn jsonb_reads_back_as_the_engines_json() {
    let c = engine();
    let mut rng = Rng(0x5eed_1234_abcd);
    let cancel = AtomicBool::new(false);
    let mut inputs: Vec<String> = (0..400).map(|_| json(&mut rng, 4)).collect();
    // JSON5 input stores INT5, FLOAT5 and TEXT5 elements.
    inputs.extend(
        [
            "{a:0x1F,b:.5,c:'it\\'s',d:+7,e:5.,f:[1,2,],}",
            "[Infinity,-Infinity]",
        ]
        .map(String::from),
    );
    for input in inputs {
        let values = query(&c, "SELECT jsonb(?1), json(jsonb(?1))", &input);
        let (turso::Value::Blob(blob), turso::Value::Text(expected)) = (&values[0], &values[1])
        else {
            panic!("{input}: {values:?}");
        };
        let tree =
            jsonb(blob, &Limits::default(), &cancel).unwrap_or_else(|e| panic!("{input}: {e:?}"));
        assert!(tree.complete);
        assert_eq!(&tree.json(), expected, "{input}");
    }
}

#[test]
fn mutated_jsonb_never_panics_and_valid_trees_stay_bounded() {
    let c = engine();
    let mut rng = Rng(0xfeed_beef);
    let cancel = AtomicBool::new(false);
    let items: Vec<String> = (0..20).map(|_| json(&mut rng, 4)).collect();
    let input = format!("[{}]", items.join(","));
    let values = query(&c, "SELECT jsonb(?1)", &input);
    let turso::Value::Blob(blob) = &values[0] else {
        panic!("{values:?}")
    };
    for _ in 0..2000 {
        let mut bytes = blob.clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes[at] = rng.next() as u8;
        }
        bytes.truncate(bytes.len().saturating_sub(rng.below(3) as usize));
        if let Ok(tree) = jsonb(&bytes, &Limits::default(), &cancel) {
            assert!(tree.nodes.len() <= bytes.len());
        }
    }
}

#[test]
fn vectors_read_back_as_the_engines_vector_extract() {
    use tursobench_blob::vector::{self, Layout};
    let c = engine();
    let mut rng = Rng(0xabcd_0042);
    for dims in [1usize, 3, 8, 9, 17, 64] {
        let numbers: Vec<String> = (0..dims)
            .map(|_| format!("{:.4}", (rng.below(20_000) as f64 - 10_000.0) / 1_000.0))
            .collect();
        let text = format!("[{}]", numbers.join(","));
        for (function, expected) in [
            ("vector32", Layout::F32),
            ("vector64", Layout::F64),
            ("vector1bit", Layout::OneBit),
            ("vector8", Layout::EightBit),
            ("vector32_sparse", Layout::Sparse),
        ] {
            let sql = format!("SELECT {function}(?1), vector_extract({function}(?1))");
            let values = query(&c, &sql, &text);
            let (turso::Value::Blob(blob), turso::Value::Text(extracted)) =
                (&values[0], &values[1])
            else {
                panic!("{function}: {values:?}");
            };
            let read =
                vector::read(blob, true).unwrap_or_else(|e| panic!("{function} {text}: {e}"));
            // vector32 stores plain float32, which only the column's hint names.
            let tail = &blob[blob.len().saturating_sub(8)..];
            let named = vector::layout(tail, blob.len() as u64).map(|(layout, _)| layout);
            if function != "vector32" {
                assert_eq!(named, Some(expected), "{function} {text}");
            }
            assert_eq!(
                (read.layout, read.dims),
                (expected, dims),
                "{function} {text}"
            );
            assert_eq!(&read.text(), extracted, "{function} {text}");
        }
    }
}

#[test]
fn random_bytes_never_panic_the_vector_reader() {
    let mut rng = Rng(0x7777);
    for _ in 0..5000 {
        let len = rng.below(80) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if let Some(last) = bytes.last_mut() {
            *last = [1, 2, 3, 4, 9][rng.below(5) as usize];
        }
        let _ = tursobench_blob::vector::read(&bytes, rng.below(2) == 0);
    }
}
