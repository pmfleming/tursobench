use super::*;

pub(crate) async fn describe(c: &Connection, name: &str) -> Result<Table, Failure> {
    let mut r = c
        .query(
            "SELECT type FROM sqlite_schema WHERE name = ?1 AND type IN ('table', 'view')",
            [name],
        )
        .await
        .map_err(failure)?;
    let kind = r
        .next()
        .await
        .map_err(failure)?
        .ok_or_else(|| Failure::input("Table or view no longer exists"))?
        .get_value(0)
        .map_err(failure)?;
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
        columns.push(TableColumn {
            name: text(1)?,
            declared_type: text(2)?,
            not_null: integer(3)? != 0,
            default_sql: match get(4)? {
                Value::Text(s) => Some(s),
                _ => None,
            },
            primary_key: integer(5)?,
            hidden: integer(6)?,
        });
        if columns.len() > 512 {
            return Err(Failure::input("Table exceeds 512-column budget"));
        }
    }
    Ok(Table {
        name: name.into(),
        columns,
        is_table: kind == turso::Value::Text("table".into()),
    })
}

pub(crate) fn browse_sql(table: &Table, spec: &Browse) -> Result<(String, Vec<Value>), Failure> {
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
    let filter = if let Some((i, value)) = &spec.filter {
        params.push(value.clone());
        format!(" WHERE {} IS ?1", column(*i)?)
    } else {
        String::new()
    };
    let mut order = Vec::new();
    if let Some(i) = spec.sort {
        column(i)?;
        order.push(i);
    }
    let keys = table.keys();
    for i in if keys.is_empty() {
        (0..table.columns.len()).collect()
    } else {
        keys
    } {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    let direction = if spec.descending { " DESC" } else { " ASC" };
    let order = order
        .into_iter()
        .map(|i| column(i).map(|c| c + direction))
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
