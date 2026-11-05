use tursobench_core::{Failure, Table, quote_identifier};

#[derive(Clone, Debug)]
pub struct Request {
    pub table: String,
    /// None profiles every visible column (up to 24); wide tables use one column.
    pub column: Option<String>,
    /// Deterministic prefix sample, not a statistical/random sample.
    pub sample_rows: Option<usize>,
}
pub(crate) fn query(table: &Table, request: &Request, stddev: bool) -> Result<String, Failure> {
    if request.sample_rows.is_some_and(|n| n == 0 || n > 100_000) {
        return Err(Failure::input("Profile sample must contain 1–100,000 rows"));
    }
    let columns: Vec<_> = table
        .columns
        .iter()
        .filter(|c| c.hidden == 0 && request.column.as_ref().is_none_or(|name| name == &c.name))
        .collect();
    if columns.is_empty() || columns.len() > 24 {
        return Err(Failure::input(
            "Select one column for tables wider than 24 columns; the selected column must exist",
        ));
    }
    let mut parts = Vec::new();
    for column in columns {
        let c = quote_identifier(&column.name);
        let label = format!("'{}'", column.name.replace('\'', "''"));
        let numeric = format!("CASE WHEN typeof({c}) IN ('integer','real') THEN {c} END");
        let text = format!("CASE WHEN typeof({c})='text' THEN length({c}) END");
        let mut metrics = vec![
            ("rows", "count(*)".to_string()),
            ("nulls", format!("count(*)-count({c})")),
            ("distinct non-null", format!("count(DISTINCT {c})")),
            ("completeness", format!("1.0*count({c})/nullif(count(*),0)")),
            (
                "distinct / non-null",
                format!("1.0*count(DISTINCT {c})/nullif(count({c}),0)"),
            ),
            ("minimum", format!("min({c})")),
            ("maximum", format!("max({c})")),
            ("numeric rows", format!("count({numeric})")),
            ("numeric average", format!("avg({numeric})")),
            ("numeric sum (real)", format!("total({numeric})")),
            ("text minimum length", format!("min({text})")),
            ("text maximum length", format!("max({text})")),
            ("text average length", format!("avg({text})")),
        ];
        if stddev {
            metrics.push(("numeric stddev", format!("stddev({numeric})")));
        } else {
            metrics.push((
                "numeric stddev",
                "CASE WHEN count(*) >= 0 THEN 'Unsupported by this engine build' END".into(),
            ));
        }
        for (metric, expr) in metrics {
            parts.push(format!("SELECT {label} AS column_name, '{metric}' AS statistic, {expr} AS value, NULL AS frequency FROM profile_source"));
        }
        parts.push(format!("SELECT {label}, 'top value', value, frequency FROM (SELECT {c} AS value, count(*) AS frequency FROM profile_source WHERE {c} IS NOT NULL GROUP BY {c} ORDER BY frequency DESC, {c} LIMIT 5)"));
    }
    let limit = request
        .sample_rows
        .map_or(String::new(), |n| format!(" LIMIT {n}"));
    Ok(format!(
        "WITH profile_source AS MATERIALIZED (SELECT * FROM main.{}{limit}) {}",
        quote_identifier(&table.name),
        parts.join(" UNION ALL ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unsupported_aggregate_is_one_diagnostic_row_even_for_empty_input() {
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let c = db.connect().unwrap();
        c.execute("CREATE TABLE t(v)", ()).await.unwrap();
        let table = crate::browse::describe(&c, "t").await.unwrap();
        for insert in [false, true] {
            if insert {
                c.execute("INSERT INTO t VALUES(1),(2),(3)", ())
                    .await
                    .unwrap();
            }
            let sql = query(
                &table,
                &Request {
                    table: "t".into(),
                    column: None,
                    sample_rows: None,
                },
                false,
            )
            .unwrap();
            let mut rows = c.query(sql, ()).await.unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().await.unwrap() {
                if row.get_value(1).unwrap() == turso::Value::Text("numeric stddev".into()) {
                    count += 1;
                }
            }
            assert_eq!(count, 1);
        }
    }
}
