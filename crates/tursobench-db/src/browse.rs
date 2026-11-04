use crate::connection::{failure, from_engine};
use turso::Connection;
use tursobench_core::{
    Browse, Failure, Filter, FilterExpr, FilterOp, Table, TableColumn, Value, quote_identifier,
};

pub(crate) async fn describe(c: &Connection, name: &str) -> Result<Table, Failure> {
    if name.len() > 64 * 1024 {
        return Err(Failure::input("Table name exceeds metadata budget"));
    }
    let mut r = c
        .query(
            "SELECT type, sql FROM sqlite_schema WHERE name = ?1 COLLATE NOCASE AND type IN ('table', 'view')",
            [name],
        )
        .await
        .map_err(failure)?;
    let schema = r
        .next()
        .await
        .map_err(failure)?
        .ok_or_else(|| Failure::input("Table or view no longer exists"))?;
    let kind = schema.get_value(0).map_err(failure)?;
    let declarations = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => crate::sql::declared_types(&ddl),
        _ => Default::default(),
    };
    let rowid_key = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => crate::sql::inline_rowid_key(&ddl),
        _ => None,
    };
    let has_rowid = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => crate::sql::has_rowid(&ddl),
        _ => false,
    };
    let binary_collations = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => {
            !crate::sql::tokens(&ddl).iter().any(|t| {
                t.style == crate::sql::TokenStyle::Keyword
                    && ddl[t.range.clone()].trim().eq_ignore_ascii_case("COLLATE")
            })
        }
        _ => false,
    };
    while r.next().await.map_err(failure)?.is_some() {}
    drop(r);
    let mut r = c
        .query(
            format!("PRAGMA table_xinfo({})", quote_identifier(name)),
            (),
        )
        .await
        .map_err(failure)?;
    let mut columns = Vec::new();
    let mut bytes = name.len();
    while let Some(row) = r.next().await.map_err(failure)? {
        let get = |i| row.get_value(i).map(from_engine).map_err(failure);
        let text = |i| match get(i)? {
            Value::Text(s) => Ok(s),
            _ => Err(Failure::input("Unexpected schema metadata")),
        };
        let integer = |i| match get(i)? {
            Value::Integer(n) => Ok(n),
            _ => Err(Failure::input("Unexpected schema metadata")),
        };
        let column = TableColumn {
            name: text(1)?,
            declared_type: declarations
                .get(&text(1)?.to_ascii_lowercase())
                .cloned()
                .unwrap_or(text(2)?),
            not_null: integer(3)? != 0
                || rowid_key
                    .as_ref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&text(1).unwrap_or_default())),
            default_sql: match get(4)? {
                Value::Text(s) => Some(s),
                _ => None,
            },
            primary_key: integer(5)?,
            hidden: integer(6)?,
        };
        bytes += column.name.len()
            + column.declared_type.len()
            + column.default_sql.as_ref().map_or(0, String::len);
        if bytes > 1024 * 1024 {
            return Err(Failure::input("Table descriptor exceeds 1 MiB"));
        }
        columns.push(column);
        if columns.len() > 512 {
            return Err(Failure::input("Table exceeds 512-column budget"));
        }
    }
    Ok(Table {
        name: name.into(),
        columns,
        is_table: kind == turso::Value::Text("table".into()),
        has_rowid,
        binary_collations,
    })
}

struct Predicates<'a> {
    table: &'a Table,
    params: Vec<Value>,
    leaves: usize,
    bytes: usize,
}
impl Predicates<'_> {
    fn column(&self, i: usize) -> Result<String, Failure> {
        self.table
            .columns
            .get(i)
            .map(|c| quote_identifier(&c.name))
            .ok_or_else(|| Failure::input("Invalid column index"))
    }
    fn bind(&mut self, value: Value) -> Result<String, Failure> {
        self.bytes += value.bytes();
        if self.bytes > 1024 * 1024 || self.params.len() >= 1000 {
            return Err(Failure::input("Filter bindings exceed 1 MiB / 1000 values"));
        }
        self.params.push(value);
        Ok(format!("?{}", self.params.len()))
    }
    fn expression(&mut self, expr: &FilterExpr, depth: usize) -> Result<String, Failure> {
        if depth > 8 {
            return Err(Failure::input("Filter nesting exceeds 8 levels"));
        }
        self.leaves += 1;
        if self.leaves > 128 {
            return Err(Failure::input("Filters exceed 128 nodes"));
        }
        match expr {
            FilterExpr::All(parts) | FilterExpr::Any(parts) => {
                if parts.is_empty() {
                    return Err(Failure::input("Empty filter group"));
                }
                let parts = parts
                    .iter()
                    .map(|p| self.expression(p, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!(
                    "({})",
                    parts.join(if matches!(expr, FilterExpr::All(_)) {
                        " AND "
                    } else {
                        " OR "
                    })
                ))
            }
            FilterExpr::In { column, values } => {
                let col = self.column(*column)?;
                if values.is_empty() {
                    return Err(Failure::input("IN requires at least one value"));
                }
                let params = values
                    .iter()
                    .cloned()
                    .map(|v| self.bind(v))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("{col} IN ({})", params.join(",")))
            }
            FilterExpr::Compare(Filter { column, op, value }) => {
                let col = self.column(*column)?;
                let operator = match op {
                    FilterOp::Equal => "IS",
                    FilterOp::NotEqual => "IS NOT",
                    FilterOp::Less => "<",
                    FilterOp::Greater => ">",
                    FilterOp::LessOrEqual => "<=",
                    FilterOp::GreaterOrEqual => ">=",
                    FilterOp::Contains => "LIKE",
                    FilterOp::IsNull => "IS NULL",
                    FilterOp::IsNotNull => "IS NOT NULL",
                };
                if op.unary() {
                    return Ok(format!("{col} {operator}"));
                }
                let value = if *op == FilterOp::Contains {
                    let Value::Text(text) = value else {
                        return Err(Failure::input("Contains requires a TEXT value"));
                    };
                    Value::Text(format!(
                        "%{}%",
                        text.replace('\\', "\\\\")
                            .replace('%', "\\%")
                            .replace('_', "\\_")
                    ))
                } else {
                    value.clone()
                };
                let p = self.bind(value)?;
                Ok(format!(
                    "{col} {operator} {p}{}",
                    if *op == FilterOp::Contains {
                        " ESCAPE '\\'"
                    } else {
                        ""
                    }
                ))
            }
        }
    }
}
pub(crate) fn supports_seek(table: &Table) -> bool {
    let keys = table.keys();
    table.is_table && !keys.is_empty() && keys.iter().all(|i| table.columns[*i].not_null)
}
fn order_columns(table: &Table, spec: &Browse) -> Result<Vec<(usize, bool)>, Failure> {
    let mut order = Vec::new();
    if let Some(i) = spec.sort {
        order.push((i, spec.descending));
    }
    for &(i, desc) in &spec.options.sorts {
        if !order.iter().any(|(col, _)| *col == i) {
            order.push((i, desc));
        }
    }
    if order.len() > 8 {
        return Err(Failure::input("Sort exceeds eight columns"));
    }
    for i in table.keys() {
        if !order.iter().any(|(col, _)| *col == i) {
            order.push((i, spec.descending));
        }
    }
    for (i, _) in &order {
        if *i >= table.columns.len() {
            return Err(Failure::input("Invalid sort column"));
        }
    }
    Ok(order)
}
fn filtered<'a>(
    table: &'a Table,
    spec: &Browse,
    relation: Option<(String, Vec<Value>)>,
) -> Result<(Predicates<'a>, Vec<String>), Failure> {
    let mut builder = Predicates {
        table,
        params: Vec::new(),
        leaves: 0,
        bytes: 0,
    };
    if spec.filters.len() > 32 {
        return Err(Failure::input("Filters exceed 32 predicates"));
    }
    if let Some((_, params)) = &relation {
        builder.params = params.clone();
        builder.bytes = params.iter().map(Value::bytes).sum();
    }
    let mut parts = spec
        .filters
        .iter()
        .map(|f| builder.expression(&FilterExpr::Compare(f.clone()), 0))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some((condition, _)) = relation {
        parts.push(condition);
    }
    if let Some(expr) = &spec.options.predicate {
        parts.push(builder.expression(expr, 0)?);
    }
    Ok((builder, parts))
}
pub(crate) fn count_sql(
    table: &Table,
    spec: &Browse,
    relation: Option<(String, Vec<Value>)>,
) -> Result<(String, Vec<Value>), Failure> {
    let (builder, parts) = filtered(table, spec, relation)?;
    Ok((
        format!(
            "SELECT count(*) AS row_count FROM {} AS c{}",
            quote_identifier(&table.name),
            where_clause(&parts)
        ),
        builder.params,
    ))
}
fn where_clause(parts: &[String]) -> String {
    if parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", parts.join(" AND "))
    }
}
pub(crate) fn browse_sql(
    table: &Table,
    spec: Browse,
    relation: Option<(String, Vec<Value>)>,
) -> Result<(String, Vec<Value>), Failure> {
    if table.columns.is_empty() || table.columns.iter().any(|c| c.hidden != 0) {
        return Err(Failure::input(
            "Hidden/generated columns are not supported by this browser",
        ));
    }
    let (mut builder, mut predicates) = filtered(table, &spec, relation)?;
    let mut order = order_columns(table, &spec)?;
    let seek = spec.options.cursor.as_ref();
    if let Some(cursor) = seek {
        if !supports_seek(table) || cursor.len() != table.columns.len() {
            return Err(Failure::input(
                "Cursor requires a proven non-null primary key",
            ));
        }
        let mut branches = Vec::new();
        let mut prefix = Vec::new();
        for &(i, desc) in &order {
            let col = builder.column(i)?;
            let param = builder.bind(cursor[i].clone())?;
            let less = desc != spec.options.reverse;
            let cmp = match (&cursor[i], less, table.columns[i].not_null) {
                (Value::Null, true, _) => "0".into(),
                (Value::Null, false, _) => format!("{col} IS NOT NULL"),
                (_, true, false) => format!("({col} < {param} OR {col} IS NULL)"),
                (_, true, true) => format!("{col} < {param}"),
                (_, false, _) => format!("{col} > {param}"),
            };
            let mut branch = prefix.clone();
            branch.push(cmp);
            branches.push(format!("({})", branch.join(" AND ")));
            prefix.push(format!("{col} IS {param}"));
        }
        predicates.push(format!("({})", branches.join(" OR ")));
    }
    let mut terms = order
        .iter()
        .map(|&(i, desc)| {
            Ok(format!(
                "{} {}",
                builder.column(i)?,
                if desc != spec.options.reverse {
                    "DESC"
                } else {
                    "ASC"
                }
            ))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    if !supports_seek(table) {
        let rowid = table
            .has_rowid
            .then(|| {
                ["rowid", "_rowid_", "oid"].into_iter().find(|name| {
                    !table
                        .columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case(name))
                })
            })
            .flatten();
        if let Some(rowid) = rowid {
            terms.push(format!(
                "{} {}",
                quote_identifier(rowid),
                if spec.descending != spec.options.reverse {
                    "DESC"
                } else {
                    "ASC"
                }
            ));
        } else {
            for i in 0..table.columns.len() {
                if !order.iter().any(|(col, _)| *col == i) {
                    order.push((i, false));
                    terms.push(format!(
                        "{} COLLATE BINARY, typeof({})",
                        builder.column(i)?,
                        builder.column(i)?
                    ));
                }
            }
        }
    }
    let names = (0..table.columns.len())
        .map(|i| builder.column(i))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let offset = if seek.is_some() { 0 } else { spec.offset };
    if offset > i64::MAX as usize {
        return Err(Failure::input("Row offset exceeds engine integer range"));
    }
    Ok((
        format!(
            "SELECT {names} FROM {} AS c{} ORDER BY {} LIMIT 100 OFFSET {offset}",
            quote_identifier(&table.name),
            where_clause(&predicates),
            terms.join(", ")
        ),
        builder.params,
    ))
}

pub(crate) async fn relationship(
    c: &Connection,
    table: &Table,
    spec: &Browse,
) -> Result<Option<(String, Vec<Value>)>, Failure> {
    let Some((key, values)) = spec.options.relation.as_deref() else {
        return Ok(None);
    };
    let info = crate::inspection::inspect(c, &table.name).await?;
    if !info.foreign_keys.contains(key) {
        return Err(Failure::input("Relationship metadata changed; refresh"));
    }
    let parent = describe(c, &key.target).await?;
    Ok(Some((
        crate::inspection::reference_condition(table, &parent, key, values)?,
        values.clone(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_browse_does_not_require_a_temporary_sort() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let db = turso::Builder::new_local(dir.path().join("plan.db").to_str().unwrap())
                    .build()
                    .await
                    .unwrap();
                let c = db.connect().unwrap();
                c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)", ())
                    .await
                    .unwrap();
                c.execute("CREATE INDEX by_name ON t(name)", ())
                    .await
                    .unwrap();
                let table = describe(&c, "t").await.unwrap();
                for sort in [None, Some(1)] {
                    let (sql, _) = browse_sql(
                        &table,
                        Browse {
                            table: "t".into(),
                            offset: 1_000_100,
                            sort,
                            descending: false,
                            filters: vec![],
                            options: Default::default(),
                        },
                        None,
                    )
                    .unwrap();
                    let mut rows = c
                        .query(format!("EXPLAIN QUERY PLAN {sql}"), ())
                        .await
                        .unwrap();
                    let mut plan = String::new();
                    while let Some(row) = rows.next().await.unwrap() {
                        for i in 0..rows.column_count() {
                            plan.push_str(&format!("{:?}", row.get_value(i).unwrap()));
                        }
                    }
                    assert!(!plan.to_uppercase().contains("TEMP B-TREE"), "{plan}");
                }
            });
    }
}
