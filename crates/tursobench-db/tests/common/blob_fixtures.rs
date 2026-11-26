//! The record viewer's blob samples, synthetic contents only.
use std::path::Path;
use turso::{Builder, params};

/// Values the record viewer recognises, as `kind` and bytes: a subset of the
/// `blobs` table `tests/fixtures/generate.py` writes, with two values large
/// enough to be deferred.
pub fn blob_samples() -> Vec<(&'static str, Vec<u8>)> {
    let mut large_png =
        b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x01\0\0\0\x01\0\x08\x02\0\0\0".to_vec();
    large_png.resize(200_000, 0x5a);
    let mut large_gzip = vec![0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0xff];
    large_gzip.resize(150_000, 0xa5);
    vec![
        ("short bytes", vec![0, 255, 122]),
        ("utf-8 text", "Synthetic note λ".as_bytes().to_vec()),
        (
            "uuid",
            (0..16)
                .map(|i| [0x3f, 0x2a, 0x9c, 0x1e, 0x7b, 0x40, 0x4d, 0x1a, 0x9e, 0x5c][i % 10])
                .collect(),
        ),
        ("large png", large_png),
        ("large gzip", large_gzip),
    ]
}
pub async fn blobs(path: &Path) -> turso::Result<()> {
    assert!(!path.exists(), "fixture creation never overwrites files");
    let db = Builder::new_local(path.to_str().unwrap()).build().await?;
    let c = db.connect()?;
    c.execute(
        "CREATE TABLE blobs(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, data BLOB)",
        (),
    )
    .await?;
    for (kind, data) in blob_samples() {
        c.execute(
            "INSERT INTO blobs(kind, data) VALUES(?1, ?2)",
            params![kind, data],
        )
        .await?;
    }
    Ok(())
}
