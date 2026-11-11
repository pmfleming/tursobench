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
    let projection = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let limit = request
        .sample_rows
        .map_or(String::new(), |n| format!(" LIMIT {n}"));
    let mut sources = vec![format!(
        "profile_source AS MATERIALIZED (SELECT {projection} FROM main.{}{limit})",
        quote_identifier(&table.name),
    )];
    let mut parts = Vec::new();
    for (index, column) in columns.into_iter().enumerate() {
        let c = quote_identifier(&column.name);
        let label = format!("'{}'", column.name.replace('\'', "''"));
        let numeric = format!("CASE WHEN typeof({c}) IN ('integer','real') THEN {c} END");
        let text = format!("CASE WHEN typeof({c})='text' THEN length({c}) END");
        let deviation = if stddev {
            format!("stddev({numeric})")
        } else {
            "'Unsupported by this engine build'".into()
        };
        // One source pass per column; the presentation branches read one stats row.
        let stats = format!("profile_stats_{index}");
        sources.push(format!(
            "{stats} AS MATERIALIZED (SELECT count(*) AS population, count({c}) AS non_null, \
             count(DISTINCT {c}) AS distinct_count, min({c}) AS v_min, max({c}) AS v_max, \
             count({numeric}) AS numeric_count, avg({numeric}) AS numeric_avg, \
             total({numeric}) AS numeric_sum, min({text}) AS text_min, \
             max({text}) AS text_max, avg({text}) AS text_avg, \
             {deviation} AS deviation FROM profile_source)"
        ));
        for (metric, expr) in [
            ("rows", "population"),
            ("nulls", "population-non_null"),
            ("distinct non-null", "distinct_count"),
            ("completeness", "1.0*non_null/nullif(population,0)"),
            (
                "distinct / non-null",
                "1.0*distinct_count/nullif(non_null,0)",
            ),
            ("minimum", "v_min"),
            ("maximum", "v_max"),
            ("numeric rows", "numeric_count"),
            ("numeric average", "numeric_avg"),
            ("numeric sum (real)", "numeric_sum"),
            ("text minimum length", "text_min"),
            ("text maximum length", "text_max"),
            ("text average length", "text_avg"),
            ("numeric stddev", "deviation"),
        ] {
            parts.push(format!("SELECT {label} AS column_name, '{metric}' AS statistic, {expr} AS value, NULL AS frequency FROM {stats}"));
        }
        parts.push(format!("SELECT {label}, 'top value', value, frequency FROM (SELECT {c} AS value, count(*) AS frequency FROM profile_source WHERE {c} IS NOT NULL GROUP BY {c} ORDER BY frequency DESC, {c} LIMIT 5)"));
    }
    Ok(format!(
        "WITH {} {}",
        sources.join(", "),
        parts.join(" UNION ALL ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn consolidated_statistics_preserve_values_sampling_and_projection() {
        use turso::Value::{Integer, Null, Real, Text};
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let c = db.connect().unwrap();
        c.execute("CREATE TABLE t(n, label, unused BLOB)", ())
            .await
            .unwrap();
        c.execute(
            "INSERT INTO t VALUES(-2,'λ',NULL),(4,'hello',NULL),(4,NULL,NULL),(NULL,'λ',NULL)",
            (),
        )
        .await
        .unwrap();
        let table = crate::browse::describe(&c, "t").await.unwrap();
        for sample in [None, Some(3)] {
            let sql = query(
                &table,
                &Request {
                    table: "t".into(),
                    column: Some("n".into()),
                    sample_rows: sample,
                },
                false,
            )
            .unwrap();
            assert!(!sql.contains("SELECT *"));
            assert!(!sql.contains("unused"));
            let mut rows = c.query(sql, ()).await.unwrap();
            let mut metrics = std::collections::BTreeMap::new();
            let mut frequencies = Vec::new();
            while let Some(row) = rows.next().await.unwrap() {
                let Text(metric) = row.get_value(1).unwrap() else {
                    panic!("statistic name")
                };
                let value = row.get_value(2).unwrap();
                if metric == "top value" {
                    frequencies.push((value, row.get_value(3).unwrap()));
                } else {
                    metrics.insert(metric, value);
                }
            }
            assert_eq!(metrics.len(), 14);
            for (name, expected) in [
                ("rows", Integer(if sample.is_some() { 3 } else { 4 })),
                ("nulls", Integer(i64::from(sample.is_none()))),
                ("distinct non-null", Integer(2)),
                (
                    "completeness",
                    Real(if sample.is_some() { 1.0 } else { 0.75 }),
                ),
                ("distinct / non-null", Real(2.0 / 3.0)),
                ("minimum", Integer(-2)),
                ("maximum", Integer(4)),
                ("numeric rows", Integer(3)),
                ("numeric average", Real(2.0)),
                ("numeric sum (real)", Real(6.0)),
                ("text minimum length", Null),
                ("text maximum length", Null),
                ("text average length", Null),
                (
                    "numeric stddev",
                    Text("Unsupported by this engine build".into()),
                ),
            ] {
                assert_eq!(metrics[name], expected, "{name}");
            }
            assert_eq!(
                frequencies,
                vec![(Integer(4), Integer(2)), (Integer(-2), Integer(1))]
            );
        }
        let sql = query(
            &table,
            &Request {
                table: "t".into(),
                column: Some("label".into()),
                sample_rows: None,
            },
            false,
        )
        .unwrap();
        let mut rows = c.query(sql, ()).await.unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            if row.get_value(1).unwrap() == Text("text average length".into()) {
                assert_eq!(row.get_value(2).unwrap(), Real(7.0 / 3.0));
            }
        }
    }
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
