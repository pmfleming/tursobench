use super::{App, View, document::Document};
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ResultSet, Ticket};
use tursobench_db::{Operation, Request};

#[derive(Default)]
pub(super) struct Pages {
    pub pending: Option<Pending>,
    chunks: VecDeque<usize>,
    pub end: Option<usize>,
    pub error: String,
    pub restore_rows: usize,
    pub retry_previous: bool,
}
pub(super) struct Pending {
    pub result: ResultSet,
    owner: Option<Ticket>,
    offset: usize,
    previous: bool,
    error: String,
}
impl App {
    pub(super) fn request_page(&mut self, previous: bool) {
        if self.document.view != View::Browse
            || self.document.active.is_some()
            || self.document.tools.pages.pending.is_some()
            || self.has_drafts()
            || self.commit_ticket.is_some()
            || !self.ready
            || self.closing
        {
            return;
        }
        if !self
            .document
            .result
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery == Delivery::Complete && c.error.is_none())
        {
            return;
        }
        let Some(mut spec) = self.document.browse.clone() else {
            return;
        };
        let pages = &mut self.document.tools.pages;
        let end = spec.offset + self.document.result.rows.len();
        if previous {
            if spec.offset == 0 {
                return;
            }
            spec.offset = spec.offset.saturating_sub(100);
        } else {
            if pages.end.is_some_and(|last| end >= last) || end > 1_000_000 {
                return;
            }
            spec.offset = end;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        pages.retry_previous = previous;
        let offset = spec.offset;
        match session.submit(Request {
            ticket,
            operation: Operation::Browse(spec),
        }) {
            Ok(()) => {
                if pages.chunks.is_empty() && !self.document.result.rows.is_empty() {
                    pages.chunks.push_back(self.document.result.rows.len());
                }
                let mut result = ResultSet::default();
                result.start(ticket);
                pages.pending = Some(Pending {
                    result,
                    owner: self.document.result.current,
                    offset,
                    previous,
                    error: String::new(),
                });
                pages.error.clear();
            }
            Err(e) => pages.error = e.to_string(),
        }
    }
    pub(super) fn maybe_page(&mut self, start: usize, end: usize, delta: f32) {
        if !self.document.tools.pages.error.is_empty() {
            return;
        }
        if self.document.tools.pages.restore_rows > self.document.result.rows.len() {
            self.request_page(false);
        } else if start <= 20 && delta > 0.0 {
            self.request_page(true);
        } else if end + 20 >= self.document.result.rows.len() && delta <= 0.0 {
            self.request_page(false);
        }
    }
    pub(super) fn page_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        let Some(mut page) = self.document.tools.pages.pending.take() else {
            return;
        };
        if page.result.current != Some(event.ticket) {
            self.document.tools.pages.pending = Some(page);
            return;
        }
        match &event.kind {
            EventKind::Table(table) if self.document.table.as_ref() != Some(table) => {
                page.error = "Schema changed while loading; refresh the table".into()
            }
            EventKind::Failed(e) => page.error = e.to_string(),
            EventKind::Finished(c) => {
                self.transaction = c.transaction;
                if c.error.is_some() || c.delivery != Delivery::Complete {
                    page.error = "Page delivery incomplete; refresh or retry".into();
                }
            }
            _ => {}
        }
        page.result.apply(event);
        if !done {
            self.document.tools.pages.pending = Some(page);
            return;
        }
        if page.owner != self.document.result.current || self.has_drafts() || self.closing {
            return;
        }
        if !page.error.is_empty() || page.result.columns != self.document.result.columns {
            self.document.tools.pages.error = if page.error.is_empty() {
                "Column metadata changed; refresh the table".into()
            } else {
                page.error
            };
            return;
        }
        self.document
            .append_page(page, self.preferences.row_limit.clamp(200, 10_000) - 100);
        self.restore_browse_selection();
    }
}
impl Document {
    fn append_page(&mut self, page: Pending, keep: usize) {
        let Some(browse) = self.browse.as_mut() else {
            return;
        };
        let count = page.result.rows.len();
        if count < 100 && !page.previous {
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
    fn trim_pages(&mut self, previous: bool, keep: usize) {
        let Some(browse) = self.browse.as_mut() else {
            return;
        };
        let mut delta = 0isize;
        // Reserve one 100-row / 4 MiB page for in-flight delivery. The table
        // window has a 200-row minimum; SQL retains the user's exact row limit.
        while self.tools.pages.chunks.len() > 1
            && (self.result.rows.len() > keep || retained_bytes(&self.result) > 4 * 1024 * 1024)
        {
            if previous {
                let Some(n) = self.tools.pages.chunks.pop_back() else {
                    break;
                };
                self.result.rows.truncate(self.result.rows.len() - n);
                if self
                    .inspect
                    .is_some_and(|(r, _)| r >= self.result.rows.len())
                {
                    self.inspect = None;
                    self.tools.detail = None;
                    self.tools.grid_selection = None;
                }
            } else {
                let Some(n) = self.tools.pages.chunks.pop_front() else {
                    break;
                };
                self.result.rows.drain(..n);
                browse.offset += n;
                delta -= n as isize;
            }
        }
        if delta != 0 {
            self.rebase_rows(delta);
        }
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
fn retained_bytes(result: &ResultSet) -> usize {
    result.rows.iter().flatten().map(|v| v.bytes()).sum()
}

#[cfg(test)]
mod tests {
    use super::{App, Delivery, Event, EventKind, Pending, ResultSet, Ticket};
    use eframe::egui;
    use tursobench_core::{Column, Completion, Failure, Table, TransactionState, Value};

    #[test]
    fn evicted_rows_use_the_rendered_stride_including_spacing() {
        let mut app = App::new(&egui::Context::default());
        app.document.tools.row_stride = 37.0;
        app.document.tools.grid_scroll.y = 4000.0;
        app.document.inspect = Some((120, 0));
        app.document.rebase_rows(-100);
        assert_eq!(app.document.tools.grid_scroll.y, 300.0);
        assert_eq!(app.document.inspect, Some((20, 0)));
        app.document.rebase_rows(100);
        assert_eq!(app.document.tools.grid_scroll.y, 4000.0);
    }

    #[test]
    fn rejected_pages_preserve_rows_owner_and_unknown_end() {
        let ticket = Ticket {
            database: 1,
            session: 1,
            request: 2,
            generation: 0,
        };
        let owner = Ticket {
            request: 1,
            ..ticket
        };
        let completion = |delivery, error| {
            EventKind::Finished(Completion {
                elapsed_ms: 1,
                delivery,
                rows_seen: 100,
                changed: 0,
                transaction: TransactionState::Autocommit,
                error,
            })
        };
        let column = |name: &str, declared_type| Column {
            name: name.into(),
            declared_type,
        };
        let mut app = App::new(&egui::Context::default());
        for (kind, expected) in [
            (completion(Delivery::Limited, None), "incomplete"),
            (completion(Delivery::Discarded, None), "incomplete"),
            (
                completion(Delivery::Complete, Some(Failure::input("failed"))),
                "incomplete",
            ),
            (
                EventKind::Failed(Failure::input("engine failure")),
                "engine failure",
            ),
            (
                EventKind::Table(Table {
                    name: "changed".into(),
                    columns: vec![],
                    is_table: true,
                    has_rowid: true,
                }),
                "Schema changed",
            ),
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
            app.document.result = ResultSet {
                current: Some(owner),
                columns: vec![column("id", None)],
                rows: vec![vec![Value::Integer(7)]],
                ..Default::default()
            };
            app.document.tools.pages = Default::default();
            app.document.tools.pages.pending = Some(Pending {
                result: ResultSet {
                    current: Some(ticket),
                    columns: vec![column("id", None)],
                    rows: vec![vec![Value::Integer(8)]],
                    ..Default::default()
                },
                owner: Some(owner),
                offset: 1,
                previous: false,
                error: String::new(),
            });
            // A late completion cannot consume or alter the current pending page.
            app.page_event(Event {
                ticket: owner,
                kind: completion(Delivery::Complete, None),
            });
            assert!(app.document.tools.pages.pending.is_some());
            app.page_event(Event { ticket, kind });
            app.page_event(Event {
                ticket,
                kind: completion(Delivery::Complete, None),
            });
            assert!(app.document.tools.pages.pending.is_none());
            assert!(
                app.document.tools.pages.error.contains(expected),
                "{expected}: {}",
                app.document.tools.pages.error
            );
            assert_eq!(app.document.result.rows, vec![vec![Value::Integer(7)]]);
            assert_eq!(app.document.result.current, Some(owner));
            assert_eq!(app.document.tools.pages.end, None);
        }
    }
}
