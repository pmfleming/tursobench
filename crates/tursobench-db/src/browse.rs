use crate::connection::{failure, from_engine};
use turso::Connection;
use tursobench_core::{Browse, Failure, FilterOp, Table, TableColumn, Value, quote_identifier};

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
    let has_rowid = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => crate::sql::has_rowid(&ddl),
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
            declared_type: text(2)?,
            not_null: integer(3)? != 0,
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
    })
}

pub(crate) fn browse_sql(table: &Table, spec: Browse) -> Result<(String, Vec<Value>), Failure> {
    if table.columns.is_empty() || table.columns.iter().any(|c| c.hidden != 0) {
        return Err(Failure::input(
            "Hidden/generated columns are not supported by this browser",
        ));
    }
    let column = |i: usize| {
        table
            .columns
            .get(i)
            .map(|c| quote_identifier(&c.name))
            .ok_or_else(|| Failure::input("Invalid column index"))
    };
    let mut params = Vec::new();
    if spec.filters.len() > 32
        || spec.filters.iter().map(|f| f.value.bytes()).sum::<usize>() > 1024 * 1024
    {
        return Err(Failure::input(
            "Filters exceed the 32-predicate / 1 MiB budget",
        ));
    }
    let mut predicates = Vec::new();
    for filter in spec.filters {
        let col = column(filter.column)?;
        let op = match filter.op {
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
        if filter.op.unary() {
            predicates.push(format!("{col} {op}"));
            continue;
        }
        let value = if filter.op == FilterOp::Contains {
            let Value::Text(text) = filter.value else {
                return Err(Failure::input("Contains requires a TEXT value"));
            };
            Value::Text(format!(
                "%{}%",
                text.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ))
        } else {
            filter.value
        };
        params.push(value);
        let suffix = if filter.op == FilterOp::Contains {
            " ESCAPE '\\'"
        } else {
            ""
        };
        predicates.push(format!("{col} {op} ?{}{suffix}", params.len()));
    }
    let filter = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };
    let mut order = Vec::new();
    if let Some(i) = spec.sort {
        column(i)?;
        order.push(i);
    }
    let keys = table.keys();
    // Primary keys can contain NULL in rowid tables. Full-row suffixes keep
    // distinguishable rows ordered even when such keys are not unique.
    for i in keys.into_iter().chain(0..table.columns.len()) {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    let direction = if spec.descending { " DESC" } else { " ASC" };
    let order = order
        .into_iter()
        .map(|i| {
            column(i).map(|c| format!("{c} COLLATE BINARY{direction}, typeof({c}){direction}"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let names = (0..table.columns.len())
        .map(column)
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let sql = format!(
        "SELECT {names} FROM {}{filter} ORDER BY {order} LIMIT 100 OFFSET {}",
        quote_identifier(&table.name),
        spec.offset.min(1_000_000)
    );
    Ok((sql, params))
}
