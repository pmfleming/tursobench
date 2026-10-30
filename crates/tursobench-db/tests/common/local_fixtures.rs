//! Exact local-project schemas, synthetic contents only. No user files are opened.
use std::path::Path;
use turso::{Builder, Connection, params};

pub const NAMES: [&str; 4] = ["scratchpad", "agenda", "notifications", "clipboard"];
pub const ROWS: usize = 205;

pub async fn create(path: &Path, name: &str) -> turso::Result<()> {
    assert!(!path.exists(), "fixture creation never overwrites files");
    let (schema, application, version) = match name {
        "scratchpad" => (
            include_str!("../../../../tests/fixtures/local-projects/scratchpad.sql"),
            0x5350_4350,
            9,
        ),
        "agenda" => (
            include_str!("../../../../tests/fixtures/local-projects/agenda.sql"),
            0x4147_4e44,
            1,
        ),
        "notifications" => (
            include_str!("../../../../tests/fixtures/local-projects/notifications.sql"),
            0x4e4f_5446,
            1,
        ),
        "clipboard" => (
            include_str!("../../../../tests/fixtures/local-projects/clipboard.sql"),
            0x434c_4950,
            1,
        ),
        _ => panic!("unknown fixture"),
    };
    let db = Builder::new_local(path.to_str().unwrap())
        .with_io(turso::IoBackend::Syscall)
        .experimental_index_method(true)
        .build()
        .await?;
    let c = db.connect()?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;")
        .await?;
    c.execute_batch(schema).await?;
    match name {
        "scratchpad" => scratchpad(&c).await?,
        "agenda" => agenda(&c).await?,
        "notifications" => notifications(&c).await?,
        "clipboard" => clipboard(&c).await?,
        _ => unreachable!(),
    }
    c.execute_batch(format!(
        "PRAGMA application_id={application}; PRAGMA user_version={version}; COMMIT;"
    ))
    .await?;
    // Leave committed WAL in place: clients must open the entire closed fixture,
    // not just copy its main database file. Dropping these handles closes it.
    Ok(())
}
async fn scratchpad(c: &Connection) -> turso::Result<()> {
    c.execute("INSERT INTO session_state VALUES(1,?1)", [b"{}".as_slice()])
        .await?;
    c.execute("INSERT INTO tabs VALUES(0,?1)", [b"{}".as_slice()])
        .await?;
    for i in 0..ROWS {
        c.execute(
            "INSERT INTO buffer_ids VALUES(?1)",
            [format!("buffer-{i:04}")],
        )
        .await?;
    }
    c.execute(
        "INSERT INTO history(temp_id,seq,global_seq,entry) VALUES('buffer-0000',1,1,?1)",
        [b"undo\0\r\n".as_slice()],
    )
    .await?;
    c.execute(
        "INSERT INTO blobs VALUES(?1,?2,?3)",
        params![vec![0_u8; 32], b"text\0\xff\r\n".to_vec(), 8_i64],
    )
    .await?;
    c.execute(
        "INSERT INTO blobs VALUES(?1,?2,?3)",
        params![vec![1_u8; 32], vec![0x42_u8; 1024 * 1024], 1024 * 1024_i64],
    )
    .await?;
    c.execute(
        "INSERT INTO snapshots VALUES('buffer-0000',8,?1)",
        [vec![0_u8; 32]],
    )
    .await?;
    c.execute(
        "INSERT INTO parts VALUES('buffer-0000',0,?1)",
        [vec![0_u8; 32]],
    )
    .await?;
    c.execute(
        "INSERT INTO quarantine VALUES('quarantined',?1,'synthetic reason')",
        [b"{}".as_slice()],
    )
    .await?;
    c.execute(
        "INSERT INTO closed_tabs VALUES(1,'test-token',1700000000,?1)",
        [b"{}".as_slice()],
    )
    .await?;
    c.execute("INSERT INTO buffer_versions(id,temp_id,created_at,bytes,digest) VALUES(1,'buffer-0000',1700000000,8,?1)", [vec![0_u8;32]]).await?;
    c.execute("INSERT INTO version_parts VALUES(1,0,?1)", [vec![0_u8; 32]])
        .await?;
    Ok(())
}
async fn agenda(c: &Connection) -> turso::Result<()> {
    for i in 0..ROWS {
        c.execute(
            "INSERT INTO items VALUES(?1,'local',?2,'task',1,?3)",
            params![
                format!("item-{i:04}"),
                if i % 2 == 0 {
                    Some(format!("source-{i}"))
                } else {
                    None
                },
                b"{\"synthetic\":true}".to_vec()
            ],
        )
        .await?;
    }
    c.execute(
        "INSERT INTO operation_receipts VALUES('request','fingerprint',?1)",
        [b"{}".as_slice()],
    )
    .await?;
    c.execute(
        "INSERT INTO migrations VALUES('fixture','fingerprint',?1)",
        [b"{}".as_slice()],
    )
    .await?;
    Ok(())
}
async fn notifications(c: &Connection) -> turso::Result<()> {
    for i in 0..ROWS {
        c.execute("INSERT INTO notifications(session_id,payload_json,preview_json,app_key,search_text,created_unix_ms,updated_unix_ms,closed_unix_ms,close_reason) VALUES(1,?1,'{}','test-app',?2,1700000000000,1700000000000,?3,?4)", params![format!("{{\"title\":\"Synthetic λ {i}\"}}"), format!("notification {i}"), if i%2==0 { None } else { Some(1700000001000_i64) }, if i%2==0 { None } else { Some(2_i64) }]).await?;
    }
    c.execute(
        "INSERT INTO notification_once VALUES('test-event',1700000000000)",
        (),
    )
    .await?;
    c.execute(
        "INSERT INTO notification_meta VALUES('test-key','test-value')",
        (),
    )
    .await?;
    Ok(())
}
async fn clipboard(c: &Connection) -> turso::Result<()> {
    for i in 0..ROWS {
        let id = format!("entry-{i:04}");
        let body = if i == ROWS - 1 {
            vec![0x42; 1024 * 1024]
        } else {
            format!("clipboard Straße λ {i}\0\r\n").into_bytes()
        };
        let content = if i == 0 {
            "unique needle Straße λ".to_string()
        } else {
            format!("clipboard synthetic {i}")
        };
        c.execute(
            "INSERT INTO entries VALUES(?1,1,0,?2,'text/plain','text/plain',NULL,?3,?4,?5,'Text')",
            params![
                id.clone(),
                i as i64 + 1,
                body.len() as i64,
                vec![i as u8; 32],
                content.clone()
            ],
        )
        .await?;
        c.execute(
            "INSERT INTO payloads VALUES(?1,?2)",
            params![id.clone(), body],
        )
        .await?;
        c.execute(
            "INSERT INTO search_content VALUES(?1,?2,?2,1)",
            params![id, content],
        )
        .await?;
    }
    c.execute(
        "INSERT INTO artifacts VALUES('synthetic.png','entry-0000','image',1700000000,1,1,1000)",
        (),
    )
    .await?;
    c.execute(
        "INSERT INTO entry_artifacts VALUES('entry-0000','synthetic.png')",
        (),
    )
    .await?;
    c.execute(
        "INSERT INTO inline_echoes VALUES('image','entry-0000',1700000000)",
        (),
    )
    .await?;
    Ok(())
}
