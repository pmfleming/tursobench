use super::{App, View, document::Document};
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ResultSet, Ticket};
use tursobench_db::{Operation, Request};

pub(super) const PAGE_BYTES: usize = 512 * 1024;
pub(super) fn fetch_rows(limit: usize) -> usize {
    ((limit.clamp(200, 10000) - 100) / 3).clamp(1, 100)
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
impl App {
    pub(super) fn request_page(&mut self, previous: bool) {
        if self.database.document.stale
            || self.database.document.view != View::Browse
            || self.database.document.active.is_some()
            || self.database.document.tools.pages.pending.is_some()
            || !self.requests_enabled()
        {
            return;
        }
        if !self
            .database
            .document
            .result
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery == Delivery::Complete && c.error.is_none())
        {
            return;
        }
        let Some(mut spec) = self.database.document.browse.clone() else {
            return;
        };
        let fetch_rows = fetch_rows(self.preferences.row_limit);
        let pages = &mut self.database.document.tools.pages;
        let end = spec.offset + self.database.document.result.rows.len();
        if previous {
            if spec.offset == 0 {
                return;
            }
            spec.offset = spec.offset.saturating_sub(fetch_rows);
        } else {
            if pages.end.is_some_and(|last| end >= last) {
                return;
            }
            spec.offset = end;
        }
        let seek = self.database.document.table.as_ref().is_some_and(|table| {
            let keys = table.keys();
            table.is_table
                && !keys.is_empty()
                && keys.len() <= 8
                && keys.iter().all(|i| table.columns[*i].not_null)
        });
        let cursor = if previous {
            self.database.document.result.rows.first()
        } else {
            self.database.document.result.rows.last()
        };
        let seek = seek
            && cursor.is_some_and(|row| {
                !row.iter()
                    .any(|v| matches!(v, tursobench_core::Value::Deferred(_)))
            });
        spec.options.tail = previous && !seek;
        spec.options.page_rows = if spec.options.tail {
            self.database
                .document
                .browse
                .as_ref()
                .unwrap()
                .offset
                .min(fetch_rows)
        } else {
            fetch_rows
        };
        spec.options.page_bytes = PAGE_BYTES;
        let tail = spec.options.tail;
        spec.options.cursor = if seek {
            if previous {
                self.database.document.result.rows.first().cloned()
            } else {
                self.database.document.result.rows.last().cloned()
            }
        } else {
            None
        };
        spec.options.reverse = previous && seek;
        let reverse = spec.options.reverse;
        let Some(session) = &self.database.session else {
            return;
        };
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        pages.retry_previous = previous;
        let offset = spec.offset;
        match session.submit(Request {
            ticket,
            operation: Operation::Browse(spec),
        }) {
            Ok(()) => {
                if pages.chunks.is_empty() && !self.database.document.result.rows.is_empty() {
                    pages
                        .chunks
                        .push_back(self.database.document.result.rows.len());
                }
                let mut result = ResultSet::default();
                result.start(ticket);
                pages.pending = Some(Pending {
                    result,
                    owner: self.database.document.result.current,
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
        if !self.database.document.tools.pages.error.is_empty() {
            return;
        }
        if self.database.document.tools.pages.restore_rows
            > self.database.document.result.rows.len()
        {
            self.request_page(false);
        } else if start <= 20 && delta > 0.0 {
            self.request_page(true);
        } else if end + 20 >= self.database.document.result.rows.len() && delta <= 0.0 {
            let keep = self.preferences.row_limit.clamp(200, 10000) - 100;
            let rows_needed = (self.database.document.result.rows.len()
                + fetch_rows(self.preferences.row_limit))
            .saturating_sub(keep);
            let bytes_needed = retained_bytes(&self.database.document.result)
                .saturating_add(PAGE_BYTES)
                .saturating_sub(4 * 1024 * 1024);
            let before = self
                .database
                .document
                .result
                .rows
                .iter()
                .take(start)
                .flatten()
                .map(tursobench_core::Value::bytes)
                .sum::<usize>();
            if start >= rows_needed && before >= bytes_needed {
                self.request_page(false);
            }
        }
    }
    pub(super) fn page_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        let Some(mut page) = self.database.document.tools.pages.pending.take() else {
            return;
        };
        if page.result.current != Some(event.ticket) {
            self.database.document.tools.pages.pending = Some(page);
            return;
        }
        match &event.kind {
            EventKind::Table(table) if self.database.document.table.as_ref() != Some(table) => {
                page.error = "Schema changed while loading; refresh the table".into()
            }
            EventKind::Failed(e) => page.error = e.to_string(),
            EventKind::Finished(c) => {
                self.database.transaction = c.transaction;
                if c.error.is_some() || c.delivery != Delivery::Complete {
                    page.error = "Page delivery incomplete; refresh or retry".into();
                }
            }
            _ => {}
        }
        page.result.apply(event);
        if !done {
            self.database.document.tools.pages.pending = Some(page);
            return;
        }
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
        self.database
            .document
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
    fn trim_pages(&mut self, previous: bool, keep: usize) {
        let row_excess = self.result.rows.len().saturating_sub(keep);
        let byte_excess = retained_bytes(&self.result).saturating_sub(4 * 1024 * 1024);
        let mut count = 0;
        let mut bytes = 0;
        for i in 0..self.result.rows.len() {
            if count >= row_excess && bytes >= byte_excess {
                break;
            }
            let row = if previous {
                self.result.rows.len() - 1 - i
            } else {
                i
            };
            bytes += self.result.rows[row]
                .iter()
                .map(tursobench_core::Value::bytes)
                .sum::<usize>();
            count += 1;
        }
        if count == 0 {
            return;
        }
        self.tools.pages.capacity_reached = true;
        let mut remaining = count;
        while remaining > 0 {
            let chunk = if previous {
                self.tools.pages.chunks.back_mut()
            } else {
                self.tools.pages.chunks.front_mut()
            };
            let Some(chunk) = chunk else {
                break;
            };
            let n = remaining.min(*chunk);
            *chunk -= n;
            remaining -= n;
            if *chunk == 0 {
                if previous {
                    self.tools.pages.chunks.pop_back();
                } else {
                    self.tools.pages.chunks.pop_front();
                }
            }
        }
        if previous {
            self.result.rows.truncate(self.result.rows.len() - count);
            let len = self.result.rows.len();
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
        app.database.document.tools.row_stride = 37.0;
        app.database.document.tools.grid_scroll.y = 4000.0;
        app.database.document.inspect = Some((120, 0));
        app.database.document.rebase_rows(-100);
        assert_eq!(app.database.document.tools.grid_scroll.y, 300.0);
        assert_eq!(app.database.document.inspect, Some((20, 0)));
        app.database.document.rebase_rows(100);
        assert_eq!(app.database.document.tools.grid_scroll.y, 4000.0);
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
                page_has_more: None,
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
                    binary_collations: true,
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
            app.database.document.result = ResultSet {
                current: Some(owner),
                columns: vec![column("id", None)],
                rows: vec![vec![Value::Integer(7)]],
                ..Default::default()
            };
            app.database.document.tools.pages = Default::default();
            app.database.document.tools.pages.pending = Some(Pending {
                result: ResultSet {
                    current: Some(ticket),
                    columns: vec![column("id", None)],
                    rows: vec![vec![Value::Integer(8)]],
                    ..Default::default()
                },
                owner: Some(owner),
                offset: 1,
                previous: false,
                reverse: false,
                tail: false,
                error: String::new(),
            });
            // A late completion cannot consume or alter the current pending page.
            app.page_event(Event {
                ticket: owner,
                kind: completion(Delivery::Complete, None),
            });
            assert!(app.database.document.tools.pages.pending.is_some());
            app.page_event(Event { ticket, kind });
            app.page_event(Event {
                ticket,
                kind: completion(Delivery::Complete, None),
            });
            assert!(app.database.document.tools.pages.pending.is_none());
            assert!(
                app.database.document.tools.pages.error.contains(expected),
                "{expected}: {}",
                app.database.document.tools.pages.error
            );
            assert_eq!(
                app.database.document.result.rows,
                vec![vec![Value::Integer(7)]]
            );
            assert_eq!(app.database.document.result.current, Some(owner));
            assert_eq!(app.database.document.tools.pages.end, None);
        }
    }
}
