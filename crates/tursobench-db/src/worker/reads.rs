//! Read requests become bounded browse-connection queries; table metadata is
//! published before the rows it describes.
use super::{
    Worker,
    execute::{Query, elapsed_ms},
};
use crate::{browse, inspection};
use tursobench_core::{
    Browse, Budget, Column, Completion, EventKind, Failure, ForeignKey, Ticket, Value,
};

const SCHEMA_SQL: &str = "SELECT type, name, tbl_name, CASE WHEN length(CAST(sql AS BLOB))<=16384 THEN sql ELSE NULL END AS sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1 ELSE 2 END, name";

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Reference {
    Rows,
    Count,
    Peek,
}
fn or_default(value: usize, default: usize) -> usize {
    if value == 0 { default } else { value }
}

impl<'a> Worker<'a> {
    pub(super) async fn browse_query(
        &self,
        ticket: Ticket,
        spec: Browse,
    ) -> Result<Query<'a>, Failure> {
        let budget = Budget {
            rows: or_default(spec.options.page_rows, 100),
            bytes: or_default(spec.options.page_bytes, 4 * 1024 * 1024),
            page: true,
            ..Budget::default()
        };
        let (table, sql, parameters) = browse::page(self.browse, spec).await?;
        self.output.send(ticket, EventKind::Table(table));
        Ok(Query::new(self.browse, sql, parameters.into(), budget))
    }
    pub(super) async fn count_filtered(&self, spec: &Browse) -> Result<Query<'a>, Failure> {
        let (sql, parameters) = browse::count(self.browse, spec).await?;
        Ok(Query::read(self.browse, sql, parameters.into()))
    }
    pub(super) async fn lookup(
        &self,
        ticket: Ticket,
        table: &str,
        keys: Vec<(String, Value)>,
    ) -> Result<Query<'a>, Failure> {
        let table = browse::describe(self.browse, table).await?;
        let (sql, parameters) = inspection::lookup_sql(&table, keys)?;
        self.output.send(ticket, EventKind::Table(table));
        Ok(Query::read(self.browse, sql, parameters.into()))
    }
    pub(super) async fn reference_query(
        &self,
        ticket: Ticket,
        name: String,
        key: ForeignKey,
        values: Vec<Value>,
        mode: Reference,
    ) -> Result<Query<'a>, Failure> {
        let child = inspection::inspect(self.browse, &name).await?;
        let peek = mode == Reference::Peek;
        if !child.foreign_keys.contains(&key)
            || peek && (key.columns.len() != values.len() || values.contains(&Value::Null))
        {
            return Err(Failure::input(if peek {
                "Relationship metadata changed or key is NULL"
            } else {
                "Relationship metadata changed; refresh the schema"
            }));
        }
        let parent = browse::describe(self.browse, &key.target).await?;
        if peek {
            return self.peek(&parent, &key, values);
        }
        let count = mode == Reference::Count;
        let sql = inspection::references_sql(&child.table, &parent, &key, &values, count)?;
        self.output.send(ticket, EventKind::Table(child.table));
        let rows = if count { Budget::default().rows } else { 100 };
        Ok(Query::new(
            self.browse,
            sql,
            values.into(),
            Budget {
                rows,
                ..Budget::default()
            },
        ))
    }
    /// The one parent row a child's key refers to.
    fn peek(
        &self,
        parent: &tursobench_core::Table,
        key: &ForeignKey,
        values: Vec<Value>,
    ) -> Result<Query<'a>, Failure> {
        let keys = key
            .columns
            .iter()
            .zip(values)
            .map(|((_, target), value)| {
                target
                    .clone()
                    .map(|target| (target, value))
                    .ok_or_else(|| Failure::input("Unresolved parent key"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (sql, parameters) = inspection::lookup_sql(parent, keys)?;
        Ok(Query::new(
            self.browse,
            sql,
            parameters.into(),
            Budget {
                rows: 1,
                bytes: 512 * 1024,
                ..Budget::default()
            },
        ))
    }
    pub(super) async fn profile(
        &self,
        request: &crate::profile::Request,
    ) -> Result<Query<'a>, Failure> {
        let table = browse::describe(self.browse, &request.table).await?;
        let stddev = self.browse.prepare("SELECT stddev(1)").await.is_ok();
        let sql = crate::profile::query(&table, request, stddev)?;
        Ok(Query::read(self.browse, sql, vec![].into()))
    }
    pub(super) async fn diagram(&self) -> Result<EventKind, Failure> {
        inspection::diagram(self.browse)
            .await
            .map(EventKind::Diagram)
    }
    pub(super) async fn inspect(&self, name: &str) -> Result<EventKind, Failure> {
        inspection::inspect(self.browse, name)
            .await
            .map(EventKind::Inspection)
    }
    pub(super) fn catalog(&self) -> Query<'a> {
        let budget = Budget {
            rows: 10_000,
            bytes: 8 * 1024 * 1024,
            cell_bytes: 1024 * 1024,
            page: false,
        };
        Query::new(self.browse, SCHEMA_SQL.into(), vec![].into(), budget)
    }
    pub(super) async fn admin(
        &self,
        ticket: Ticket,
        change: Option<crate::admin::Change>,
    ) -> Result<EventKind, Failure> {
        if change.is_some() {
            self.settled("reopen before changing settings")?;
        }
        let started = std::time::Instant::now();
        let rows =
            crate::admin::rows(self.editor, self.browse, self.path, self.access, change).await?;
        let rows_seen = rows.len() as u64;
        let columns = crate::admin::COLUMNS.map(|name| Column {
            name: name.into(),
            declared_type: None,
        });
        self.output.send(ticket, EventKind::Columns(columns.into()));
        self.output.send(ticket, EventKind::Rows(rows));
        Ok(EventKind::Finished(Completion {
            elapsed_ms: elapsed_ms(started),
            rows_seen,
            ..Completion::new(self.transaction())
        }))
    }
}
