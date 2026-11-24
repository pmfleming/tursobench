//! Read-only schema capture and additive migration drafts for reviewed execution.
use crate::{
    connection::{failure, text_column},
    sql,
};
use std::collections::{BTreeMap, BTreeSet};
use turso::Connection;
use turso_parser::ast::{
    AlterTableBody, Cmd, ColumnDefinition, CreateTableBody, Name, QualifiedName, Stmt,
};
use tursobench_core::{Failure, SchemaObject, SchemaSnapshot, quote_identifier};

async fn version(c: &Connection) -> Result<i64, Failure> {
    let mut rows = c
        .query("PRAGMA main.schema_version", ())
        .await
        .map_err(failure)?;
    match rows
        .next()
        .await
        .map_err(failure)?
        .ok_or_else(|| Failure::input("Missing schema version"))?
        .get_value(0)
        .map_err(failure)?
    {
        turso::Value::Integer(n) => Ok(n),
        _ => Err(Failure::input("Invalid schema version")),
    }
}
pub(crate) async fn capture(c: &Connection) -> Result<SchemaSnapshot, Failure> {
    let before = version(c).await?;
    let mut rows = c.query("SELECT type,name,tbl_name,CASE WHEN length(CAST(sql AS BLOB)) <= 1048576 THEN sql ELSE NULL END,sql IS NOT NULL FROM main.sqlite_schema WHERE name NOT GLOB 'sqlite_*' AND name NOT GLOB '__turso_internal_*' ORDER BY type,name",()).await.map_err(failure)?;
    let mut objects = vec![];
    let mut bytes = 0;
    while let Some(row) = rows.next().await.map_err(failure)? {
        let text = |i| text_column(&row, i, "schema metadata");
        let ddl = match row.get_value(3).map_err(failure)? {
            turso::Value::Text(v) => Some(v),
            turso::Value::Null
                if row.get_value(4).map_err(failure)? == turso::Value::Integer(0) =>
            {
                None
            }
            _ => {
                return Err(Failure::input(
                    "Schema definition exceeds 1 MiB; comparison is incomplete",
                ));
            }
        };
        let object = SchemaObject {
            kind: text(0)?,
            name: text(1)?,
            table: text(2)?,
            ddl,
        };
        bytes += object.kind.len()
            + object.name.len()
            + object.table.len()
            + object.ddl.as_ref().map_or(0, String::len);
        if objects.len() >= 10_000 || bytes > 8 * 1024 * 1024 {
            return Err(Failure::input(
                "Schema snapshot exceeds 10,000 objects or 8 MiB",
            ));
        }
        objects.push(object);
    }
    drop(rows);
    if version(c).await? != before {
        return Err(Failure::input(
            "Schema changed while capturing; refresh snapshots",
        ));
    }
    Ok(SchemaSnapshot {
        version: before,
        objects,
    })
}
fn statement(ddl: &str) -> Option<Stmt> {
    sql::validate_single(ddl).ok()?;
    match sql::first_command(ddl)? {
        Cmd::Stmt(s) => Some(s),
        _ => None,
    }
}
// Guarded SQL names its main-schema target explicitly so a temporary object
// with the same name cannot redirect the migration away from the captured schema.
fn target(statement: &mut Stmt) -> Option<&mut QualifiedName> {
    match statement {
        Stmt::CreateTable {
            temporary: false,
            tbl_name,
            ..
        } => Some(tbl_name),
        Stmt::CreateIndex { idx_name, .. } => Some(idx_name),
        Stmt::CreateView {
            temporary: false,
            view_name,
            ..
        } => Some(view_name),
        Stmt::CreateTrigger {
            temporary: false,
            trigger_name,
            ..
        } => Some(trigger_name),
        Stmt::AlterTable(t) if matches!(t.body, AlterTableBody::AddColumn(_)) => Some(&mut t.name),
        _ => None,
    }
}
pub(crate) fn additive(sql: &str) -> bool {
    statement(sql)
        .as_mut()
        .and_then(target)
        .is_some_and(|name| {
            name.db_name
                .as_ref()
                .is_some_and(|n| n.as_str().eq_ignore_ascii_case("main"))
        })
}
fn draft_ddl(sql: &str) -> Option<String> {
    let mut stmt = statement(sql)?;
    let name = target(&mut stmt)?;
    if name
        .db_name
        .as_ref()
        .is_some_and(|n| !n.as_str().eq_ignore_ascii_case("main"))
    {
        return None;
    }
    name.db_name = Some(Name::from_string("main"));
    Some(format!("{stmt};"))
}
fn table(ddl: &str) -> Option<CreateTableBody> {
    match statement(ddl)? {
        Stmt::CreateTable { body, .. } => Some(body),
        _ => None,
    }
}
fn column_name(c: &ColumnDefinition) -> String {
    c.col_name.to_string()
}
fn additions(desired: &SchemaObject, current: &SchemaObject) -> Option<String> {
    let (
        CreateTableBody::ColumnsAndConstraints {
            columns: new,
            constraints: nc,
            options: no,
        },
        CreateTableBody::ColumnsAndConstraints {
            columns: old,
            constraints: oc,
            options: oo,
        },
    ) = (table(desired.ddl.as_ref()?)?, table(current.ddl.as_ref()?)?)
    else {
        return None;
    };
    if nc != oc || no != oo || new.len() <= old.len() || !new.starts_with(&old) {
        return None;
    }
    // Only unconstrained appended columns are automatic. Defaults, NOT NULL,
    // generated columns and constraint transformations need explicit data review.
    if new[old.len()..].iter().any(|c| !c.constraints.is_empty()) {
        return None;
    }
    Some(
        new[old.len()..]
            .iter()
            .map(|c| {
                format!(
                    "ALTER TABLE main.{} ADD COLUMN {c};",
                    quote_identifier(&desired.name)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Removed,
    Modified,
    Identical,
}
#[derive(Clone, Debug)]
pub struct Difference {
    pub kind: String,
    pub name: String,
    pub status: Status,
    pub details: Vec<String>,
    pub migration: Option<String>,
    pub guidance: Option<String>,
}
fn column_differences(desired: &SchemaObject, current: &SchemaObject) -> Vec<String> {
    let (
        Some(CreateTableBody::ColumnsAndConstraints {
            columns: new,
            constraints: nc,
            options: no,
        }),
        Some(CreateTableBody::ColumnsAndConstraints {
            columns: old,
            constraints: oc,
            options: oo,
        }),
    ) = (
        desired.ddl.as_deref().and_then(table),
        current.ddl.as_deref().and_then(table),
    )
    else {
        return vec![];
    };
    let mut details = vec![];
    for c in &new {
        let key = column_name(c);
        match old.iter().find(|b| column_name(b) == key) {
            None => details.push(format!("+ column {c}")),
            Some(b) if b != c => details.push(format!("~ column {key}: {b} → {c}")),
            _ => {}
        }
    }
    for c in &old {
        if !new.iter().any(|a| column_name(a) == column_name(c)) {
            details.push(format!("− column {c}"));
        }
    }
    if nc != oc {
        details.push("Table constraints changed; inspect both definitions".into());
    }
    if no != oo {
        details.push("Table options changed; inspect both definitions".into());
    }
    details
}
pub fn compare(desired: &SchemaSnapshot, current: &SchemaSnapshot) -> Vec<Difference> {
    fn map(s: &SchemaSnapshot) -> BTreeMap<(&str, &str), &SchemaObject> {
        s.objects
            .iter()
            .map(|o| ((o.kind.as_str(), o.name.as_str()), o))
            .collect()
    }
    let (source, target) = (map(desired), map(current));
    let keys: BTreeSet<_> = source.keys().chain(target.keys()).copied().collect();
    keys.into_iter()
        .map(|key| difference(key, source.get(&key).copied(), target.get(&key).copied()))
        .collect()
}
/// How one object differs; renames are never inferred.
fn difference(
    (kind, name): (&str, &str),
    desired: Option<&SchemaObject>,
    current: Option<&SchemaObject>,
) -> Difference {
    let (status, details, migration) = match (desired, current) {
        (Some(a), Some(b)) if a.ddl == b.ddl => (Status::Identical, vec![], None),
        (Some(a), Some(b)) if kind == "table" => {
            (Status::Modified, column_differences(a, b), additions(a, b))
        }
        (Some(_), Some(_)) => (Status::Modified, vec![], None),
        (Some(a), None) => (Status::Added, vec![], a.ddl.as_deref().and_then(draft_ddl)),
        _ => (Status::Removed, vec![], None),
    };
    let guidance = (status != Status::Identical && migration.is_none()).then(|| {
        if status == Status::Removed {
            "Manual removal: verify dependencies and retained data. Renames are never inferred from added/removed pairs.".into()
        } else {
            "Manual rebuild/change: define explicit old-to-new data mapping; validate conversions/defaults and NOT NULL/UNIQUE/CHECK/FK constraints; preserve indexes, views and triggers; copy and verify rows in a reviewed transaction; validate foreign keys before publication. No executable rebuild is inferred.".into()
        }
    });
    Difference {
        kind: kind.into(),
        name: name.into(),
        status,
        details,
        migration,
        guidance,
    }
}
