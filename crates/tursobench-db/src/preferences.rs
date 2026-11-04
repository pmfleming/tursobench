//! Application-owned preferences, independent of inspected database sessions.
use std::{path::Path, time::Duration};
use turso::{Connection, Database, Value};

const APPLICATION_ID: i64 = 0x54554250;
const VERSION: i64 = 1;
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

/// Own this on a storage worker: all methods perform synchronous engine work.
pub struct Store {
    connection: Connection,
    _database: Database,
    runtime: tokio::runtime::Runtime,
    expected: Option<String>,
    disabled: bool,
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn validate(payload: &str) -> Result<(), String> {
    if payload.len() > MAX_BYTES {
        return Err("Preferences exceed 2 MiB".into());
    }
    let value: serde_json::Value = serde_json::from_str(payload).map_err(error)?;
    if !value.is_object() {
        return Err("Preferences must be an object".into());
    }
    Ok(())
}
async fn scalar(c: &Connection, sql: &str) -> Result<i64, String> {
    let mut rows = c.query(sql, ()).await.map_err(error)?;
    match rows
        .next()
        .await
        .map_err(error)?
        .ok_or("Missing preferences metadata")?
        .get_value(0)
        .map_err(error)?
    {
        Value::Integer(n) => Ok(n),
        _ => Err("Invalid preferences metadata".into()),
    }
}
async fn identify(c: &Connection) -> Result<(), String> {
    if scalar(c, "PRAGMA application_id").await? != APPLICATION_ID
        || scalar(c, "PRAGMA user_version").await? != VERSION
    {
        return Err("Unsupported preferences database format; file preserved".into());
    }
    Ok(())
}
async fn read(c: &Connection) -> Result<Option<String>, String> {
    identify(c).await?;
    let mut rows = c.query("SELECT format_version, typeof(payload), length(CAST(payload AS BLOB)) FROM app_preferences WHERE id=1", ()).await.map_err(error)?;
    let Some(row) = rows.next().await.map_err(error)? else {
        return Ok(None);
    };
    if row.get_value(0).map_err(error)? != Value::Integer(VERSION)
        || row.get_value(1).map_err(error)? != Value::Text("text".into())
    {
        return Err("Unsupported preferences snapshot; data preserved".into());
    }
    match row.get_value(2).map_err(error)? {
        Value::Integer(n) if (0..=MAX_BYTES as i64).contains(&n) => {}
        _ => return Err("Preferences exceed 2 MiB".into()),
    }
    drop(rows);
    let mut rows = c
        .query("SELECT payload FROM app_preferences WHERE id=1", ())
        .await
        .map_err(error)?;
    match rows
        .next()
        .await
        .map_err(error)?
        .ok_or("Preferences changed during read")?
        .get_value(0)
        .map_err(error)?
    {
        Value::Text(payload) => {
            validate(&payload)?;
            Ok(Some(payload))
        }
        _ => Err("Invalid preferences payload".into()),
    }
}
impl Store {
    /// Inspect a snapshot without acquiring a writer or creating a file.
    pub fn read_file(path: &Path) -> Result<Option<String>, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(error)?;
        runtime.block_on(async {
            let db = turso::Builder::new_local(path.to_str().ok_or("Non-UTF8 preferences path")?)
                .read_only(true)
                .build()
                .await
                .map_err(error)?;
            let c = db.connect().map_err(error)?;
            c.query_timeout(Duration::from_secs(5)).map_err(error)?;
            read(&c).await
        })
    }
    pub fn open(path: &Path) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(error)?;
        let (database, connection, expected) = runtime.block_on(async {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            std::fs::create_dir_all(parent).map_err(error)?;
            let text = path.to_str().ok_or("Non-UTF8 preferences paths are unsupported")?;
            let created = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(file) => {
                    #[cfg(unix)] {
                        use std::os::unix::fs::PermissionsExt;
                        file.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(error)?;
                    }
                    true
                },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(e) => return Err(error(e)),
            };
            // Reject unrelated/newer/corrupt stores before any writable open.
            if !created {
                if !std::fs::metadata(path).map_err(error)?.is_file() || std::fs::metadata(path).map_err(error)?.len() == 0 {
                    return Err("Invalid preferences database; file preserved".into());
                }
                let db = turso::Builder::new_local(text).read_only(true).build().await.map_err(error)?;
                let c = db.connect().map_err(error)?;
                c.query_timeout(Duration::from_secs(5)).map_err(error)?;
                read(&c).await?;
            }
            let db = turso::Builder::new_local(text).build().await.map_err(error)?;
            let c = db.connect().map_err(error)?;
            c.busy_timeout(Duration::from_millis(250)).map_err(error)?;
            c.query_timeout(Duration::from_secs(5)).map_err(error)?;
            if created {
                c.execute("BEGIN IMMEDIATE", ()).await.map_err(error)?;
                c.execute("CREATE TABLE app_preferences(id INTEGER PRIMARY KEY CHECK(id=1),format_version INTEGER NOT NULL,payload TEXT NOT NULL,previous TEXT)", ()).await.map_err(error)?;
                c.execute(&format!("PRAGMA application_id={APPLICATION_ID}"), ()).await.map_err(error)?;
                c.execute(&format!("PRAGMA user_version={VERSION}"), ()).await.map_err(error)?;
                c.execute("COMMIT", ()).await.map_err(error)?;
            }
            let expected = read(&c).await?;
            Ok::<_, String>((db, c, expected))
        })?;
        Ok(Self {
            connection,
            _database: database,
            runtime,
            expected,
            disabled: false,
        })
    }
    pub fn payload(&self) -> Option<&str> {
        self.expected.as_deref()
    }
    pub fn save(&mut self, payload: &str) -> Result<(), String> {
        if self.disabled {
            return Err(
                "Preference saving disabled after an earlier failure; reopen and inspect the store"
                    .into(),
            );
        }
        validate(payload)?;
        let c = &self.connection;
        let result = self.runtime.block_on(async {
            c.execute("BEGIN IMMEDIATE", ()).await.map_err(error)?;
            let update = async {
                if read(c).await? != self.expected { return Err("Preferences changed outside this storage worker; current and previous snapshots preserved".into()); }
                c.execute("INSERT OR REPLACE INTO app_preferences(id,format_version,payload,previous) VALUES(1,1,?1,?2)",
                    (payload, self.expected.as_deref())).await.map_err(error)?;
                c.execute("COMMIT", ()).await.map_err(|e| format!("Preference commit outcome unknown: {e}"))?;
                Ok::<_, String>(())
            }.await;
            if let Err(e) = update {
                if !c.is_autocommit().map_err(error)? {
                    c.execute("ROLLBACK", ()).await.map_err(|cleanup| format!("{e}; rollback failed: {cleanup}"))?;
                }
                return Err(e);
            }
            Ok(())
        });
        if result.is_ok() {
            self.expected = Some(payload.into());
        } else {
            self.disabled = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commits_reopen_and_preserve_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.db");
        let mut s = Store::open(&path).unwrap();
        assert!(s.payload().is_none());
        s.save(r#"{"font_size":14}"#).unwrap();
        s.save(r#"{"font_size":18}"#).unwrap();
        let previous = s.runtime.block_on(async {
            let mut rows = s
                .connection
                .query("SELECT previous FROM app_preferences", ())
                .await
                .unwrap();
            rows.next().await.unwrap().unwrap().get_value(0).unwrap()
        });
        assert_eq!(previous, Value::Text(r#"{"font_size":14}"#.into()));
        assert!(
            s.save(&format!("{{\"huge\":\"{}\"}}", "x".repeat(MAX_BYTES)))
                .is_err()
        );
        drop(s);
        assert_eq!(
            Store::open(&path).unwrap().payload(),
            Some(r#"{"font_size":18}"#)
        );
    }
    #[test]
    fn runtime_corruption_and_external_changes_cannot_be_overwritten() {
        for payload in [r#"{"other":1}"#, "{broken"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("prefs.db");
            let mut s = Store::open(&path).unwrap();
            s.save("{}").unwrap();
            s.save(r#"{"font_size":18}"#).unwrap();
            s.runtime
                .block_on(
                    s.connection
                        .execute("UPDATE app_preferences SET payload=?1", [payload]),
                )
                .unwrap();
            assert!(s.save("{}").is_err());
            assert!(s.disabled);
            s.runtime.block_on(async {
                let mut rows = s
                    .connection
                    .query("SELECT payload,previous FROM app_preferences", ())
                    .await
                    .unwrap();
                let row = rows.next().await.unwrap().unwrap();
                assert_eq!(row.get_value(0).unwrap(), Value::Text(payload.into()));
                assert_eq!(row.get_value(1).unwrap(), Value::Text("{}".into()));
            });
        }
    }
    #[test]
    fn unsupported_versions_and_unrelated_files_are_preserved() {
        for version in [0, 2] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("prefs.db");
            let s = Store::open(&path).unwrap();
            s.runtime
                .block_on(
                    s.connection
                        .execute(&format!("PRAGMA user_version={version}"), ()),
                )
                .unwrap();
            drop(s);
            let before = std::fs::read(&path).unwrap();
            assert!(Store::open(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.db");
        std::fs::write(&path, "corrupt database").unwrap();
        assert!(Store::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt database");
    }
}
