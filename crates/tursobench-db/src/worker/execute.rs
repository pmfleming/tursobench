//! Single-statement execution with retention and delivery budgets.
use super::Output;
use crate::{
    connection::{columns, failure, from_engine, params, state},
    sql,
    values::Depot,
};
use std::time::Instant;
use turso::Connection;
use tursobench_core::{
    Access, Bindings, Budget, Completion, Delivery, EventKind, Failure, Ticket, Value,
};

/// Rows, bytes and cell bytes one result may retain regardless of budget.
pub(super) const MAX_ROWS: usize = 10_000;
pub(super) const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CELL: usize = 1024 * 1024;

pub(super) struct Query<'a> {
    pub(super) connection: &'a Connection,
    pub(super) sql: String,
    pub(super) parameters: Bindings,
    pub(super) budget: Budget,
}
impl<'a> Query<'a> {
    pub(super) fn new(
        connection: &'a Connection,
        sql: String,
        parameters: Bindings,
        budget: Budget,
    ) -> Self {
        Self {
            connection,
            sql,
            parameters,
            budget,
        }
    }
    /// A query under the default result budget.
    pub(super) fn read(connection: &'a Connection, sql: String, parameters: Bindings) -> Self {
        Self::new(connection, sql, parameters, Budget::default())
    }
}
pub(super) struct ExecutionContext<'a> {
    pub(super) out: &'a Output,
    pub(super) access: Access,
    pub(super) depot: &'a mut Depot,
    pub(super) tail: bool,
}
pub(super) fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}
/// Retention and delivery budgets are independent of engine execution. Writes
/// drain their rows even when this buffer refuses further output.
struct ResultDelivery<'a> {
    out: &'a Output,
    ticket: Ticket,
    budget: Budget,
    tail: bool,
    stoppable: bool,
    chunk: Vec<Vec<Value>>,
    suffix: std::collections::VecDeque<(Vec<Value>, usize)>,
    kept: usize,
    bytes: usize,
    chunk_bytes: usize,
}
impl<'a> ResultDelivery<'a> {
    fn new(out: &'a Output, ticket: Ticket, budget: Budget, tail: bool, stoppable: bool) -> Self {
        Self {
            out,
            ticket,
            budget,
            tail,
            stoppable,
            chunk: Vec::new(),
            suffix: Default::default(),
            kept: 0,
            bytes: 0,
            chunk_bytes: 0,
        }
    }
    /// Reads every row; stoppable reads end at the first refused or discarded row.
    async fn drain(
        &mut self,
        rows: &mut turso::Rows,
        depot: &mut Depot,
        completion: &mut Completion,
    ) -> Result<(), Failure> {
        while let Some(row) = rows.next().await.map_err(failure)? {
            completion.rows_seen += 1;
            if self.out.discarded(self.ticket) {
                completion.delivery = Delivery::Discarded;
                self.chunk.clear();
                if self.stoppable {
                    break;
                }
                continue;
            }
            if completion.delivery == Delivery::Limited {
                continue;
            }
            let values = (0..rows.column_count())
                .map(|i| row.get_value(i).map(from_engine).map_err(failure))
                .collect::<Result<_, _>>()?;
            if !self.push(values, depot, completion) && self.stoppable {
                break;
            }
        }
        Ok(())
    }
    fn push(
        &mut self,
        mut values: Vec<Value>,
        depot: &mut Depot,
        completion: &mut Completion,
    ) -> bool {
        self.defer_cells(&mut values, depot);
        let size = values.iter().map(Value::bytes).sum::<usize>();
        if self.tail {
            self.retain_tail(values, size, completion);
            return true;
        }
        if self.kept >= self.budget.rows.min(MAX_ROWS)
            || self.bytes + size > self.budget.bytes.min(MAX_BYTES)
            || values
                .iter()
                .any(|v| v.bytes() > self.budget.cell_bytes.min(MAX_CELL))
        {
            if self.budget.page && self.kept > 0 {
                completion.page_has_more = Some(true);
            } else {
                completion.delivery = Delivery::Limited;
            }
            return false;
        }
        self.bytes += size;
        self.kept += 1;
        if !self.chunk(values, size) {
            completion.delivery = Delivery::Discarded;
        }
        true
    }
    fn defer_cells(&self, values: &mut [Value], depot: &mut Depot) {
        if self.budget.cell_bytes == 0 {
            return;
        }
        for value in values.iter_mut() {
            if value.bytes() > self.budget.cell_bytes.min(MAX_CELL) {
                *value = depot.defer(std::mem::replace(value, Value::Null));
            }
        }
        if self.budget.page && values.iter().map(Value::bytes).sum::<usize>() > self.budget.bytes {
            for value in values {
                if matches!(value, Value::Text(_) | Value::Blob(_)) {
                    *value = depot.defer(std::mem::replace(value, Value::Null));
                }
            }
        }
    }
    fn retain_tail(&mut self, values: Vec<Value>, size: usize, completion: &mut Completion) {
        while self.bytes + size > self.budget.bytes
            || self.suffix.len() >= self.budget.rows.min(100)
        {
            let Some((_, evicted_bytes)) = self.suffix.pop_front() else {
                break;
            };
            self.bytes -= evicted_bytes;
            completion.page_has_more = Some(true);
        }
        if size > self.budget.bytes {
            completion.delivery = Delivery::Limited;
            return;
        }
        self.bytes += size;
        self.suffix.push_back((values, size));
    }
    fn chunk(&mut self, values: Vec<Value>, size: usize) -> bool {
        self.chunk_bytes += size;
        self.chunk.push(values);
        if self.chunk.len() < 64 && self.chunk_bytes < 64 * 1024 {
            return true;
        }
        self.chunk_bytes = 0;
        self.out.send(
            self.ticket,
            EventKind::Rows(std::mem::take(&mut self.chunk)),
        )
    }
    fn finish(mut self) {
        while let Some((values, size)) = self.suffix.pop_front() {
            self.chunk(values, size);
        }
        if !self.chunk.is_empty() {
            self.out.send(self.ticket, EventKind::Rows(self.chunk));
        }
    }
}

fn validate(sql: &str, access: Access, parameters: &Bindings) -> Result<(), Failure> {
    if access == Access::ReadOnly {
        sql::validate_readonly(sql)?;
    }
    sql::validate_coordinated(sql)?;
    sql::validate_parameters(sql, parameters)
}

pub(super) async fn execute(
    ticket: Ticket,
    query: Query<'_>,
    context: ExecutionContext<'_>,
) -> Completion {
    let Query {
        connection: c,
        sql,
        parameters,
        budget,
    } = query;
    let ExecutionContext {
        out,
        access,
        depot,
        tail,
    } = context;
    let started = Instant::now();
    let stoppable = sql::stoppable_read(&sql);
    let read = stoppable.then(|| out.discard.interruptible(ticket.request, c));
    let mut completion = Completion {
        page_has_more: budget.page.then_some(false),
        ..Completion::new(state(c))
    };
    let mut delivery = ResultDelivery::new(out, ticket, budget, tail, stoppable);
    let result = async {
        sql::validate_single(&sql)?;
        if stoppable && out.discarded(ticket) {
            completion.delivery = Delivery::Discarded;
            return Ok(());
        }
        validate(&sql, access, &parameters)?;
        let mut stmt = c.prepare(&sql).await.map_err(failure)?;
        let columns = columns(&stmt)
            .ok_or_else(|| Failure::input("Result metadata exceeds display budget"))?;
        out.send(ticket, EventKind::Columns(columns));
        let parameters = params(parameters)?;
        if stoppable && out.discarded(ticket) {
            completion.delivery = Delivery::Discarded;
            return Ok(());
        }
        let mut rows = stmt.query(parameters).await.map_err(failure)?;
        delivery.drain(&mut rows, depot, &mut completion).await?;
        if budget.page && delivery.kept >= budget.rows {
            completion.page_has_more = Some(true);
        }
        completion.changed = stmt.n_change();
        Ok::<(), Failure>(())
    }
    .await;
    drop(read);
    delivery.finish();
    completion.elapsed_ms = elapsed_ms(started);
    completion.error = result.err();
    completion.transaction = state(c);
    if out.discarded(ticket) {
        completion.delivery = Delivery::Discarded;
    }
    completion
}
