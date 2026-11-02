use super::{App, View, shell, tools};
use eframe::egui;
use tursobench_core::{ForeignKey, Value};
use tursobench_db::{Operation, Request};

use tools::Destination;
impl App {
    pub(super) fn request_inspection(&mut self, name: String) {
        self.tools.inspection = None;
        self.tools.inspection_error.clear();
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Inspect(name),
        }) {
            Ok(()) => self.tools.inspection_ticket = Some(ticket),
            Err(e) => {
                self.tools.inspection_ticket = None;
                self.tools.inspection_error = e.to_string();
            }
        }
    }
    pub(super) fn schema_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Schema");
        if self.catalog_ticket.is_some() {
            ui.label("Loading catalog…");
        }
        if !self.catalog_error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.catalog_error);
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.tools.schema_filter)
                .hint_text("Search objects…")
                .desired_width(ui.available_width()),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.ready && !self.closing,
                    egui::Button::new("Refresh schema"),
                )
                .clicked()
            {
                self.refresh_catalog();
            }
        });
        let filter = self.tools.schema_filter.to_lowercase();
        if self
            .catalog
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery != tursobench_core::Delivery::Complete || c.error.is_some())
        {
            ui.colored_label(ui.visuals().warn_fg_color, "Catalog incomplete — use SQL to inspect objects outside the retained metadata budget.");
        }
        for kind in ["table", "view", "system", "index", "trigger"] {
            let objects: Vec<_> = self
                .schema
                .iter()
                .filter(|o| {
                    (if kind == "system" {
                        o.kind == "table" && o.name.starts_with("__turso_internal_")
                    } else {
                        o.kind == kind
                            && !(kind == "table" && o.name.starts_with("__turso_internal_"))
                    }) && (o.name.to_lowercase().contains(&filter)
                        || o.table.to_lowercase().contains(&filter))
                })
                .cloned()
                .collect();
            egui::CollapsingHeader::new(format!("{kind} ({})", objects.len()))
                .default_open(kind == "table")
                .show(ui, |ui| {
                    for object in objects {
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("DDL").clicked() {
                                        self.tools.ddl = Some(object.clone());
                                        self.show_structure();
                                    }
                                    let button = ui
                                        .add_sized(
                                            [ui.available_width().max(40.0), 26.0],
                                            egui::Button::new(&object.name)
                                                .frame_when_inactive(false)
                                                .selected(
                                                    self.table
                                                        .as_ref()
                                                        .is_some_and(|t| t.name == object.name),
                                                )
                                                .truncate(),
                                        )
                                        .on_hover_text(&object.name);
                                    if button.clicked() {
                                        if kind == "table" || kind == "view" || kind == "system" {
                                            self.tools.back.clear();
                                            self.browse_table(object.name.clone());
                                        } else {
                                            self.tools.ddl = Some(object.clone());
                                            self.show_structure();
                                        }
                                    }
                                },
                            );
                        });
                    }
                });
        }
        ui.separator();
        if let Some(table) = self.table.clone() {
            if ui.button("Columns / indexes / foreign keys").clicked() {
                self.tools.ddl = None;
                self.show_structure();
                if self.tools.inspection.is_none() {
                    self.request_inspection(table.name.clone());
                }
            }
            if ui
                .button("Count rows (explicit scan)")
                .on_hover_text(
                    "Full COUNT on the browse worker. May be slow; discard does not interrupt.",
                )
                .clicked()
            {
                self.submit(Operation::Count(table.name), View::Diagnostics);
            }
        }
        ui.small("Committed browse connection. Uncommitted editor changes are not visible. No automatic row counts.");
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
                self.tools.diagnostic = diagnostic;
                self.submit(Operation::Diagnostic(diagnostic), View::Diagnostics);
            }
        }
        ui.separator();
        ui.label("No repair, VACUUM, checkpoint or writable PRAGMA shortcuts.");
        ui.small(
            "Checks run on the browse worker and may be slow. Discard does not interrupt them.",
        );
    }
    pub(super) fn schema_windows(&mut self, _ctx: &egui::Context) {
        // Compatibility with existing action state: route metadata to the dock.
        if self.tools.show_metadata {
            self.show_structure();
            self.tools.show_metadata = false;
        }
    }
    pub(super) fn structure_inspector(&mut self, ui: &mut egui::Ui) {
        let diagram = self.ui.workspace == shell::Workspace::Diagram && self.tools.ddl.is_none();
        let node = if diagram {
            self.ui
                .diagram
                .data
                .as_ref()
                .and_then(|g| {
                    g.tables
                        .iter()
                        .find(|t| Some(&t.name) == self.ui.diagram.selected.as_ref())
                })
                .cloned()
        } else {
            None
        };
        let info = if diagram {
            self.tools
                .inspection
                .as_ref()
                .filter(|i| Some(&i.table.name) == self.ui.diagram.selected.as_ref())
                .cloned()
                .or_else(|| node.as_ref().and_then(|n| n.inspection.clone()))
        } else {
            self.tools.inspection.clone()
        };
        let object = if !diagram && self.tools.ddl.is_some() {
            self.tools.ddl.clone()
        } else {
            let name = if diagram {
                self.ui.diagram.selected.as_deref()
            } else {
                info.as_ref().map(|i| i.table.name.as_str())
            };
            name.and_then(|name| self.schema.iter().find(|o| o.name == name).cloned())
        };
        if diagram && self.ui.diagram.selected.is_none() {
            ui.label(
                "Select any table in the diagram to inspect keys, constraints and relationships.",
            );
            return;
        }
        if let Some(node) = &node
            && let Some(error) = &node.error
        {
            ui.heading(&node.name);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(info) = info
            .as_ref()
            .filter(|i| object.as_ref().is_none_or(|o| o.name == i.table.name))
        {
            ui.heading(&info.table.name);
            ui.small("Metadata snapshot from the committed browse connection.");
            if ui
                .add_enabled(
                    self.ready && !self.closing,
                    egui::Button::new("Refresh structure"),
                )
                .clicked()
            {
                self.request_inspection(info.table.name.clone());
                self.refresh_catalog();
            }
            if info.table.engine_managed() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Engine-managed storage — generated edits disabled. Do not modify directly.",
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.ready && !self.closing,
                        egui::Button::new("Browse table"),
                    )
                    .clicked()
                {
                    self.browse_table(info.table.name.clone());
                }
                if ui
                    .add_enabled(self.ready && !self.closing, egui::Button::new("Count rows"))
                    .on_hover_text(
                        "Explicit full scan on the browse worker; no interruption guarantee",
                    )
                    .clicked()
                {
                    self.submit(Operation::Count(info.table.name.clone()), View::Diagnostics);
                }
            });
            ui.separator();
            ui.strong("Columns / keys");
            for c in &info.table.columns {
                ui.monospace(format!("{} · {}", c.name, c.declared_type));
                ui.small(format!(
                    "{} · PK position {} · hidden {}",
                    if c.not_null {
                        "NOT NULL"
                    } else {
                        "NOT NULL not reported"
                    },
                    c.primary_key,
                    c.hidden
                ));
                if let Some(default) = &c.default_sql {
                    ui.label(format!("DEFAULT {default}"));
                }
            }
            ui.separator();
            ui.strong("Indexes");
            if info.indexes.is_empty() {
                ui.small("None returned");
            }
            for index in &info.indexes {
                ui.monospace(&index.name);
                ui.label(format!(
                    "{} · {} ({})",
                    if index.unique { "UNIQUE" } else { "non-unique" },
                    if index.partial { "partial" } else { "full" },
                    index
                        .columns
                        .iter()
                        .map(|c| c
                            .as_deref()
                            .unwrap_or("<expression / method metadata unavailable>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            ui.separator();
            ui.strong("Foreign keys / relationships");
            if info.foreign_keys.is_empty() {
                ui.small("None declared — relationships are never inferred from names");
            }
            for fk in &info.foreign_keys {
                ui.monospace(format!(
                    "FK {}: {} → {} ({})",
                    fk.id,
                    fk.columns
                        .iter()
                        .map(|(s, _)| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    fk.target,
                    fk.columns
                        .iter()
                        .map(|(_, t)| t.as_deref().unwrap_or("<unresolved PK>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                ui.small(format!(
                    "ON UPDATE {} · ON DELETE {}",
                    fk.on_update, fk.on_delete
                ));
            }
        } else if !diagram && object.is_none() {
            ui.label(if self.tools.inspection_error.is_empty() {
                "Choose a table or schema object. Select a browse cell to follow row references."
            } else {
                &self.tools.inspection_error
            });
        }
        if info.is_none() && !self.tools.inspection_error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.tools.inspection_error);
        }
        let owner = info
            .as_ref()
            .map(|i| i.table.name.as_str())
            .or_else(|| object.as_ref().map(|o| o.table.as_str()));
        if let Some(owner) = owner {
            let objects: Vec<_> = self
                .schema
                .iter()
                .filter(|o| o.table == owner && (o.kind == "index" || o.kind == "trigger"))
                .cloned()
                .collect();
            ui.separator();
            ui.strong("Indexes and triggers · original definitions");
            for object in objects {
                ui.collapsing(format!("{} {}", object.kind, object.name), |ui| {
                    if let Some(ddl) = &object.ddl {
                        if ui.button("Copy definition").clicked() {
                            ui.ctx().copy_text(ddl.clone());
                        }
                        ui.monospace(ddl);
                    } else {
                        ui.label("Engine-generated object; no CREATE statement returned.");
                    }
                });
            }
            ui.small(
                "CHECK clauses, index expressions and FTS methods are shown in the original DDL.",
            );
        }
        if let Some(object) = object {
            ui.separator();
            ui.strong(format!("{} {} — original DDL", object.kind, object.name));
            if let Some(ddl) = object.ddl {
                if ui.button("Copy DDL").clicked() {
                    ui.ctx().copy_text(ddl.clone());
                }
                let mut ddl_view = ddl.as_str();
                ui.collapsing("CREATE statement", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut ddl_view)
                            .code_editor()
                            .desired_width(ui.available_width())
                            .desired_rows(8),
                    );
                });
            } else {
                ui.small("No SQL definition returned by Turso");
            }
            if (object.kind == "index" || object.kind == "trigger")
                && ui
                    .button(format!("Browse owning table: {}", object.table))
                    .clicked()
            {
                self.browse_table(object.table);
            }
        } else if diagram {
            ui.small("DDL was not retained in the schema catalog. Refresh schema to retrieve it within display budgets.");
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
            || self.view != View::Browse
            || self.active.is_some()
        {
            self.status =
                "Finish execution and commit or discard drafts before following a reference".into();
            return;
        }
        let Some(table) = self.table.as_ref() else {
            return;
        };
        let Some(values) = self.result.rows.get(row) else {
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
        let previous = self
            .tools
            .related
            .clone()
            .or_else(|| self.browse.clone().map(Destination::Page));
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
            self.browse = None;
            self.table = None;
            self.ui.diagram.selected = Some(key.target.clone());
            self.tools.ddl = None;
            self.tools.related = Some(Destination::Related(key.target, keys));
        }
    }
    pub(super) fn follow_incoming(&mut self, row: usize, child: String, key: ForeignKey) {
        if self.active.is_some() || self.view != View::Browse {
            return;
        }
        let (Some(table), Some(values)) = (&self.table, self.result.rows.get(row)) else {
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
        let previous = self
            .tools
            .related
            .clone()
            .or_else(|| self.browse.clone().map(Destination::Page));
        if self.submit(destination.operation(), View::Browse).is_some() {
            if let Some(previous) = previous {
                self.tools.back.push(previous);
                if self.tools.back.len() > 32 {
                    self.tools.back.remove(0);
                }
            }
            self.browse = None;
            self.table = None;
            self.tools.ddl = None;
            self.ui.diagram.selected = Some(child);
            self.tools.related = Some(destination);
        }
    }
    pub(super) fn navigate_back(&mut self) {
        let Some(destination) = self.tools.back.last().cloned() else {
            return;
        };
        let operation = destination.operation();
        if self.submit(operation, View::Browse).is_some() {
            self.tools.back.pop();
            self.tools.ddl = None;
            self.table = None;
            match destination {
                Destination::Page(b) => {
                    self.ui.diagram.selected = Some(b.table.clone());
                    self.browse = Some(b);
                    self.tools.related = None;
                }
                related => {
                    self.ui.diagram.selected = Some(related.table().into());
                    self.browse = None;
                    self.tools.related = Some(related);
                }
            }
        }
    }
}
