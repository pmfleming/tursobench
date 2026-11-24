//! Bounded committed-schema discovery: per-table inspection, the schema diagram
//! and foreign-key diagnostics. No table scans and no inferred relationships.
use crate::{
    Diagnostic,
    browse::describe,
    connection::{failure, from_engine, text_column},
};
use turso::Connection;
use tursobench_core::{
    DiagramTable, Failure, ForeignKey, IndexInfo, Inspection, SchemaDiagram, Table, TableColumn,
    Value, quote_identifier,
};

async fn metadata(c: &Connection, sql: String) -> Result<Vec<Vec<Value>>, Failure> {
    let mut rows = c.query(sql, ()).await.map_err(failure)?;
    let mut result = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next().await.map_err(failure)? {
        let values = (0..rows.column_count())
            .map(|i| row.get_value(i).map(from_engine).map_err(failure))
            .collect::<Result<Vec<_>, _>>()?;
        bytes += values.iter().map(Value::bytes).sum::<usize>();
        if result.len() >= 512 || bytes > 1024 * 1024 {
            return Err(Failure::input("Schema metadata budget exceeded"));
        }
        result.push(values);
    }
    Ok(result)
}
fn text(row: &[Value], i: usize) -> Result<String, Failure> {
    match row.get(i) {
        Some(Value::Text(s)) => Ok(s.clone()),
        _ => Err(Failure::input("Unexpected schema metadata text")),
    }
}
fn integer(row: &[Value], i: usize) -> Result<i64, Failure> {
    match row.get(i) {
        Some(Value::Integer(n)) => Ok(*n),
        _ => Err(Failure::input("Unexpected schema metadata integer")),
    }
}
async fn indexes(c: &Connection, name: &str) -> Result<Vec<IndexInfo>, Failure> {
    let mut indexes = Vec::new();
    let mut bytes = 0;
    for row in metadata(c, format!("PRAGMA index_list({})", quote_identifier(name))).await? {
        if indexes.len() >= 128 {
            return Err(Failure::input("Index count exceeds metadata budget"));
        }
        let index_name = text(&row, 1)?;
        let entries = metadata(
            c,
            format!("PRAGMA index_info({})", quote_identifier(&index_name)),
        )
        .await?;
        bytes += index_name.len() + entries.iter().flatten().map(Value::bytes).sum::<usize>();
        if bytes > 1024 * 1024 {
            return Err(Failure::input("Combined index metadata exceeds 1 MiB"));
        }
        indexes.push(IndexInfo {
            name: index_name,
            unique: integer(&row, 2)? != 0,
            partial: integer(&row, 4)? != 0,
            columns: entries
                .iter()
                .map(|r| match r.get(2) {
                    Some(Value::Text(s)) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
        });
    }
    Ok(indexes)
}
/// Names an implicit parent primary key, unless empty identifiers make the
/// native metadata ambiguous.
async fn resolve_implicit(
    c: &Connection,
    parent_name: &str,
    columns: &mut [(String, Option<String>)],
) {
    if !columns.iter().any(|(_, target)| target.is_none()) {
        return;
    }
    let Ok(parent) = describe(c, parent_name).await else {
        return;
    };
    let keys = parent.keys();
    if keys.len() != columns.len() || parent.columns.iter().any(|c| c.name.is_empty()) {
        return;
    }
    for (position, (_, target)) in columns.iter_mut().enumerate() {
        if target.is_none() {
            *target = Some(parent.columns[keys[position]].name.clone());
        }
    }
}
async fn foreign_keys(c: &Connection, name: &str) -> Result<Vec<ForeignKey>, Failure> {
    type Parts = std::collections::BTreeMap<i64, (String, Option<String>)>;
    let mut keys = std::collections::BTreeMap::<i64, (ForeignKey, Parts)>::new();
    let sql = format!("PRAGMA foreign_key_list({})", quote_identifier(name));
    for row in metadata(c, sql).await? {
        let id = integer(&row, 0)?;
        let sequence = integer(&row, 1)?;
        let key = ForeignKey {
            id,
            target: text(&row, 2)?,
            columns: vec![],
            on_update: text(&row, 5)?,
            on_delete: text(&row, 6)?,
        };
        let (_, parts) = keys.entry(id).or_insert((key, Parts::new()));
        // Turso 0.8.3 reports an empty string, not NULL, for an implicit parent PK.
        let target = match row.get(4) {
            Some(Value::Text(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        parts.insert(sequence, (text(&row, 3)?, target));
    }
    let mut foreign_keys = Vec::new();
    let mut bytes = 0;
    for (mut key, parts) in keys.into_values() {
        key.columns = parts.into_values().collect();
        resolve_implicit(c, &key.target, &mut key.columns).await;
        bytes += key.bytes();
        if bytes > 1024 * 1024 {
            return Err(Failure::input(
                "Resolved foreign-key metadata exceeds 1 MiB",
            ));
        }
        foreign_keys.push(key);
    }
    Ok(foreign_keys)
}
pub(crate) async fn inspect(c: &Connection, name: &str) -> Result<Inspection, Failure> {
    Ok(Inspection {
        table: describe(c, name).await?,
        indexes: indexes(c, name).await?,
        foreign_keys: foreign_keys(c, name).await?,
    })
}
pub(crate) fn lookup_sql(
    table: &Table,
    keys: Vec<(String, Value)>,
) -> Result<(String, Vec<Value>), Failure> {
    if keys.is_empty()
        || keys.len() > table.columns.len()
        || table.columns.iter().any(|c| c.hidden != 0)
    {
        return Err(Failure::input("Unsupported related-row lookup"));
    }
    let mut predicates = Vec::new();
    let mut values = Vec::new();
    for (name, value) in keys {
        if !table
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            return Err(Failure::input("Referenced column no longer exists"));
        }
        values.push(value);
        predicates.push(format!("{} IS ?{}", quote_identifier(&name), values.len()));
    }
    let names = table
        .columns
        .iter()
        .map(|c| quote_identifier(&c.name))
        .collect::<Vec<_>>()
        .join(", ");
    Ok((
        format!(
            "SELECT {names} FROM {} WHERE {} LIMIT 100",
            quote_identifier(&table.name),
            predicates.join(" AND ")
        ),
        values,
    ))
}

// Compare parent columns on the left so parent affinity/collation govern FK
// equality. EXISTS avoids duplicate children if invalid parent keys are present.
pub(crate) fn references_sql(
    child: &Table,
    parent: &Table,
    key: &ForeignKey,
    values: &[Value],
    count: bool,
) -> Result<String, Failure> {
    let condition = reference_condition(child, parent, key, values)?;
    let columns = child
        .columns
        .iter()
        .map(|c| format!("c.{}", quote_identifier(&c.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let columns = if count {
        "count(*) AS row_count".into()
    } else {
        columns
    };
    let limit = if count { "" } else { " LIMIT 101" };
    Ok(format!(
        "SELECT {columns} FROM {} AS c WHERE {condition}{limit}",
        quote_identifier(&child.name),
    ))
}

pub(crate) fn reference_condition(
    child: &Table,
    parent: &Table,
    key: &ForeignKey,
    values: &[Value],
) -> Result<String, Failure> {
    if key.columns.is_empty()
        || key.columns.len() != values.len()
        || child
            .columns
            .iter()
            .chain(&parent.columns)
            .any(|c| c.hidden != 0)
        || values.contains(&Value::Null)
    {
        return Err(Failure::input("Unsupported or NULL reverse reference"));
    }
    let mut predicates = Vec::new();
    for (i, (source, target)) in key.columns.iter().enumerate() {
        let target = target
            .as_ref()
            .ok_or_else(|| Failure::input("Unresolved parent key"))?;
        let (ct, pt) = child
            .columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(source))
            .zip(
                parent
                    .columns
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(target)),
            )
            .ok_or_else(|| Failure::input("Relationship columns no longer exist"))?;
        if key.columns.len() == 1
            && parent.binary_collations
            && ct.declared_type.eq_ignore_ascii_case(&pt.declared_type)
        {
            return Ok(format!(
                "c.{} IN (SELECT p.{} FROM {} AS p WHERE p.{} IS ?1)",
                quote_identifier(source),
                quote_identifier(target),
                quote_identifier(&parent.name),
                quote_identifier(target)
            ));
        }
        let target = quote_identifier(target);
        predicates.push(format!(
            "p.{target} = c.{} AND p.{target} IS ?{}",
            quote_identifier(source),
            i + 1
        ));
    }

    Ok(format!(
        "EXISTS (SELECT 1 FROM {} AS p WHERE {})",
        quote_identifier(&parent.name),
        predicates.join(" AND ")
    ))
}

/// Table names in name order, cut short (`true`) once `max` names or
/// `max_bytes` of names are reached.
async fn table_names(
    c: &Connection,
    max: usize,
    max_bytes: usize,
) -> Result<(Vec<String>, bool), Failure> {
    let sql = format!(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name LIMIT {}",
        max + 1
    );
    let mut rows = c.query(sql, ()).await.map_err(failure)?;
    let (mut names, mut bytes) = (Vec::new(), 0);
    while let Some(row) = rows.next().await.map_err(failure)? {
        let name = text_column(&row, 0, "schema name")?;
        bytes += name.len();
        if names.len() == max || bytes > max_bytes {
            return Ok((names, true));
        }
        names.push(name);
    }
    Ok((names, false))
}

const DIAGRAM_TABLES: usize = 512;
const DIAGRAM_BYTES: usize = 8 * 1024 * 1024;
fn inspection_bytes(info: &Inspection) -> usize {
    info.table.name.capacity()
        + info.table.columns.capacity() * std::mem::size_of::<TableColumn>()
        + info.indexes.capacity() * std::mem::size_of::<IndexInfo>()
        + info.foreign_keys.capacity() * std::mem::size_of::<ForeignKey>()
        + info
            .table
            .columns
            .iter()
            .map(|c| {
                c.name.capacity()
                    + c.declared_type.capacity()
                    + c.default_sql.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + info
            .indexes
            .iter()
            .map(|i| {
                i.name.capacity()
                    + i.columns.capacity() * std::mem::size_of::<Option<String>>()
                    + i.columns
                        .iter()
                        .flatten()
                        .map(String::capacity)
                        .sum::<usize>()
            })
            .sum::<usize>()
        + info
            .foreign_keys
            .iter()
            .map(|f| {
                f.target.capacity()
                    + f.on_update.capacity()
                    + f.on_delete.capacity()
                    + f.columns.capacity() * std::mem::size_of::<(String, Option<String>)>()
                    + f.columns
                        .iter()
                        .map(|(a, b)| a.capacity() + b.as_ref().map_or(0, String::capacity))
                        .sum::<usize>()
            })
            .sum::<usize>()
}
pub(crate) async fn diagram(c: &Connection) -> Result<SchemaDiagram, Failure> {
    let (mut names, mut limited) = table_names(c, DIAGRAM_TABLES, DIAGRAM_BYTES).await?;
    if let Some(long) = names.iter().position(|name| name.len() > 64 * 1024) {
        names.truncate(long);
        limited = true;
    }
    let mut graph = SchemaDiagram {
        tables: Vec::with_capacity(DIAGRAM_TABLES),
        limited,
    };
    let mut used = std::mem::size_of::<SchemaDiagram>()
        + graph.tables.capacity() * std::mem::size_of::<DiagramTable>();
    for name in names {
        let (inspection, error) = match inspect(c, &name).await {
            Ok(info) => (Some(info), None),
            Err(error) => (
                None,
                Some(error.to_string().chars().take(2048).collect::<String>()),
            ),
        };
        used += name.capacity()
            + inspection.as_ref().map_or(0, inspection_bytes)
            + error.as_ref().map_or(0, String::capacity);
        if used > DIAGRAM_BYTES {
            graph.limited = true;
            break;
        }
        graph.tables.push(DiagramTable {
            name,
            inspection,
            error,
        });
    }
    Ok(graph)
}

impl Diagnostic {
    /// This check's SQL; foreign keys are checked by a query built from supported
    /// native metadata rather than `PRAGMA foreign_key_check`.
    pub(crate) async fn query(self, c: &Connection) -> Result<String, Failure> {
        match self {
            Self::ForeignKeys => foreign_key_check(c).await,
            kind => Ok(kind.sql().into()),
        }
    }
}
/// Counts `child` rows whose non-NULL `key` has no `parent` row.
fn key_check(
    child: &str,
    key: &ForeignKey,
    parent: &Table,
    checked_tables: usize,
) -> Result<String, Failure> {
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
            .ok_or_else(|| Failure::input("Unresolved parent key; validation incomplete"))?;
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
    Ok(format!(
        "SELECT {} AS child_table, {} AS parent_table, {} AS foreign_key_id, count(*) AS violations, {checked_tables} AS checked_tables FROM {} AS c WHERE {} AND NOT EXISTS (SELECT 1 FROM {} AS p WHERE {})",
        literal(child),
        literal(&key.target),
        key.id,
        quote_identifier(child),
        present.join(" AND "),
        quote_identifier(&key.target),
        matches.join(" AND ")
    ))
}
async fn foreign_key_check(c: &Connection) -> Result<String, Failure> {
    let (names, truncated) = table_names(c, 1000, 1024 * 1024).await?;
    if truncated {
        return Err(Failure::input(
            "Schema exceeds validation budget; validation incomplete",
        ));
    }
    let mut checks = Vec::new();
    for name in &names {
        let info = inspect(c, name).await?;
        for key in &info.foreign_keys {
            let parent = describe(c, &key.target).await?;
            checks.push(key_check(name, key, &parent, names.len())?);
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
