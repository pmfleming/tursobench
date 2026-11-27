use super::{App, View, document::Document};
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ResultSet, Table, Ticket, Value};
use tursobench_db::Operation;

pub(super) const PAGE_BYTES: usize = 512 * 1024;
/// Retained table rows stay under this many bytes as pages load.
const RETAINED_BYTES: usize = 4 * 1024 * 1024;
pub(super) fn fetch_rows(limit: usize) -> usize {
    (keep_rows(limit) / 3).clamp(1, 100)
}
/// The most rows a table view retains as pages load.
fn keep_rows(limit: usize) -> usize {
    limit.clamp(200, 10_000) - 100
}
#[derive(Default)]
pub(super) struct Pages {
    pub pending: Option<Pending>,
    chunks: VecDeque<usize>,
    pub end: Option<usize>,
    pub error: String,
    pub restore_rows: usize,
    pub capacity_reached: bool,
    pub retry_previous: bool,
}
pub(super) struct Pending {
    pub result: ResultSet,
    owner: Option<Ticket>,
    offset: usize,
    previous: bool,
    reverse: bool,
    tail: bool,
    error: String,
}
impl Pages {
    /// Forget the sizes of `rows` evicted rows, from the back or the front.
    fn forget(&mut self, mut rows: usize, from_back: bool) {
        while rows > 0 {
            let chunk = if from_back {
                self.chunks.pop_back()
            } else {
                self.chunks.pop_front()
            };
            let Some(chunk) = chunk else {
                break;
            };
            if chunk > rows {
                let rest = chunk - rows;
                if from_back {
                    self.chunks.push_back(rest);
                } else {
                    self.chunks.push_front(rest);
                }
                break;
            }
            rows -= chunk;
        }
    }
}
/// Whether a table can be read by keyset from a boundary row: it has a
/// non-NULL key of at most eight columns.
fn seekable(table: &Table) -> bool {
    let keys = table.keys();
    table.is_table
        && !keys.is_empty()
        && keys.len() <= 8
        && keys.iter().all(|i| table.columns[*i].not_null)
}
/// What a page event reports went wrong: the table's schema changed, the read
/// failed, or the page arrived incomplete.
fn page_failure(kind: &EventKind, table: Option<&Table>) -> Option<String> {
    match kind {
        EventKind::Table(changed) if table != Some(changed) => {
            Some("Schema changed while loading; refresh the table".into())
        }
        EventKind::Failed(e) => Some(e.to_string()),
        EventKind::Finished(c) if c.error.is_some() || c.delivery != Delivery::Complete => {
            Some("Page delivery incomplete; refresh or retry".into())
        }
        _ => None,
    }
}
impl App {
    /// Nothing loads, pages or commits, and no edit is staged, so the
    /// retained rows can be replaced or extended.
    pub(super) fn paging_idle(&self) -> bool {
        let document = &self.database.document;
        !document.stale
            && document.active.is_none()
            && document.tools.pages.pending.is_none()
            && self.requests_enabled()
    }
    fn page_available(&self, previous: bool) -> bool {
        let document = &self.database.document;
        let complete = document
            .result
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery == Delivery::Complete && c.error.is_none());
        let Some(spec) = document
            .browse
            .as_ref()
            .filter(|_| complete && document.view == View::Browse && self.paging_idle())
        else {
            return false;
        };
        if previous {
            spec.offset > 0
        } else {
            let retained = spec.offset + document.result.rows.len();
            document.tools.pages.end.is_none_or(|last| retained < last)
        }
    }
    /// Browse from the row number typed in the status bar.
    pub(super) fn jump_to_row(&mut self) {
        let Some(mut next) = self.database.document.browse.clone() else {
            return;
        };
        next.offset = self.database.document.tools.jump_row.saturating_sub(1);
        next.options.cursor = None;
        next.options.reverse = false;
        next.options.tail = false;
        if self
            .submit(Operation::Browse(next.clone()), View::Browse)
            .is_some()
        {
            self.database.document.browse = Some(next);
        }
    }
    pub(super) fn request_page(&mut self, previous: bool) {
        if !self.page_available(previous) {
            return;
        }
        let document = &self.database.document;
        let Some(mut spec) = document.browse.clone() else {
            return;
        };
        let fetch_rows = fetch_rows(self.limits().row_limit);
        let start = spec.offset;
        let rows = &document.result.rows;
        // A keyed table continues from its boundary row, once that row is fully loaded.
        let edge = if previous { rows.first() } else { rows.last() };
        let cursor = edge.filter(|row| {
            document.table.as_ref().is_some_and(seekable)
                && !row.iter().any(|v| matches!(v, Value::Deferred(_)))
        });
        spec.offset = if previous {
            start.saturating_sub(fetch_rows)
        } else {
            start + rows.len()
        };
        let options = &mut spec.options;
        options.tail = previous && cursor.is_none();
        options.reverse = previous && cursor.is_some();
        options.page_rows = if options.tail {
            start.min(fetch_rows)
        } else {
            fetch_rows
        };
        options.page_bytes = PAGE_BYTES;
        options.cursor = cursor.cloned();
        let (offset, reverse, tail) = (spec.offset, options.reverse, options.tail);
        let Some(outcome) = self.database.request(Operation::Browse(spec)) else {
            return;
        };
        let document = &mut self.database.document;
        let pages = &mut document.tools.pages;
        pages.retry_previous = previous;
        match outcome {
            Ok(ticket) => {
                if pages.chunks.is_empty() && !document.result.rows.is_empty() {
                    pages.chunks.push_back(document.result.rows.len());
                }
                let mut result = ResultSet::default();
                result.start(ticket);
                pages.pending = Some(Pending {
                    result,
                    owner: document.result.current,
                    offset,
                    previous,
                    reverse,
                    tail,
                    error: String::new(),
                });
                pages.error.clear();
            }
            Err(e) => pages.error = e.to_string(),
        }
    }
    pub(super) fn maybe_page(&mut self, start: usize, end: usize, delta: f32) {
        let pages = &self.database.document.tools.pages;
        let rows = self.database.document.result.rows.len();
        if !pages.error.is_empty() {
            return;
        }
        if pages.restore_rows > rows {
            self.request_page(false);
        } else if start <= 20 && delta > 0.0 {
            self.request_page(true);
        } else if end + 20 >= rows && delta <= 0.0 && self.next_page_fits(start) {
            self.request_page(false);
        }
    }
    /// Whether the next page fits by evicting only rows above the view, both
    /// in row count and in retained bytes.
    fn next_page_fits(&self, start: usize) -> bool {
        // Repaints at EOF, during a fetch, or with drafts need no cell scans.
        if !self.page_available(false) {
            return false;
        }
        let limit = self.limits().row_limit;
        let result = &self.database.document.result;
        let rows_needed = (result.rows.len() + fetch_rows(limit)).saturating_sub(keep_rows(limit));
        if start < rows_needed {
            return false;
        }
        let bytes_needed = retained_bytes(result)
            .saturating_add(PAGE_BYTES)
            .saturating_sub(RETAINED_BYTES);
        let above: usize = result
            .rows
            .iter()
            .take(start)
            .flatten()
            .map(Value::bytes)
            .sum();
        above >= bytes_needed
    }
    pub(super) fn page_event(&mut self, event: Event) {
        let pages = &mut self.database.document.tools.pages;
        let Some(page) = pages
            .pending
            .as_mut()
            .filter(|p| p.result.current == Some(event.ticket))
        else {
            return;
        };
        if let EventKind::Finished(c) = &event.kind {
            self.database.transaction = c.transaction;
        }
        if let Some(error) = page_failure(&event.kind, self.database.document.table.as_ref()) {
            page.error = error;
        }
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        page.result.apply(event);
        if let Some(page) = pages.pending.take_if(|_| done) {
            self.accept_page(page);
        }
    }
    /// Join a delivered page to the rows it continues, unless the view moved
    /// on, drafts are staged, or the page failed.
    fn accept_page(&mut self, mut page: Pending) {
        if page.owner != self.database.document.result.current
            || self.has_drafts()
            || self.database.closing
        {
            return;
        }
        if !page.error.is_empty() || page.result.columns != self.database.document.result.columns {
            self.database.document.tools.pages.error = if page.error.is_empty() {
                "Column metadata changed; refresh the table".into()
            } else {
                page.error
            };
            return;
        }
        if page.reverse {
            page.result.rows.reverse();
        }
        if (page.reverse || page.tail)
            && let Some(browse) = &self.database.document.browse
        {
            page.offset = browse.offset.saturating_sub(page.result.rows.len());
        }
        let keep = keep_rows(self.limits().row_limit);
        self.database.document.append_page(page, keep);
        self.restore_browse_selection();
    }
}
impl Document {
    fn append_page(&mut self, page: Pending, keep: usize) {
        let Some(browse) = self.browse.as_mut() else {
            return;
        };
        let count = page.result.rows.len();
        if count < 100
            && !page.previous
            && page
                .result
                .completion
                .as_ref()
                .is_none_or(|c| c.page_has_more != Some(true))
        {
            self.tools.pages.end = Some(page.offset + count);
        }
        if count == 0 {
            return;
        }
        if page.previous {
            if page.offset + count != browse.offset {
                self.tools.pages.error = "Earlier page changed; refresh the table".into();
                return;
            }
            self.result.rows.splice(0..0, page.result.rows);
            self.tools.pages.chunks.push_front(count);
            browse.offset = page.offset;
            self.rebase_rows(count as isize);
        } else {
            if browse.offset + self.result.rows.len() != page.offset {
                return;
            }
            self.result.rows.extend(page.result.rows);
            self.tools.pages.chunks.push_back(count);
        }
        self.trim_pages(page.previous, keep);
    }
    /// Evict rows from the end away from the page just added, down to `keep`
    /// rows and the retained byte budget.
    fn trim_pages(&mut self, previous: bool, keep: usize) {
        let count = self.excess_rows(previous, keep);
        if count == 0 {
            return;
        }
        self.tools.pages.capacity_reached = true;
        self.tools.pages.forget(count, previous);
        if previous {
            let len = self.result.rows.len() - count;
            self.result.rows.truncate(len);
            self.inspect = self.inspect.filter(|(r, _)| *r < len);
            self.tools.detail = self.tools.detail.filter(|r| *r < len);
            self.tools.grid_selection = self
                .tools
                .grid_selection
                .filter(|s| s.anchor.0 < len && s.cursor.0 < len);
        } else {
            self.result.rows.drain(..count);
            if let Some(browse) = self.browse.as_mut() {
                browse.offset += count;
            }
            self.rebase_rows(-(count as isize));
        }
    }
    /// How many rows, counted from the far end, exceed `keep` rows or the byte budget.
    fn excess_rows(&self, previous: bool, keep: usize) -> usize {
        let rows = &self.result.rows;
        let row_excess = rows.len().saturating_sub(keep);
        let byte_excess = retained_bytes(&self.result).saturating_sub(RETAINED_BYTES);
        let (mut count, mut bytes) = (0, 0);
        while count < rows.len() && (count < row_excess || bytes < byte_excess) {
            let row = if previous {
                &rows[rows.len() - 1 - count]
            } else {
                &rows[count]
            };
            bytes += row.iter().map(Value::bytes).sum::<usize>();
            count += 1;
        }
        count
    }
    fn rebase_rows(&mut self, delta: isize) {
        self.inspect = self
            .inspect
            .and_then(|(r, c)| r.checked_add_signed(delta).map(|r| (r, c)));
        self.tools.detail = self.tools.detail.and_then(|r| r.checked_add_signed(delta));
        self.tools.grid_selection = self.tools.grid_selection.and_then(|mut s| {
            s.anchor.0 = s.anchor.0.checked_add_signed(delta)?;
            s.cursor.0 = s.cursor.0.checked_add_signed(delta)?;
            Some(s)
        });
        self.tools.grid_scroll.y = (self.tools.grid_scroll.y
            + delta as f32 * self.tools.row_stride.max(super::grid::ROW_HEIGHT))
        .max(0.0);
        self.tools.restore_scroll = Some(self.tools.grid_scroll);
    }
}
#[cfg(test)]
thread_local! { static BYTE_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn retained_bytes(result: &ResultSet) -> usize {
    #[cfg(test)]
    BYTE_SCANS.with(|count| count.set(count.get() + 1));
    result.rows.iter().flatten().map(|v| v.bytes()).sum()
}

#[cfg(test)]
mod tests {
    use super::{App, BYTE_SCANS, Delivery, Event, EventKind, Pending, ResultSet, Ticket};
    use eframe::egui;
    use tursobench_core::{Column, Completion, Failure, Table, TransactionState, Value};

    fn ticket(request: u64) -> Ticket {
        Ticket {
            database: 1,
            session: 1,
            request,
            generation: 0,
        }
    }
    fn pending(result: ResultSet, owner: Option<Ticket>, offset: usize) -> Pending {
        Pending {
            result,
            owner,
            offset,
            previous: false,
            reverse: false,
            tail: false,
            error: String::new(),
        }
    }
    fn completion(delivery: Delivery, error: Option<Failure>) -> Completion {
        Completion {
            delivery,
            error,
            ..Completion::new(TransactionState::Autocommit)
        }
    }

    #[test]
    fn repaints_do_not_scan_cells_when_paging_is_blocked_or_at_eof() {
        let mut app = App::new(&egui::Context::default());
        app.database.ready = true;
        let document = &mut app.database.document;
        document.view = super::View::Browse;
        document.browse = Some(super::super::first_page("t".into()));
        document.result.rows = vec![vec![Value::Integer(1)]; 1000];
        document.result.completion = Some(Completion {
            page_has_more: Some(true),
            ..completion(Delivery::Complete, None)
        });
        let scans = || BYTE_SCANS.with(|count| count.get());
        for blocked in 0..5 {
            let document = &mut app.database.document;
            document.tools.pages.end = (blocked == 0).then_some(1000);
            document.stale = blocked == 1;
            document.active = (blocked == 3).then_some(ticket(1));
            document.tools.pages.pending =
                (blocked == 4).then(|| pending(ResultSet::default(), None, 1000));
            app.database.closing = blocked == 2;
            BYTE_SCANS.with(|count| count.set(0));
            app.maybe_page(980, 1000, 0.0);
            assert_eq!(scans(), 0, "blocked case {blocked}");
        }
        app.database.document.tools.pages.pending = None;
        // An eligible fetch sizes its buffer; a viewport that cannot evict rows does not.
        app.maybe_page(0, 1000, 0.0);
        assert_eq!(scans(), 0);
        app.maybe_page(980, 1000, 0.0);
        assert_eq!(scans(), 1);
    }

    #[test]
    fn evicted_rows_use_the_rendered_stride_including_spacing() {
        let mut app = App::new(&egui::Context::default());
        let document = &mut app.database.document;
        document.tools.row_stride = 37.0;
        document.tools.grid_scroll.y = 4000.0;
        document.inspect = Some((120, 0));
        document.rebase_rows(-100);
        assert_eq!(document.tools.grid_scroll.y, 300.0);
        assert_eq!(document.inspect, Some((20, 0)));
        document.rebase_rows(100);
        assert_eq!(document.tools.grid_scroll.y, 4000.0);
    }

    #[test]
    fn rejected_pages_preserve_rows_owner_and_unknown_end() {
        let (owner, ticket) = (ticket(1), ticket(2));
        let finished = |delivery, error| EventKind::Finished(completion(delivery, error));
        let column = |name: &str, declared_type| Column {
            name: name.into(),
            declared_type,
        };
        let rows = |result, n| ResultSet {
            current: Some(result),
            columns: vec![column("id", None)],
            rows: vec![vec![Value::Integer(n)]],
            ..Default::default()
        };
        let changed = Table {
            name: "changed".into(),
            columns: vec![],
            is_table: true,
            has_rowid: true,
            binary_collations: true,
        };
        let mut app = App::new(&egui::Context::default());
        for (kind, expected) in [
            (finished(Delivery::Limited, None), "incomplete"),
            (finished(Delivery::Discarded, None), "incomplete"),
            (
                finished(Delivery::Complete, Some(Failure::input("x"))),
                "incomplete",
            ),
            (
                EventKind::Failed(Failure::input("engine failure")),
                "engine failure",
            ),
            (EventKind::Table(changed), "Schema changed"),
            (EventKind::Columns(vec![]), "Column metadata"),
            (
                EventKind::Columns(vec![column("renamed", None)]),
                "Column metadata",
            ),
            (
                EventKind::Columns(vec![column("id", Some("TEXT".into()))]),
                "Column metadata",
            ),
        ] {
            let document = &mut app.database.document;
            document.result = rows(owner, 7);
            document.tools.pages = Default::default();
            document.tools.pages.pending = Some(pending(rows(ticket, 8), Some(owner), 1));
            // A late completion cannot consume or alter the current pending page.
            let late = finished(Delivery::Complete, None);
            app.page_event(Event {
                ticket: owner,
                kind: late,
            });
            assert!(app.database.document.tools.pages.pending.is_some());
            app.page_event(Event { ticket, kind });
            let kind = finished(Delivery::Complete, None);
            app.page_event(Event { ticket, kind });
            let document = &app.database.document;
            let pages = &document.tools.pages;
            assert!(pages.pending.is_none());
            assert!(
                pages.error.contains(expected),
                "{expected}: {}",
                pages.error
            );
            assert_eq!(document.result.rows, vec![vec![Value::Integer(7)]]);
            assert_eq!((document.result.current, pages.end), (Some(owner), None));
        }
    }
}
