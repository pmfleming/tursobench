use super::results::code_view;
use super::{
    App, View,
    icons::{self, Icon},
    shell, tools,
};
use eframe::egui;
use tursobench_core::{Event, EventKind, SchemaObject};
use tursobench_core::{ForeignKey, Value};
use tursobench_db::{Operation, Request};

use tools::{Destination, NavigationEntry};
impl App {
    pub(super) fn finish_count(&mut self, failure: Option<String>) {
        let Some((name, result)) = self.tools.count_request.take() else {
            return;
        };
        if result
            .completion
            .as_ref()
            .is_some_and(|c| c.error.is_none() && c.delivery == tursobench_core::Delivery::Complete)
            && let Some(Value::Integer(n)) = result.rows.first().and_then(|r| r.first())
        {
            self.status = format!("{name}: {n} rows · explicit full scan");
            self.tools.counts.insert(name, *n);
        } else {
            self.status = failure.map_or_else(
                || format!("Count failed or incomplete: {:?}", result.completion),
                |error| format!("Count failed: {error}"),
            );
        }
        self.log_outcome(self.status.clone());
    }
    pub(super) fn catalog_event(&mut self, event: Event) {
        let done = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        let error = match &event.kind {
            EventKind::Finished(c) => {
                self.transaction = c.transaction;
                c.error.as_ref()
            }
            EventKind::Failed(error) => Some(error),
            _ => None,
        };
        if let Some(error) = error {
            self.catalog_error = error.to_string();
            self.status.clone_from(&self.catalog_error);
        }
        self.catalog.apply(event);
        if done {
            self.catalog_ticket = None;
            self.schema = self
                .catalog
                .rows
                .drain(..)
                .filter_map(|row| {
                    let [
                        Value::Text(kind),
                        Value::Text(name),
                        Value::Text(table),
                        ddl,
                    ]: [Value; 4] = row.try_into().ok()?
                    else {
                        return None;
                    };
                    Some(SchemaObject {
                        kind,
                        name,
                        table,
                        ddl: match ddl {
                            Value::Text(sql) => Some(sql),
                            _ => None,
                        },
                    })
                })
                .collect();
        }
    }

    pub(super) fn count_table(&mut self, name: String) {
        if self.tools.count_request.is_some() || !self.ready || self.closing {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Count(name.clone()),
        }) {
            Ok(()) => {
                let mut result = tursobench_core::ResultSet::default();
                result.start(ticket);
                self.tools.count_request = Some((name, result));
                self.status =
                    "Counting rows; full scan, discard does not interrupt execution".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub(super) fn request_inspection(&mut self, name: String) {
        self.document.tools.inspection = None;
        self.document.tools.inspection_error.clear();
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Inspect(name),
        }) {
            Ok(()) => self.document.tools.inspection_ticket = Some(ticket),
            Err(e) => {
                self.document.tools.inspection_ticket = None;
                self.document.tools.inspection_error = e.to_string();
            }
        }
    }
    pub(super) fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Read-only checks");
        for diagnostic in tursobench_db::Diagnostic::ALL {
            if ui
                .add_enabled(
                    self.ready && !self.closing,
                    egui::Button::new(diagnostic.label()),
                )
                .clicked()
            {
                self.submit(Operation::Diagnostic(diagnostic), View::Diagnostics);
            }
        }
        ui.separator();
        ui.label("No repair, VACUUM, checkpoint or writable PRAGMA shortcuts.");
        ui.small(
            "Checks run on the browse worker and may be slow. Discard does not interrupt them.",
        );
    }
    pub(super) fn structure_inspector(&mut self, ui: &mut egui::Ui) {
        let name = self
            .document
            .tools
            .ddl
            .as_ref()
            .map(|o| o.name.clone())
            .or_else(|| self.document.object.clone())
            .or_else(|| self.ui.diagram.selected.clone());
        let Some(name) = name else {
            ui.label("Choose a table to inspect its structure.");
            return;
        };
        let info = self.loaded_inspection(&name);
        let object = self
            .document
            .tools
            .ddl
            .as_ref()
            .or_else(|| self.schema.iter().find(|o| o.name == name));
        let mut browse = None;
        let mut load = false;
        if let Some(node) = self
            .ui
            .diagram
            .data
            .as_ref()
            .and_then(|g| g.tables.iter().find(|n| n.name == name))
            && let Some(error) = &node.error
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.document.tools.inspection_ticket.is_some() {
            ui.small("Loading structure…");
        }
        if !self.document.tools.inspection_error.is_empty() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                &self.document.tools.inspection_error,
            );
        }
        let count = if ui.ctx().content_rect().width() >= 1920.0 && ui.available_width() >= 800.0 {
            2
        } else {
            1
        };
        ui.columns(count, |panes| {
            let ui = &mut panes[0];
            if let Some(info) = &info {
                if info.table.engine_managed() {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Engine-managed storage · generated edits disabled",
                    );
                }
                ui.strong("Columns").on_hover_text(
                    "Committed browse metadata. Uncommitted editor changes are not visible.",
                );
                ui.group(|ui| {
                    for c in &info.table.columns {
                        ui.horizontal_wrapped(|ui| {
                            if c.primary_key > 0 {
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(14.0, 14.0),
                                    egui::Sense::hover(),
                                );
                                icons::paint(
                                    ui.painter(),
                                    r,
                                    Icon::Key,
                                    ui.visuals().warn_fg_color,
                                );
                            }
                            ui.monospace(&c.name);
                            ui.weak(&c.declared_type);
                            if c.primary_key > 0 {
                                ui.small(format!("PK {}", c.primary_key));
                            }
                            if c.not_null {
                                ui.small("NOT NULL");
                            }
                            if c.hidden > 0 {
                                ui.small(format!("hidden {}", c.hidden));
                            }
                            if let Some(default) = &c.default_sql {
                                ui.small(format!("DEFAULT {default}"));
                            }
                        });
                        ui.separator();
                    }
                });
            }
            let ui = &mut panes[count - 1];
            if let Some(info) = &info {
                ui.strong("Indexes");
                if info.indexes.is_empty() {
                    ui.weak("No declared indexes returned");
                }
                for index in &info.indexes {
                    ui.group(|ui| {
                        ui.monospace(&index.name);
                        ui.horizontal_wrapped(|ui| {
                            if index.unique {
                                ui.small("UNIQUE");
                            }
                            if index.partial {
                                ui.small("partial");
                            }
                            ui.label(
                                index
                                    .columns
                                    .iter()
                                    .map(|c| c.as_deref().unwrap_or("expression / method: see DDL"))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            );
                        });
                    });
                }
                ui.strong("Foreign keys");
                if info.foreign_keys.is_empty() {
                    ui.weak("No declared foreign keys");
                }
                for key in &info.foreign_keys {
                    ui.group(|ui| {
                        if ui.link(&key.target).clicked() {
                            browse = Some(key.target.clone());
                        }
                        ui.monospace(
                            key.columns
                                .iter()
                                .map(|(from, to)| {
                                    format!(
                                        "{from} references {}",
                                        to.as_deref().unwrap_or("unresolved PK")
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                        ui.small(format!(
                            "ON UPDATE {} · ON DELETE {}",
                            key.on_update, key.on_delete
                        ));
                    });
                }
            } else if object.is_none_or(|o| o.kind == "table" || o.kind == "view")
                && self.document.tools.inspection_ticket.is_none()
                && ui.button("Load structure").clicked()
            {
                load = true;
            }
            let owner = info
                .as_ref()
                .map(|i| i.table.name.as_str())
                .or_else(|| object.as_ref().map(|o| o.table.as_str()));
            if let Some(owner) = owner {
                let definitions: Vec<_> = self
                    .schema
                    .iter()
                    .filter(|o| o.table == owner && (o.kind == "index" || o.kind == "trigger"))
                    .collect();
                if !definitions.is_empty() {
                    ui.strong("Index and trigger definitions");
                }
                for object in definitions {
                    ui.collapsing(format!("{} {}", object.kind, object.name), |ui| {
                        if let Some(ddl) = &object.ddl {
                            if icons::button(ui, Icon::Copy, "Copy definition", "").clicked() {
                                ui.ctx().copy_text(ddl.clone());
                            }
                            ui.monospace(ddl);
                        } else {
                            ui.weak("Engine-generated definition");
                        }
                    });
                }
            }
            ui.strong("Definition");
            ui.weak(
            "CHECK clauses, index expressions and FTS methods are preserved in the original DDL.",
        );
            if let Some(object) = object
                && let Some(ddl) = &object.ddl
            {
                ui.collapsing("CREATE statement", |ui| {
                    if icons::button(ui, Icon::Copy, "Copy original DDL", "").clicked() {
                        ui.ctx().copy_text(ddl.clone());
                    }
                    code_view(ui, ddl, 8);
                });
            } else {
                ui.weak("DDL not retained; refresh the catalog.");
            }
        });
        if load {
            self.request_inspection(name);
        }
        if let Some(table) = browse {
            self.browse_table(table);
        }
    }
    pub(super) fn follow_key(&mut self, row: usize, key: ForeignKey) {
        if !self.ready || self.closing {
            self.status =
                "Open a database before following references; retained values are unchanged".into();
            return;
        }
        if self.has_drafts()
            || self.commit_ticket.is_some()
            || self.document.view != View::Browse
            || self.document.active.is_some()
        {
            self.status =
                "Finish execution and commit or discard drafts before following a reference".into();
            return;
        }
        let Some(table) = self.document.table.as_ref() else {
            return;
        };
        let Some(values) = self.document.result.rows.get(row) else {
            return;
        };
        let mut keys = Vec::new();
        for (source, target) in key.columns {
            let Some(target) = target else {
                self.status = "Referenced primary key could not be resolved".into();
                return;
            };
            let Some(index) = table
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&source))
            else {
                return;
            };
            let Some(value) = values.get(index) else {
                return;
            };
            if *value == Value::Null {
                self.status = "NULL foreign key has no referenced row".into();
                return;
            }
            keys.push((target, value.clone()));
        }
        let previous = self.navigation_entry();
        if self.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(key.target.clone()),
                    Some(self.document.id),
                    true,
                )
                .is_none()
        {
            return;
        }
        if self
            .submit(
                Operation::Lookup {
                    table: key.target.clone(),
                    keys: keys.clone(),
                },
                View::Browse,
            )
            .is_some()
        {
            if let Some(previous) = previous {
                self.tools.back.push(previous);
                if self.tools.back.len() > 32 {
                    self.tools.back.remove(0);
                }
            }
            self.document.browse = None;
            self.document.table = None;
            self.ui.diagram.selected = Some(key.target.clone());
            self.document.tools.ddl = None;
            self.document.tools.related = Some(Destination::Related(key.target, keys));
        }
    }
    pub(super) fn follow_incoming(&mut self, row: usize, child: String, key: ForeignKey) {
        if self.document.active.is_some() || self.document.view != View::Browse {
            return;
        }
        let (Some(table), Some(values)) =
            (&self.document.table, self.document.result.rows.get(row))
        else {
            return;
        };
        if !table.name.eq_ignore_ascii_case(&key.target) {
            return;
        }
        let mut bound = Vec::new();
        for (_, target) in &key.columns {
            let Some(target) = target else {
                return;
            };
            let Some(index) = table
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(target))
            else {
                return;
            };
            let Some(value) = values.get(index).filter(|v| **v != Value::Null) else {
                return;
            };
            bound.push(value.clone());
        }
        let destination = Destination::References(child.clone(), key, bound);
        let previous = self.navigation_entry();
        if self.has_drafts() || self.commit_ticket.is_some() {
            return;
        }
        if self.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(child.clone()),
                    Some(self.document.id),
                    true,
                )
                .is_none()
        {
            return;
        }
        if self.submit(destination.operation(), View::Browse).is_some() {
            if let Some(previous) = previous {
                self.tools.back.push(previous);
                if self.tools.back.len() > 32 {
                    self.tools.back.remove(0);
                }
            }
            self.document.browse = None;
            self.document.table = None;
            self.document.tools.ddl = None;
            self.ui.diagram.selected = Some(child);
            self.document.tools.related = Some(destination);
        }
    }
    pub(super) fn navigation_entry(&self) -> Option<NavigationEntry> {
        if self.document.view != View::Browse {
            return None;
        }
        let destination = self
            .document
            .tools
            .related
            .clone()
            .or_else(|| self.document.browse.clone().map(Destination::Page))?;
        Some(NavigationEntry {
            destination,
            cell: self.document.inspect.and_then(|(r, c)| {
                self.document
                    .result
                    .rows
                    .get(r)
                    .cloned()
                    .map(|row| (row, c))
            }),
            record: self
                .document
                .tools
                .detail
                .and_then(|r| self.document.result.rows.get(r).cloned()),
            structure: self.document.ui.table_view == shell::TableView::Structure,
            scroll: self.document.tools.grid_scroll,
            window_rows: self.document.result.rows.len(),
        })
    }
    pub(super) fn restore_destination(&mut self, entry: NavigationEntry) -> bool {
        if self.has_drafts() || self.commit_ticket.is_some() {
            return false;
        }
        if self.workspace.enabled
            && self
                .create_window(
                    super::tiling::Kind::Data,
                    Some(entry.destination.table().into()),
                    None,
                    false,
                )
                .is_none()
        {
            return false;
        }
        if self
            .submit(entry.destination.operation(), View::Browse)
            .is_none()
        {
            return false;
        }
        self.document.tools.ddl = None;
        self.document.table = None;
        self.ui.diagram.selected = Some(entry.destination.table().into());
        self.ui.unfolded = Some(entry.destination.table().into());
        match &entry.destination {
            Destination::Page(b) => {
                self.document.browse = Some(b.clone());
                self.document.tools.related = None;
            }
            related => {
                self.document.browse = None;
                self.document.tools.related = Some(related.clone());
            }
        }
        self.document.ui.table_view = if entry.structure {
            shell::TableView::Structure
        } else {
            shell::TableView::Data
        };
        self.document.tools.pages.restore_rows = if self.document.browse.is_some() {
            entry
                .window_rows
                .min(self.preferences.row_limit.clamp(200, 10_000) - 100)
        } else {
            0
        };
        self.document.tools.restore_navigation = Some(entry);
        true
    }
    pub(super) fn navigate_back(&mut self) {
        let Some(entry) = self.tools.back.last().cloned() else {
            return;
        };
        if self.restore_destination(entry) {
            self.tools.back.pop();
        }
    }
    pub(super) fn return_to_table(&mut self) {
        let Some(entry) = self.tools.result_return.clone() else {
            return;
        };
        if self.restore_destination(entry) {
            self.tools.result_return = None;
        }
    }
    pub(super) fn restore_browse_selection(&mut self) {
        if self.document.tools.pages.restore_rows > self.document.result.rows.len()
            && self.document.tools.pages.end.is_none()
        {
            return;
        }
        self.document.tools.pages.restore_rows = 0;
        let Some(entry) = self.document.tools.restore_navigation.take() else {
            return;
        };
        self.document.inspect = entry.cell.and_then(|(values, column)| {
            (column < self.document.result.columns.len())
                .then(|| {
                    self.document
                        .result
                        .rows
                        .iter()
                        .position(|r| *r == values)
                        .map(|row| (row, column))
                })
                .flatten()
        });
        self.document.tools.detail = entry
            .record
            .and_then(|values| self.document.result.rows.iter().position(|r| *r == values));
        self.document.tools.grid_selection =
            self.document
                .inspect
                .map(|cell| super::selection::Selection {
                    ticket: self.document.result.current,
                    anchor: cell,
                    cursor: cell,
                });
        self.document.tools.restore_scroll = Some(entry.scroll);
    }
}
