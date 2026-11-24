use crate::connection::{failure, from_engine};
use turso::Connection;
use tursobench_core::{
    Browse, Failure, Filter, FilterExpr, FilterOp, Table, TableColumn, Value, quote_identifier,
};

/// Facts parsed from bounded CREATE text; missing or oversized DDL gives defaults.
#[derive(Default)]
struct Declaration {
    types: std::collections::BTreeMap<String, String>,
    rowid_key: Option<String>,
    has_rowid: bool,
    binary_collations: bool,
}
impl Declaration {
    fn parse(ddl: &str) -> Self {
        let collates = crate::sql::tokens(ddl).iter().any(|t| {
            t.style == crate::sql::TokenStyle::Keyword
                && ddl[t.range.clone()].trim().eq_ignore_ascii_case("COLLATE")
        });
        Self {
            types: crate::sql::declared_types(ddl),
            rowid_key: crate::sql::inline_rowid_key(ddl),
            has_rowid: crate::sql::has_rowid(ddl),
            binary_collations: !collates,
        }
    }
    /// One `PRAGMA table_xinfo` row, preferring the declared type spelling.
    fn column(&self, row: &turso::Row) -> Result<TableColumn, Failure> {
        let get = |i| row.get_value(i).map(from_engine).map_err(failure);
        let text = |i| match get(i)? {
            Value::Text(s) => Ok(s),
            _ => Err(Failure::input("Unexpected schema metadata")),
        };
        let integer = |i| match get(i)? {
            Value::Integer(n) => Ok(n),
            _ => Err(Failure::input("Unexpected schema metadata")),
        };
        let name = text(1)?;
        let reported_type = text(2)?;
        let declared_type = self
            .types
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or(reported_type);
        let rowid_key = self
            .rowid_key
            .as_ref()
            .is_some_and(|key| key.eq_ignore_ascii_case(&name));
        Ok(TableColumn {
            declared_type,
            not_null: integer(3)? != 0 || rowid_key,
            default_sql: match get(4)? {
                Value::Text(s) => Some(s),
                _ => None,
            },
            primary_key: integer(5)?,
            hidden: integer(6)?,
            name,
        })
    }
}
/// Whether `name` is a table (not a view), and its parsed declaration.
async fn declaration(c: &Connection, name: &str) -> Result<(bool, Declaration), Failure> {
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
    let is_table = schema.get_value(0).map_err(failure)? == turso::Value::Text("table".into());
    let declaration = match schema.get_value(1).map_err(failure)? {
        turso::Value::Text(ddl) if ddl.len() <= 1024 * 1024 => Declaration::parse(&ddl),
        _ => Declaration::default(),
    };
    while r.next().await.map_err(failure)?.is_some() {}
    Ok((is_table, declaration))
}
pub(crate) async fn describe(c: &Connection, name: &str) -> Result<Table, Failure> {
    if name.len() > 64 * 1024 {
        return Err(Failure::input("Table name exceeds metadata budget"));
    }
    let (is_table, declaration) = declaration(c, name).await?;
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
        let column = declaration.column(&row)?;
        bytes += column.bytes();
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
        is_table,
        has_rowid: declaration.has_rowid,
        binary_collations: declaration.binary_collations,
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
    fn bind(&mut self, value: &Value) -> Result<String, Failure> {
        self.bytes += value.bytes();
        if self.bytes > 1024 * 1024 || self.params.len() >= 1000 {
            return Err(Failure::input("Filter bindings exceed 1 MiB / 1000 values"));
        }
        self.params.push(value.clone());
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
                let joiner = if matches!(expr, FilterExpr::All(_)) {
                    " AND "
                } else {
                    " OR "
                };
                let parts = parts
                    .iter()
                    .map(|p| self.expression(p, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("({})", parts.join(joiner)))
            }
            FilterExpr::In { column, values } => {
                let col = self.column(*column)?;
                if values.is_empty() {
                    return Err(Failure::input("IN requires at least one value"));
                }
                let params = values
                    .iter()
                    .map(|v| self.bind(v))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("{col} IN ({})", params.join(",")))
            }
            FilterExpr::Compare(filter) => self.compare(filter),
        }
    }
    fn compare(&mut self, Filter { column, op, value }: &Filter) -> Result<String, Failure> {
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
        if *op != FilterOp::Contains {
            let p = self.bind(value)?;
            return Ok(format!("{col} {operator} {p}"));
        }
        let Value::Text(text) = value else {
            return Err(Failure::input("Contains requires a TEXT value"));
        };
        let pattern = text
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let p = self.bind(&Value::Text(format!("%{pattern}%")))?;
        Ok(format!("{col} LIKE {p} ESCAPE '\\'"))
    }
}
pub(crate) fn supports_seek(table: &Table) -> bool {
    let keys = table.keys();
    table.is_table
        && !keys.is_empty()
        && keys.len() <= 8
        && keys.iter().all(|i| table.columns[*i].not_null)
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
    if spec.filters.len() > 32 {
        return Err(Failure::input("Filters exceed 32 predicates"));
    }
    let (condition, params) = relation.unzip();
    let params = params.unwrap_or_default();
    let mut builder = Predicates {
        table,
        bytes: params.iter().map(Value::bytes).sum(),
        params,
        // Each chip is one leaf of the node budget.
        leaves: spec.filters.len(),
    };
    let mut parts = spec
        .filters
        .iter()
        .map(|f| builder.compare(f))
        .collect::<Result<Vec<_>, _>>()?;
    parts.extend(condition);
    if let Some(expr) = &spec.options.predicate {
        parts.push(builder.expression(expr, 0)?);
    }
    Ok((builder, parts))
}
fn count_sql(
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
fn direction(descending: bool) -> &'static str {
    if descending { "DESC" } else { "ASC" }
}
/// An unshadowed name for the implicit rowid, if the table has one.
fn rowid_alias(table: &Table) -> Option<&'static str> {
    let shadowed = |name: &str| {
        table
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
    };
    table
        .has_rowid
        .then(|| {
            ["rowid", "_rowid_", "oid"]
                .into_iter()
                .find(|name| !shadowed(name))
        })
        .flatten()
}
impl Predicates<'_> {
    /// Rows strictly after `cursor` in `order`: one branch per order prefix.
    fn seek(
        &mut self,
        order: &[(usize, bool)],
        cursor: &[Value],
        reverse: bool,
    ) -> Result<String, Failure> {
        let mut branches = Vec::new();
        let mut prefix = Vec::new();
        for &(i, desc) in order {
            let col = self.column(i)?;
            let param = self.bind(&cursor[i])?;
            let less = desc != reverse;
            let cmp = match (&cursor[i], less, self.table.columns[i].not_null) {
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
        Ok(format!("({})", branches.join(" OR ")))
    }
    /// Without a seekable key, rowid (or every remaining column, binary and
    /// by storage class) makes the order total.
    fn tiebreak(&self, order: &[(usize, bool)], descending: bool) -> Result<Vec<String>, Failure> {
        if let Some(rowid) = rowid_alias(self.table) {
            return Ok(vec![format!(
                "{} {}",
                quote_identifier(rowid),
                direction(descending)
            )]);
        }
        let mut terms = Vec::new();
        for i in 0..self.table.columns.len() {
            if !self.table.is_table || !order.iter().any(|(col, _)| *col == i) {
                let col = self.column(i)?;
                terms.push(format!("{col} COLLATE BINARY, typeof({col})"));
            }
        }
        Ok(terms)
    }
}
/// The bounded LIMIT and OFFSET for one page.
fn window(spec: &Browse) -> Result<(usize, usize), Failure> {
    let seek = spec.options.cursor.is_some();
    let offset = if seek { 0 } else { spec.offset };
    let limit = if spec.options.page_rows == 0 {
        100
    } else {
        spec.options.page_rows
    };
    if spec.options.page_bytes > 4 * 1024 * 1024 {
        return Err(Failure::input("Page byte budget exceeds 4 MiB"));
    }
    if limit > 100 || (spec.options.tail && (seek || spec.options.reverse)) {
        return Err(Failure::input("Invalid controlled page window"));
    }
    if offset > i64::MAX as usize {
        return Err(Failure::input("Row offset exceeds engine integer range"));
    }
    Ok((limit, offset))
}
fn browse_sql(
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
    let order = order_columns(table, &spec)?;
    let reverse = spec.options.reverse;
    if let Some(cursor) = &spec.options.cursor {
        if !supports_seek(table) || cursor.len() != table.columns.len() {
            return Err(Failure::input(
                "Cursor requires a proven non-null primary key",
            ));
        }
        predicates.push(builder.seek(&order, cursor, reverse)?);
    }
    let mut terms = order
        .iter()
        .map(|&(i, desc)| {
            Ok(format!(
                "{} {}",
                builder.column(i)?,
                direction(desc != reverse)
            ))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    if !supports_seek(table) {
        terms.extend(builder.tiebreak(&order, spec.descending != reverse)?);
    }
    let names = (0..table.columns.len())
        .map(|i| builder.column(i))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let (limit, offset) = window(&spec)?;
    Ok((
        format!(
            "SELECT {names} FROM {} AS c{} ORDER BY {} LIMIT {limit} OFFSET {offset}",
            quote_identifier(&table.name),
            where_clause(&predicates),
            terms.join(", ")
        ),
        builder.params,
    ))
}

/// The page query for `spec`, with the table it reads.
pub(crate) async fn page(
    c: &Connection,
    spec: Browse,
) -> Result<(Table, String, Vec<Value>), Failure> {
    let table = describe(c, &spec.table).await?;
    let relation = relationship(c, &table, &spec).await?;
    let (sql, parameters) = browse_sql(&table, spec, relation)?;
    Ok((table, sql, parameters))
}
pub(crate) async fn count(c: &Connection, spec: &Browse) -> Result<(String, Vec<Value>), Failure> {
    let table = describe(c, &spec.table).await?;
    let relation = relationship(c, &table, spec).await?;
    count_sql(&table, spec, relation)
}

async fn relationship(
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
    use super::{browse_sql, describe};
    use tursobench_core::Browse;
    #[tokio::test]
    async fn indexed_browse_does_not_require_a_temporary_sort() {
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
    }
}
