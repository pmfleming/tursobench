use super::{
    App, View,
    icons::{self, Icon},
};
use eframe::egui;
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ForeignKey, ResultSet, Ticket, Value};
use tursobench_db::{Operation, Request};

#[derive(Default)]
pub(super) struct ContextReads {
    pub pending: Option<Read>,
    cache: VecDeque<Cached>,
    cache_bytes: usize,
    pub hovered: Option<(usize, ForeignKey)>,
    hover: Option<(String, f64)>,
    relation: Option<(String, i64)>,
}

pub(super) struct Read {
    pub result: ResultSet,
    owner: Option<Ticket>,
    key: String,
    error: String,
}
struct Cached {
    bytes: usize,
    key: String,
    result: ResultSet,
    error: String,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum ReadKind {
    Parent,
    Rows,
    Count,
}

struct Relationship<'a> {
    id: String,
    child: &'a str,
    key: &'a ForeignKey,
    values: Vec<&'a Value>,
    kind: ReadKind,
}
impl Relationship<'_> {
    fn into_operation(self) -> (String, Operation) {
        let table = self.child.to_owned();
        let key = self.key.clone();
        let values = self.values.into_iter().cloned().collect();
        let operation = match self.kind {
            ReadKind::Parent => Operation::Peek { table, key, values },
            ReadKind::Rows => Operation::References { table, key, values },
            ReadKind::Count => Operation::ReferenceCount { table, key, values },
        };
        (self.id, operation)
    }
}
impl Cached {
    fn new(key: String, result: ResultSet, error: String) -> Self {
        let bytes = key.len()
            + error.len()
            + result
                .columns
                .iter()
                .map(|c| c.name.len() + c.declared_type.as_ref().map_or(0, String::len))
                .sum::<usize>()
            + result
                .rows
                .iter()
                .flatten()
                .map(Value::bytes)
                .sum::<usize>();
        Self {
            key,
            result,
            error,
            bytes,
        }
    }
}
impl ContextReads {
    fn cached(&self, key: &str) -> Option<&Cached> {
        self.cache.iter().find(|c| c.key == key)
    }
    fn remember(&mut self, cached: Cached) {
        self.cache_bytes += cached.bytes;
        self.cache.push_back(cached);
        while self.cache.len() > 32 || self.cache_bytes > 8 * 1024 * 1024 {
            if let Some(old) = self.cache.pop_front() {
                self.cache_bytes -= old.bytes;
            }
        }
    }
    fn forget(&mut self, key: &str) {
        self.cache.retain(|c| {
            if c.key != key {
                return true;
            }
            self.cache_bytes -= c.bytes;
            false
        });
    }
}
impl App {
    fn relationship_read<'a>(
        &'a self,
        row: usize,
        child: &'a str,
        key: &'a ForeignKey,
        kind: ReadKind,
    ) -> Option<Relationship<'a>> {
        if self.database.document.view != View::Browse {
            return None;
        }
        let table = self.database.document.table.as_ref()?;
        let row = self.database.document.result.rows.get(row)?;
        let outgoing = matches!(kind, ReadKind::Parent);
        if outgoing && table.name != child
            || !outgoing && !table.name.eq_ignore_ascii_case(&key.target)
        {
            return None;
        }
        let values: Vec<_> = key
            .columns
            .iter()
            .map(|(source, target)| {
                let name = if outgoing {
                    source.as_str()
                } else {
                    target.as_deref()?
                };
                let i = table
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(name))?;
                row.get(i).filter(|v| **v != Value::Null)
            })
            .collect::<Option<_>>()?;
        if values.is_empty() || values.iter().map(|v| v.bytes()).sum::<usize>() > 32 * 1024 {
            return None;
        }
        let id = format!("{kind:?}:{child}:{key:?}:{values:?}");
        if id.len() > 128 * 1024 {
            return None;
        }
        Some(Relationship {
            id,
            child,
            key,
            values,
            kind,
        })
    }
    pub(super) fn request_context(
        &mut self,
        row: usize,
        child: &str,
        key: &ForeignKey,
        kind: ReadKind,
    ) {
        if !self.database.ready
            || self.database.closing
            || self.database.document.active.is_some()
            || self.database.document.tools.context.pending.is_some()
            || self.database.document.tools.pages.pending.is_some()
            || self.database.commit_ticket.is_some()
            || self.has_drafts()
        {
            return;
        }
        let Some(plan) = self.relationship_read(row, child, key, kind) else {
            return;
        };
        if self
            .database
            .document
            .tools
            .context
            .cached(&plan.id)
            .is_some()
        {
            return;
        }
        let Some(session) = &self.database.session else {
            return;
        };
        let (id, operation) = plan.into_operation();
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        if session.submit(Request { ticket, operation }).is_ok() {
            let mut result = ResultSet::default();
            result.start(ticket);
            self.database.document.tools.context.pending = Some(Read {
                result,
                owner: self.database.document.result.current,
                key: id,
                error: String::new(),
            });
        }
    }
    pub(super) fn context_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Failed(_) | EventKind::Finished(_));
        let Some(mut read) = self.database.document.tools.context.pending.take() else {
            return;
        };
        match &event.kind {
            EventKind::Failed(e) => read.error = e.to_string(),
            EventKind::Finished(c) => {
                self.database.transaction = c.transaction;
                if let Some(e) = &c.error {
                    read.error = e.to_string();
                }
            }
            _ => {}
        }
        read.result.apply(event);
        if !done {
            self.database.document.tools.context.pending = Some(read);
            return;
        }
        if read.owner != self.database.document.result.current || self.database.closing {
            return;
        }
        self.database.document.tools.context.remember(Cached::new(
            read.key,
            read.result,
            read.error,
        ));
    }
    pub(super) fn hover_context(&mut self, ctx: &egui::Context) {
        let Some((row, key)) = &self.database.document.tools.context.hovered else {
            self.database.document.tools.context.hover = None;
            return;
        };
        let Some(table) = self.database.document.table.as_ref() else {
            return;
        };
        let Some(id) = self
            .relationship_read(*row, &table.name, key, ReadKind::Parent)
            .map(|plan| plan.id)
        else {
            return;
        };
        let cached = self.database.document.tools.context.cached(&id).is_some();
        let now = ctx.input(|i| i.time);
        let since = match &self.database.document.tools.context.hover {
            Some((old, since)) if old == &id => *since,
            _ => {
                self.database.document.tools.context.hover = Some((id, now));
                now
            }
        };
        if cached {
            return;
        }
        if now - since >= 0.35 {
            let table = table.name.clone();
            if let Some((row, key)) = self.database.document.tools.context.hovered.take() {
                self.request_context(row, &table, &key, ReadKind::Parent);
                self.database.document.tools.context.hovered = Some((row, key));
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(350));
        }
    }
    pub(super) fn peek_view(&self, ui: &mut egui::Ui, row: usize, key: &ForeignKey) {
        ui.strong(format!("Parent · {}", key.target));
        let Some(table) = &self.database.document.table else {
            return;
        };
        let Some(id) = self
            .relationship_read(row, &table.name, key, ReadKind::Parent)
            .map(|plan| plan.id)
        else {
            ui.label("Unresolved or NULL key");
            return;
        };
        if let Some(c) = self.database.document.tools.context.cached(&id) {
            if !c.error.is_empty() {
                ui.label(&c.error);
                return;
            }
            if let Some(row) = c.result.rows.first() {
                for (column, value) in c.result.columns.iter().zip(row).take(12) {
                    ui.label(format!("{}  {}", column.name, value.preview()));
                }
            } else {
                ui.label("No retained parent row");
            }
            if c.result
                .completion
                .as_ref()
                .is_some_and(|c| c.delivery != Delivery::Complete)
            {
                ui.small("Preview limited by retention budget");
            }
        } else {
            ui.label("Loading parent preview…");
        }
    }
    fn incoming_keys(&self) -> Vec<(String, ForeignKey)> {
        let Some(table) = &self.database.document.table else {
            return vec![];
        };
        self.database
            .ui
            .diagram
            .data
            .iter()
            .flat_map(|g| &g.tables)
            .flat_map(|n| {
                n.inspection
                    .iter()
                    .flat_map(|i| &i.foreign_keys)
                    .filter(|k| k.target.eq_ignore_ascii_case(&table.name))
                    .map(|k| (n.name.clone(), k.clone()))
            })
            .collect()
    }
    pub(super) fn related_visible(&self) -> bool {
        self.database.document.view == View::Browse
            && (self.schema_incomplete() || !self.incoming_keys().is_empty())
    }
    pub(super) fn schema_incomplete(&self) -> bool {
        self.database.ui.diagram.stale
            || self
                .database
                .ui
                .diagram
                .data
                .as_ref()
                .is_none_or(|g| g.limited || g.tables.iter().any(|n| n.error.is_some()))
    }
    pub(super) fn related_list(&mut self, ui: &mut egui::Ui, row: usize) {
        if !self.related_visible() {
            return;
        }
        let keys = self.incoming_keys();
        let available = self.database.ready
            && !self.database.closing
            && !self.has_drafts()
            && self.database.document.active.is_none()
            && self.database.commit_ticket.is_none()
            && self.database.document.tools.context.pending.is_none();
        egui::ScrollArea::horizontal()
            .id_salt("related-line")
            .max_height(24.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.weak("Related");
                    if self.schema_incomplete() {
                        ui.colored_label(ui.visuals().warn_fg_color, "Incomplete schema")
                            .on_hover_text(
                                "Cached or incomplete schema — references may be missing",
                            );
                    }
                    for (child, key) in keys {
                        let spec = self
                            .relationship_read(row, &child, &key, ReadKind::Count)
                            .map(|plan| plan.id);
                        let cached = spec
                            .as_ref()
                            .and_then(|id| self.database.document.tools.context.cached(id));
                        let count = cached
                            .and_then(|c| {
                                c.result
                                    .completion
                                    .as_ref()
                                    .filter(|done| {
                                        done.delivery == Delivery::Complete && done.error.is_none()
                                    })
                                    .and_then(|_| c.result.rows.first()?.first())
                            })
                            .and_then(|v| {
                                if let Value::Integer(n) = v {
                                    Some(*n)
                                } else {
                                    None
                                }
                            });
                        let error = cached.map(|c| c.error.clone()).unwrap_or_default();
                        if ui
                            .add_enabled(
                                available && spec.is_some(),
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "↙ {child} · {}",
                                        count.map_or("—".into(), |n| n.to_string())
                                    ))
                                    .color(crate::appearance::Tokens::of(ui.ctx()).relationship),
                                )
                                .frame(false),
                            )
                            .on_hover_text(format!(
                                "Related {child} · FK {} · {}",
                                key.id,
                                key.columns
                                    .iter()
                                    .map(|(from, to)| format!(
                                        "{from} → {}",
                                        to.as_deref().unwrap_or("unresolved PK")
                                    ))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                            .clicked()
                        {
                            self.follow_incoming(row, child.clone(), key.clone());
                        }
                        if count.is_none()
                            && ui
                                .add_enabled_ui(available && spec.is_some(), |ui| {
                                    icons::button(
                                        ui,
                                        Icon::Count,
                                        &format!("Count related {child} (FK {})", key.id),
                                        "",
                                    )
                                })
                                .inner
                                .on_hover_text(if error.is_empty() {
                                    "Count matching rows (full scan, may be slow)".to_owned()
                                } else {
                                    format!("{error} · click to retry count")
                                })
                                .clicked()
                        {
                            if let Some(id) = &spec {
                                self.database.document.tools.context.forget(id);
                            }
                            self.request_context(row, &child, &key, ReadKind::Count);
                        }
                    }
                });
            });
    }
    pub(super) fn wide_related(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.database.document.inspect.map(|(r, _)| r).or(self
            .database
            .document
            .tools
            .detail)
        else {
            return;
        };
        let keys = self.incoming_keys();
        if keys.is_empty() {
            return;
        }
        let chosen = &self.database.document.tools.context.relation;
        let mut selected = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Related rows");
            for (child, key) in &keys {
                if ui
                    .selectable_label(
                        chosen
                            .as_ref()
                            .is_some_and(|(name, id)| name == child && *id == key.id),
                        format!("{child} · FK {}", key.id),
                    )
                    .clicked()
                {
                    selected = Some((child.clone(), key.id));
                }
            }
        });
        if let Some(selected) = selected {
            self.database.document.tools.context.relation = Some(selected);
        }
        let (child, key) = keys
            .iter()
            .find(|(child, key)| {
                self.database
                    .document
                    .tools
                    .context
                    .relation
                    .as_ref()
                    .is_some_and(|(name, id)| name == child && *id == key.id)
            })
            .unwrap_or(&keys[0]);
        self.database.document.tools.context.relation = Some((child.clone(), key.id));
        if ui.link("Open related rows in full view").clicked() {
            self.follow_incoming(row, child.clone(), key.clone());
            return;
        }
        self.request_context(row, child, key, ReadKind::Rows);
        let Some(id) = self
            .relationship_read(row, child, key, ReadKind::Rows)
            .map(|plan| plan.id)
        else {
            ui.weak("Select a resolved non-NULL parent key");
            return;
        };
        let Some(c) = self.database.document.tools.context.cached(&id) else {
            ui.spinner();
            ui.weak("Loading related rows…");
            return;
        };
        if !c.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &c.error);
            if ui.button("Retry related rows").clicked() {
                self.database.document.tools.context.forget(&id);
                self.request_context(row, child, key, ReadKind::Rows);
            }
            return;
        }
        ui.small(format!(
            "{} retained related rows · lookup capped at 100",
            c.result.rows.len()
        ));
        if c.result
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery != Delivery::Complete)
        {
            ui.weak("Related result limited by retention budget");
        }
        egui::ScrollArea::both().id_salt("related-grid").show_rows(
            ui,
            22.0,
            c.result.rows.len() + 1,
            |ui, range| {
                for r in range {
                    ui.horizontal(|ui| {
                        if r == 0 {
                            for col in c.result.columns.iter().take(12) {
                                ui.add_sized(
                                    [140.0, 22.0],
                                    egui::Label::new(egui::RichText::new(&col.name).strong())
                                        .truncate(),
                                );
                            }
                        } else {
                            for v in c.result.rows[r - 1].iter().take(12) {
                                ui.add_sized(
                                    [140.0, 22.0],
                                    egui::Label::new(v.preview()).truncate(),
                                )
                                .on_hover_text(crate::export::cell_text(v));
                            }
                        }
                    });
                }
            },
        );
        if c.result.columns.len() > 12 {
            ui.weak("Showing first 12 columns; open full view for all columns");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cached, ContextReads};
    use tursobench_core::{ResultSet, Value};

    #[test]
    fn cache_enforces_fifo_count_and_byte_limits_and_accounts_for_retries() {
        let mut reads = ContextReads::default();
        for i in 0..33 {
            reads.remember(Cached::new(
                i.to_string(),
                ResultSet::default(),
                String::new(),
            ));
        }
        assert_eq!(reads.cache.len(), 32);
        assert!(reads.cached("0").is_none());
        assert!(reads.cached("32").is_some());
        let bytes = reads.cache_bytes;
        reads.forget("13");
        reads.forget("absent");
        assert_eq!(reads.cache_bytes, bytes - 2);
        assert_eq!(reads.cache.len(), 31);

        let mut reads = ContextReads::default();
        for i in 0..3 {
            let result = ResultSet {
                rows: vec![vec![Value::Blob(vec![0; 3 * 1024 * 1024])]],
                ..Default::default()
            };
            reads.remember(Cached::new(i.to_string(), result, String::new()));
        }
        assert_eq!(reads.cache.len(), 2);
        assert!(reads.cached("0").is_none());
        assert!(reads.cached("1").is_some());
        assert!(reads.cache_bytes <= 8 * 1024 * 1024);
        reads.remember(Cached::new(
            "oversized".into(),
            ResultSet::default(),
            "x".repeat(8 * 1024 * 1024 + 1),
        ));
        assert!(reads.cache.is_empty());
        assert_eq!(reads.cache_bytes, 0);
    }
}
