use super::{App, View};
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
        if self.view != View::Browse
            || self.active.is_some()
            || self.tools.pages.pending.is_some()
            || self.has_drafts()
            || self.commit_ticket.is_some()
            || !self.ready
            || self.closing
        {
            return;
        }
        if !self
            .result
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery == Delivery::Complete && c.error.is_none())
        {
            return;
        }
        let Some(mut spec) = self.browse.clone() else {
            return;
        };
        let end = spec.offset + self.result.rows.len();
        if previous {
            if spec.offset == 0 {
                return;
            }
            spec.offset = spec.offset.saturating_sub(100);
        } else {
            if self.tools.pages.end.is_some_and(|last| end >= last) || end > 1_000_000 {
                return;
            }
            spec.offset = end;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Browse(spec.clone()),
        }) {
            Ok(()) => {
                if self.tools.pages.chunks.is_empty() && !self.result.rows.is_empty() {
                    self.tools.pages.chunks.push_back(self.result.rows.len());
                }
                let mut result = ResultSet::default();
                result.start(ticket);
                self.tools.pages.pending = Some(Pending {
                    result,
                    owner: self.result.current,
                    offset: spec.offset,
                    previous,
                    error: String::new(),
                });
                self.tools.pages.error.clear();
            }
            Err(e) => self.tools.pages.error = e.to_string(),
        }
    }
    pub(super) fn maybe_page(&mut self, start: usize, end: usize, delta: f32) {
        if !self.tools.pages.error.is_empty() {
            return;
        }
        if self.tools.pages.restore_rows > self.result.rows.len() {
            self.request_page(false);
        } else if start <= 20 && delta > 0.0 {
            self.request_page(true);
        } else if end + 20 >= self.result.rows.len() && delta <= 0.0 {
            self.request_page(false);
        }
    }
    pub(super) fn page_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        let Some(page) = &mut self.tools.pages.pending else {
            return;
        };
        match &event.kind {
            EventKind::Table(table) if self.table.as_ref() != Some(table) => {
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
            return;
        }
        let page = self.tools.pages.pending.take().unwrap();
        if page.owner != self.result.current || self.has_drafts() || self.closing {
            return;
        }
        let columns_match = page.result.columns.len() == self.result.columns.len()
            && page
                .result
                .columns
                .iter()
                .zip(&self.result.columns)
                .all(|(a, b)| a.name == b.name && a.declared_type == b.declared_type);
        if !page.error.is_empty() || !columns_match {
            self.tools.pages.error = if page.error.is_empty() {
                "Column metadata changed; refresh the table".into()
            } else {
                page.error
            };
            return;
        }
        let count = page.result.rows.len();
        if count < 100 && !page.previous {
            self.tools.pages.end = Some(page.offset + count);
        }
        if count == 0 {
            return;
        }
        if page.previous {
            if self
                .browse
                .as_ref()
                .is_none_or(|b| page.offset + count != b.offset)
            {
                self.tools.pages.error = "Earlier page changed; refresh the table".into();
                return;
            }
            self.result.rows.splice(0..0, page.result.rows);
            self.tools.pages.chunks.push_front(count);
            self.browse.as_mut().unwrap().offset = page.offset;
            self.rebase_rows(count as isize);
        } else {
            if self
                .browse
                .as_ref()
                .is_none_or(|b| b.offset + self.result.rows.len() != page.offset)
            {
                return;
            }
            self.result.rows.extend(page.result.rows);
            self.tools.pages.chunks.push_back(count);
        }
        // Reserve one 100-row / 4 MiB page for in-flight delivery. The table
        // window has a 200-row minimum; SQL retains the user's exact row limit.
        let keep = self.preferences.row_limit.clamp(200, 10_000) - 100;
        while self.tools.pages.chunks.len() > 1
            && (self.result.rows.len() > keep || retained_bytes(&self.result) > 4 * 1024 * 1024)
        {
            if page.previous {
                let n = self.tools.pages.chunks.pop_back().unwrap();
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
                let n = self.tools.pages.chunks.pop_front().unwrap();
                self.result.rows.drain(..n);
                self.browse.as_mut().unwrap().offset += n;
                self.rebase_rows(-(n as isize));
            }
        }
        self.restore_browse_selection();
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
        self.tools.grid_scroll.y = (self.tools.grid_scroll.y + delta as f32 * 25.0).max(0.0);
        self.tools.restore_scroll = Some(self.tools.grid_scroll);
    }
}
fn retained_bytes(result: &ResultSet) -> usize {
    result.rows.iter().flatten().map(|v| v.bytes()).sum()
}
