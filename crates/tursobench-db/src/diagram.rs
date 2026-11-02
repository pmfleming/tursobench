//! Bounded committed-schema discovery; no table scans and no inferred relationships.
use crate::{connection::failure, inspection};
use turso::Connection;
use tursobench_core::{
    DiagramTable, Failure, ForeignKey, IndexInfo, Inspection, SchemaDiagram, TableColumn,
};

const MAX_TABLES: usize = 512;
const MAX_BYTES: usize = 8 * 1024 * 1024;
fn bytes(info: &Inspection) -> usize {
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
pub(crate) async fn load(c: &Connection) -> Result<SchemaDiagram, Failure> {
    let mut graph = SchemaDiagram {
        tables: Vec::with_capacity(MAX_TABLES),
        limited: false,
    };
    let mut names = Vec::new();
    let mut used = 0;
    let mut rows = c.query("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name LIMIT 513", ()).await.map_err(failure)?;
    while let Some(row) = rows.next().await.map_err(failure)? {
        let turso::Value::Text(name) = row.get_value(0).map_err(failure)? else {
            return Err(Failure::input("Unexpected schema name"));
        };
        used += name.len();
        if names.len() == MAX_TABLES || used > MAX_BYTES || name.len() > 64 * 1024 {
            graph.limited = true;
            break;
        }
        names.push(name);
    }
    drop(rows);
    used = std::mem::size_of::<SchemaDiagram>()
        + graph.tables.capacity() * std::mem::size_of::<DiagramTable>();
    for name in names {
        let (inspection, error) = match inspection::inspect(c, &name).await {
            Ok(info) => (Some(info), None),
            Err(error) => (
                None,
                Some(error.to_string().chars().take(2048).collect::<String>()),
            ),
        };
        used += name.capacity()
            + inspection.as_ref().map_or(0, bytes)
            + error.as_ref().map_or(0, String::capacity);
        if used > MAX_BYTES {
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
