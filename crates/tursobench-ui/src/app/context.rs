use super::{App, View};
use eframe::egui;
use std::collections::VecDeque;
use tursobench_core::{Delivery, Event, EventKind, ForeignKey, ResultSet, Ticket, Value};
use tursobench_db::{Operation, Request};

#[derive(Default)]
pub(super) struct ContextReads {
    pub pending: Option<Read>,
    cache: VecDeque<Cached>,
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

impl App {
    fn relationship_read(
        &self,
        row: usize,
        child: &str,
        key: &ForeignKey,
        kind: ReadKind,
    ) -> Option<(String, Operation)> {
        if self.document.view != View::Browse {
            return None;
        }
        let table = self.document.table.as_ref()?;
        let row = self.document.result.rows.get(row)?;
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
                row.get(i).filter(|v| **v != Value::Null).cloned()
            })
            .collect::<Option<_>>()?;
        if values.is_empty() || values.iter().map(Value::bytes).sum::<usize>() > 32 * 1024 {
            return None;
        }
        let id = format!("{kind:?}:{child}:{key:?}:{values:?}");
        if id.len() > 128 * 1024 {
            return None;
        }
        let key = key.clone();
        let table = child.to_owned();
        let operation = match kind {
            ReadKind::Parent => Operation::Peek { table, key, values },
            ReadKind::Rows => Operation::References { table, key, values },
            ReadKind::Count => Operation::ReferenceCount { table, key, values },
        };
        Some((id, operation))
    }
    pub(super) fn request_context(
        &mut self,
        row: usize,
        child: &str,
        key: &ForeignKey,
        kind: ReadKind,
    ) {
        if !self.ready
            || self.closing
            || self.document.active.is_some()
            || self.document.tools.context.pending.is_some()
            || self.document.tools.pages.pending.is_some()
            || self.commit_ticket.is_some()
            || self.has_drafts()
        {
            return;
        }
        let Some((id, operation)) = self.relationship_read(row, child, key, kind) else {
            return;
        };
        if self
            .document
            .tools
            .context
            .cache
            .iter()
            .any(|c| c.key == id)
        {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        if session.submit(Request { ticket, operation }).is_ok() {
            let mut result = ResultSet::default();
            result.start(ticket);
            self.document.tools.context.pending = Some(Read {
                result,
                owner: self.document.result.current,
                key: id,
                error: String::new(),
            });
        }
    }
    pub(super) fn context_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Failed(_) | EventKind::Finished(_));
        let Some(read) = &mut self.document.tools.context.pending else {
            return;
        };
        match &event.kind {
            EventKind::Failed(e) => read.error = e.to_string(),
            EventKind::Finished(c) => {
                self.transaction = c.transaction;
                if let Some(e) = &c.error {
                    read.error = e.to_string();
                }
            }
            _ => {}
        }
        read.result.apply(event);
        if !done {
            return;
        }
        let read = self.document.tools.context.pending.take().unwrap();
        if read.owner != self.document.result.current || self.closing {
            return;
        }
        self.document.tools.context.cache.push_back(Cached {
            key: read.key,
            result: read.result,
            error: read.error,
        });
        while self.document.tools.context.cache.len() > 32
            || self
                .document
                .tools
                .context
                .cache
                .iter()
                .map(|c| {
                    c.key.len()
                        + c.error.len()
                        + c.result
                            .columns
                            .iter()
                            .map(|c| c.name.len() + c.declared_type.as_ref().map_or(0, String::len))
                            .sum::<usize>()
                        + c.result
                            .rows
                            .iter()
                            .flatten()
                            .map(Value::bytes)
                            .sum::<usize>()
                })
                .sum::<usize>()
                > 8 * 1024 * 1024
        {
            self.document.tools.context.cache.pop_front();
        }
    }
    pub(super) fn hover_context(&mut self, ctx: &egui::Context) {
        let Some((row, key)) = self.document.tools.context.hovered.clone() else {
            self.document.tools.context.hover = None;
            return;
        };
        let Some(table) = self.document.table.as_ref().map(|t| t.name.clone()) else {
            return;
        };
        let Some((id, _)) = self.relationship_read(row, &table, &key, ReadKind::Parent) else {
            return;
        };
        let now = ctx.input(|i| i.time);
        let since = match &self.document.tools.context.hover {
            Some((old, since)) if old == &id => *since,
            _ => {
                self.document.tools.context.hover = Some((id, now));
                now
            }
        };
        if now - since >= 0.35 {
            self.request_context(row, &table, &key, ReadKind::Parent);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(350));
        }
    }
    pub(super) fn peek_view(&self, ui: &mut egui::Ui, row: usize, key: &ForeignKey) {
        ui.strong(format!("Parent · {}", key.target));
        let Some(table) = &self.document.table else {
            return;
        };
        let Some((id, _)) = self.relationship_read(row, &table.name, key, ReadKind::Parent) else {
            ui.label("Unresolved or NULL key");
            return;
        };
        if let Some(c) = self
            .document
            .tools
            .context
            .cache
            .iter()
            .find(|c| c.key == id)
        {
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
        let Some(table) = &self.document.table else {
            return vec![];
        };
        self.ui
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
    pub(super) fn related_list(&mut self, ui: &mut egui::Ui, row: usize) {
        if self.document.view != View::Browse {
            return;
        }
        ui.separator();
        ui.strong("Related");
        if self.ui.diagram.stale
            || self
                .ui
                .diagram
                .data
                .as_ref()
                .is_none_or(|g| g.limited || g.tables.iter().any(|n| n.error.is_some()))
        {
            ui.weak("Cached or incomplete schema — references may be missing");
        }
        let keys = self.incoming_keys();
        if keys.is_empty() {
            ui.weak("No declared incoming relationships");
        }
        for (child, key) in keys {
            let spec = self.relationship_read(row, &child, &key, ReadKind::Count);
            let count = spec
                .as_ref()
                .and_then(|(id, _)| {
                    self.document
                        .tools
                        .context
                        .cache
                        .iter()
                        .find(|c| &c.key == id)
                })
                .and_then(|c| {
                    c.result
                        .completion
                        .as_ref()
                        .filter(|done| done.delivery == Delivery::Complete && done.error.is_none())
                        .and_then(|_| c.result.rows.first()?.first())
                })
                .and_then(|v| {
                    if let Value::Integer(n) = v {
                        Some(*n)
                    } else {
                        None
                    }
                });
            let error = spec
                .as_ref()
                .and_then(|(id, _)| {
                    self.document
                        .tools
                        .context
                        .cache
                        .iter()
                        .find(|c| &c.key == id)
                })
                .map(|c| c.error.clone())
                .unwrap_or_default();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        spec.is_some(),
                        egui::Button::new(format!(
                            "↙ {child} · {}",
                            count.map_or("—".into(), |n| n.to_string())
                        ))
                        .frame_when_inactive(false),
                    )
                    .clicked()
                {
                    self.follow_incoming(row, child.clone(), key.clone());
                }
                if count.is_none()
                    && ui
                        .add_enabled(spec.is_some(), egui::Button::new("?"))
                        .on_hover_text(if error.is_empty() {
                            "Count matching rows (full scan, may be slow)".to_owned()
                        } else {
                            format!("{error} · click to retry count")
                        })
                        .clicked()
                {
                    if let Some((id, _)) = &spec {
                        self.document.tools.context.cache.retain(|c| &c.key != id);
                    }
                    self.request_context(row, &child, &key, ReadKind::Count);
                }
            });
        }
    }
    pub(super) fn wide_related(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self
            .document
            .inspect
            .map(|(r, _)| r)
            .or(self.document.tools.detail)
        else {
            return;
        };
        let keys = self.incoming_keys();
        if keys.is_empty() {
            return;
        }
        let chosen = self.document.tools.context.relation.clone();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Related rows");
            for (child, key) in &keys {
                if ui
                    .selectable_label(
                        chosen.as_ref() == Some(&(child.clone(), key.id)),
                        format!("{child} · FK {}", key.id),
                    )
                    .clicked()
                {
                    self.document.tools.context.relation = Some((child.clone(), key.id));
                }
            }
        });
        let (child, key) = keys
            .iter()
            .find(|(child, key)| {
                self.document.tools.context.relation.as_ref() == Some(&(child.clone(), key.id))
            })
            .unwrap_or(&keys[0])
            .clone();
        self.document.tools.context.relation = Some((child.clone(), key.id));
        if ui.link("Open related rows in full view").clicked() {
            self.follow_incoming(row, child.clone(), key.clone());
            return;
        }
        self.request_context(row, &child, &key, ReadKind::Rows);
        let Some((id, _)) = self.relationship_read(row, &child, &key, ReadKind::Rows) else {
            ui.weak("Select a resolved non-NULL parent key");
            return;
        };
        let Some(c) = self
            .document
            .tools
            .context
            .cache
            .iter()
            .find(|c| c.key == id)
        else {
            ui.spinner();
            ui.weak("Loading related rows…");
            return;
        };
        if !c.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &c.error);
            if ui.button("Retry related rows").clicked() {
                self.document.tools.context.cache.retain(|c| c.key != id);
                self.request_context(row, &child, &key, ReadKind::Rows);
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
