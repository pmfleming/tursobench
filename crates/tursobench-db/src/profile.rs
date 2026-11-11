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
    let metrics = [
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
    ];
    let names = metrics
        .iter()
        .map(|(name, _)| format!("('{name}')"))
        .collect::<Vec<_>>()
        .join(", ");
    let cases = metrics
        .iter()
        .map(|(name, expression)| format!("WHEN '{name}' THEN {expression}"))
        .collect::<Vec<_>>()
        .join(" ");
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
    sources.push(format!(
        "profile_metrics(statistic) AS MATERIALIZED (VALUES {names})"
    ));
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
        // One aggregate row per column. A small metric table expands it without a
        // deeply nested UNION for every statistic in wide-table profiles.
        let stats = format!("profile_stats_{index}");
        sources.push(format!(
            "{stats} AS MATERIALIZED (SELECT count(*) AS population, count({c}) AS non_null, \
             count(DISTINCT {c}) AS distinct_count, min({c}) AS v_min, max({c}) AS v_max, \
             count({numeric}) AS numeric_count, avg({numeric}) AS numeric_avg, \
             total({numeric}) AS numeric_sum, min({text}) AS text_min, \
             max({text}) AS text_max, avg({text}) AS text_avg, \
             {deviation} AS deviation FROM profile_source)"
        ));
        parts.push(format!("SELECT {label} AS column_name, statistic, CASE statistic {cases} END AS value, NULL AS frequency FROM {stats} CROSS JOIN profile_metrics"));
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
    use super::{Request, query};
    use turso::{
        Connection, Value,
        Value::{Integer, Null, Real, Text},
    };
    use tursobench_core::Table;

    async fn read(c: &Connection, table: &Table, request: &Request) -> Vec<Vec<Value>> {
        let mut rows = c
            .query(query(table, request, false).unwrap(), ())
            .await
            .unwrap();
        let mut result = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            result.push((0..4).map(|i| row.get_value(i).unwrap()).collect());
        }
        result
    }
    #[tokio::test]
    async fn profiles_preserve_metrics_empty_inputs_sampling_and_column_budgets() {
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let c = db.connect().unwrap();
        c.execute("CREATE TABLE t(n, label, unused BLOB)", ())
            .await
            .unwrap();
        let table = crate::browse::describe(&c, "t").await.unwrap();
        let request = |column: Option<&str>, sample_rows| Request {
            table: "t".into(),
            column: column.map(str::to_owned),
            sample_rows,
        };
        let empty = read(&c, &table, &request(None, None)).await;
        assert_eq!(empty.len(), 42);
        assert_eq!(
            empty
                .iter()
                .filter(|r| r[1] == Text("numeric stddev".into()))
                .count(),
            3
        );
        assert!(
            empty
                .iter()
                .filter(|r| r[1] == Text("rows".into()))
                .all(|r| r[2] == Integer(0))
        );
        c.execute(
            "INSERT INTO t VALUES(-2,'λ',NULL),(4,'hello',NULL),(4,NULL,NULL),(NULL,'λ',NULL)",
            (),
        )
        .await
        .unwrap();
        for (sample, population, nulls, completeness) in [(None, 4, 1, 0.75), (Some(3), 3, 0, 1.0)]
        {
            let request = request(Some("n"), sample);
            let sql = query(&table, &request, false).unwrap();
            assert!(!sql.contains("SELECT *") && !sql.contains("unused"));
            let rows = read(&c, &table, &request).await;
            assert_eq!(rows.len(), 16);
            for (metric, value) in [
                ("rows", Integer(population)),
                ("nulls", Integer(nulls)),
                ("distinct non-null", Integer(2)),
                ("completeness", Real(completeness)),
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
                assert_eq!(
                    rows.iter().find(|r| r[1] == Text(metric.into())).unwrap()[2],
                    value,
                    "{metric}"
                );
            }
            let frequencies: Vec<_> = rows
                .iter()
                .filter(|r| r[1] == Text("top value".into()))
                .map(|r| (&r[2], &r[3]))
                .collect();
            assert_eq!(
                frequencies,
                vec![(&Integer(4), &Integer(2)), (&Integer(-2), &Integer(1))]
            );
        }
        let rows = read(&c, &table, &request(Some("label"), None)).await;
        assert_eq!(
            rows.iter()
                .find(|r| r[1] == Text("text average length".into()))
                .unwrap()[2],
            Real(7.0 / 3.0)
        );
        for invalid in [
            request(None, Some(0)),
            request(None, Some(100_001)),
            request(Some("missing"), None),
        ] {
            assert!(query(&table, &invalid, false).is_err());
        }
        let columns = (0..24)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        c.execute(format!("CREATE TABLE wide({columns})"), ())
            .await
            .unwrap();
        let wide = crate::browse::describe(&c, "wide").await.unwrap();
        assert_eq!(read(&c, &wide, &request(None, None)).await.len(), 24 * 14);
        c.execute("ALTER TABLE wide ADD COLUMN extra", ())
            .await
            .unwrap();
        let wide = crate::browse::describe(&c, "wide").await.unwrap();
        assert!(query(&wide, &request(None, None), false).is_err());
        assert_eq!(read(&c, &wide, &request(Some("c0"), None)).await.len(), 14);
    }
}
