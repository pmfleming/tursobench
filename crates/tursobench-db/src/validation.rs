//! Foreign-key diagnostics implemented against supported native metadata.
use crate::{connection::failure, inspection};
use turso::Connection;
use tursobench_core::{Failure, quote_identifier};

pub(crate) async fn foreign_keys(c: &Connection) -> Result<String, Failure> {
    let mut names = Vec::new();
    let mut bytes = 0;
    let mut rows = c.query("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name", ()).await.map_err(failure)?;
    while let Some(row) = rows.next().await.map_err(failure)? {
        let turso::Value::Text(name) = row.get_value(0).map_err(failure)? else {
            return Err(Failure::input(
                "Invalid table metadata; validation incomplete",
            ));
        };
        bytes += name.len();
        if names.len() >= 1000 || bytes > 1024 * 1024 {
            return Err(Failure::input(
                "Schema exceeds validation budget; validation incomplete",
            ));
        }
        names.push(name);
    }
    drop(rows);
    let mut checks = Vec::new();
    for name in &names {
        let info = inspection::inspect(c, name).await?;
        for key in &info.foreign_keys {
            let parent = crate::browse::describe(c, &key.target).await?;
            let mut present = Vec::new();
            let mut matches = Vec::new();
            for (source, target) in &key.columns {
                let target = target
                    .as_ref()
                    .filter(|target| {
                        parent
                            .columns
                            .iter()
                            .any(|c| c.name.eq_ignore_ascii_case(target))
                    })
                    .ok_or_else(|| {
                        Failure::input("Unresolved parent key; validation incomplete")
                    })?;
                present.push(format!("c.{} IS NOT NULL", quote_identifier(source)));
                // Parent affinity and collation govern FK comparisons.
                matches.push(format!(
                    "p.{} = c.{}",
                    quote_identifier(target),
                    quote_identifier(source)
                ));
            }
            if present.is_empty() {
                return Err(Failure::input("Empty foreign key; validation incomplete"));
            }
            let literal = |s: &str| format!("'{}'", s.replace('\'', "''"));
            checks.push(format!("SELECT {} AS child_table, {} AS parent_table, {} AS foreign_key_id, count(*) AS violations, {} AS checked_tables FROM {} AS c WHERE {} AND NOT EXISTS (SELECT 1 FROM {} AS p WHERE {})",
                literal(name), literal(&key.target), key.id, names.len(), quote_identifier(name), present.join(" AND "), quote_identifier(&key.target), matches.join(" AND ")));
            if checks.len() > 256 || checks.iter().map(String::len).sum::<usize>() > 1024 * 1024 {
                return Err(Failure::input(
                    "Foreign keys exceed validation budget; validation incomplete",
                ));
            }
        }
    }
    if checks.is_empty() {
        Ok("SELECT NULL AS child_table, NULL AS parent_table, NULL AS foreign_key_id, 0 AS violations, 0 AS checked_tables WHERE 0".into())
    } else {
        Ok(checks.join(" UNION ALL "))
    }
}
