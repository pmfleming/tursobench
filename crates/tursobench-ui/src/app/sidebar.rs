use super::{
    App, View,
    icons::{self, Icon},
    shell::{Nav, TableView, ToolPanel},
};
use eframe::egui;
use tursobench_core::Inspection;

impl App {
    pub(super) fn overview(&mut self) {
        if self.database.workspace.enabled {
            self.create_window(super::tiling::Kind::Map, None, None, false);
        }
        self.database.document.ui.nav = Nav::Overview;
        self.database.document.ui.inspector_open = false;
        if self.database.ui.diagram.data.is_none() {
            self.load_diagram();
        }
    }

    pub(super) fn activate_table(&mut self, name: String) {
        if self.database.workspace.enabled {
            self.browse_table(name);
            return;
        }
        if self.database.document.view == View::Browse
            && self
                .database
                .document
                .table
                .as_ref()
                .is_some_and(|table| table.name == name)
        {
            self.database.document.ui.nav = Nav::Table;
            self.database.document.tools.ddl = None;
            self.database.ui.unfolded = Some(name);
        } else {
            self.browse_table(name);
        }
    }

    pub(super) fn loaded_inspection(&self, name: &str) -> Option<&Inspection> {
        self.database
            .document
            .tools
            .inspection
            .as_ref()
            .filter(|i| i.table.name == name)
            .or_else(|| {
                if self.database.ui.diagram.stale {
                    return None;
                }
                self.database
                    .ui
                    .diagram
                    .data
                    .as_ref()?
                    .tables
                    .iter()
                    .find(|n| n.name == name)?
                    .inspection
                    .as_ref()
            })
    }

    pub(super) fn related_tables(&self, name: &str) -> std::collections::BTreeSet<String> {
        let mut related = std::collections::BTreeSet::new();
        if let Some(graph) = &self.database.ui.diagram.data {
            for node in &graph.tables {
                if let Some(info) = &node.inspection {
                    for fk in &info.foreign_keys {
                        if node.name == name {
                            related.insert(fk.target.clone());
                        }
                        if fk.target == name {
                            related.insert(node.name.clone());
                        }
                    }
                }
            }
        }
        related
    }

    pub(super) fn navigation_sidebar(&mut self, ui: &mut egui::Ui) {
        self.sidebar_search_ui(ui);
        if !self.sidebar_search.query.trim().is_empty() && !self.database.confirm_close {
            self.sidebar_search_results_ui(ui);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("database-navigation")
            .show(ui, |ui| {
                self.open_databases_ui(ui);
                if self.database.session.is_none()
                    && self.database.ui.sidebar == ToolPanel::Settings
                {
                    self.settings_ui(ui);
                }
                self.available_databases_ui(ui);
            });
    }

    pub(super) fn active_database_tree_ui(&mut self, ui: &mut egui::Ui) {
        if self.database.ui.sidebar != ToolPanel::Schema {
            if self.database.ui.sidebar == ToolPanel::Diagnostics {
                self.diagnostics_ui(ui);
            } else if self.database.ui.sidebar == ToolPanel::Settings {
                self.settings_ui(ui);
            } else {
                self.history_controls(ui);
            }
            return;
        }
        self.schema_tree_ui(ui);
    }

    pub(super) fn schema_tree_ui(&mut self, ui: &mut egui::Ui) {
        if self.database.catalog_ticket.is_some() {
            ui.spinner();
        }
        if !self.database.catalog_error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.database.catalog_error);
        }
        if self
            .database
            .catalog
            .completion
            .as_ref()
            .is_some_and(|c| c.delivery != tursobench_core::Delivery::Complete || c.error.is_some())
        {
            ui.colored_label(ui.visuals().warn_fg_color,"Catalog incomplete").on_hover_text("Retained metadata budget reached or catalog failed. Use SQL to inspect omitted objects.");
        }
        let filter = if self.sidebar_search.scope == super::database_navigation::Scope::ThisDatabase
        {
            self.sidebar_search.query.trim().to_lowercase()
        } else {
            String::new()
        };
        let related = self
            .database
            .ui
            .hover_table
            .as_deref()
            .map(|t| self.related_tables(t))
            .unwrap_or_default();
        let mut hovered = None;
        for (kind, title, icon) in [
            ("table", "Tables", Icon::Data),
            ("view", "Views", Icon::Data),
            ("system", "System", Icon::System),
            ("index", "Indexes", Icon::Columns),
            ("trigger", "Triggers", Icon::System),
        ] {
            let objects: Vec<_> = self
                .database
                .schema
                .iter()
                .filter(|o| {
                    let system = o.name.starts_with("__turso_internal_");
                    (if kind == "system" {
                        o.kind == "table" && system
                    } else {
                        o.kind == kind && !system
                    }) && (super::commands::fuzzy(&o.name, &filter)
                        || self.loaded_inspection(&o.name).is_some_and(|i| {
                            i.table.columns.iter().any(|c| {
                                super::commands::fuzzy(&format!("{}.{}", o.name, c.name), &filter)
                            })
                        }))
                })
                .map(|o| o.name.clone())
                .collect();
            if !filter.is_empty() && objects.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} · {}", title.to_uppercase(), objects.len()))
                    .size(crate::appearance::text_size(ui.ctx(), 10.5))
                    .strong()
                    .color(crate::appearance::Tokens::of(ui.ctx()).muted),
            )
            .id_salt(("nav-group", kind))
            .open((!filter.is_empty()).then_some(true))
            .default_open(kind == "table" || kind == "view" || !filter.is_empty())
            .show(ui, |ui| {
                for name in objects {
                    if self.sidebar_object(
                        ui,
                        &name,
                        if kind == "system" { "table" } else { kind },
                        icon,
                        related.contains(&name),
                    ) {
                        hovered = Some(name);
                    }
                }
            });
        }
        self.database.ui.hover_table = hovered;
    }

    fn sidebar_object(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        kind: &str,
        icon: Icon,
        related: bool,
    ) -> bool {
        if kind == "index" || kind == "trigger" {
            let selected = self
                .database
                .document
                .tools
                .ddl
                .as_ref()
                .is_some_and(|o| o.name == name);
            let response = tree_row(ui, name, "DDL", icon, selected, false, (22.0, 0.0));
            if response.has_focus() {
                self.database.tools.tree_focus = Some(response.id);
            }
            if response.clicked()
                || response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            {
                self.execute_command(ui.ctx(), super::commands::Command::Definition(name.into()));
            }
            return response.hovered();
        }
        let highlighted = self.sidebar_search.scope
            == super::database_navigation::Scope::ThisDatabase
            && !self.sidebar_search.query.trim().is_empty();
        let keyboard = self
            .sidebar_search
            .tree_highlight
            .as_ref()
            .filter(|(table, _)| table == name);
        let mut hovered = false;
        let selected = highlighted && keyboard.is_some_and(|(_, column)| column.is_none())
            || self.database.document.view == View::Browse
                && self.database.document.ui.nav == Nav::Table
                && self.database.ui.diagram.selected.as_deref() == Some(name);
        let unfolded = self.database.ui.unfolded.as_deref() == Some(name)
            || highlighted
                && (keyboard.is_some_and(|(_, column)| column.is_some())
                    || !super::commands::fuzzy(name, &self.sidebar_search.query)
                        && self.loaded_inspection(name).is_some());
        let count = self
            .database
            .tools
            .counts
            .get(name)
            .map_or("—".into(), |n| n.to_string());
        let response = tree_row(ui, name, &count, icon, selected, related, (26.0, 22.0));
        let disclosure = egui::Rect::from_min_size(response.rect.min, egui::vec2(20.0, 26.0));
        icons::paint(
            ui.painter(),
            disclosure.shrink(4.0),
            if unfolded { Icon::Down } else { Icon::Next },
            ui.visuals().weak_text_color(),
        );
        let arrow = ui.interact(
            disclosure,
            ui.scope_id().with(("disclosure", name)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::TreeItem,
                ui.is_enabled(),
                selected,
                format!("{name} · {kind}"),
            )
        });
        if response.has_focus() {
            self.database.tools.tree_focus = Some(response.id);
        }
        if arrow.clicked()
            || response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
        {
            self.database.ui.unfolded = if unfolded {
                None
            } else {
                Some(name.to_owned())
            };
            if !unfolded && self.loaded_inspection(name).is_none() {
                self.request_inspection(name.to_owned());
            }
        } else if response.clicked()
            || response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            self.activate_table(name.to_owned());
        }
        if response.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
        {
            self.database.ui.unfolded = None;
        }
        hovered |= response.hovered();
        if unfolded {
            self.sidebar_table(ui, name);
        }
        hovered
    }

    fn sidebar_table(&mut self, ui: &mut egui::Ui, name: &str) {
        let Some(info) = self.loaded_inspection(name) else {
            if self.database.document.tools.inspection_ticket.is_some() {
                ui.small("Loading columns…");
            } else if !self.database.document.tools.inspection_error.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    &self.database.document.tools.inspection_error,
                );
            } else if ui.small_button("Load columns").clicked() {
                self.request_inspection(name.into());
            }
            return;
        };
        let mut column = None;
        let mut table = None;
        ui.indent(("columns",name),|ui| {
            for c in &info.table.columns {
                let filtering = self.sidebar_search.scope == super::database_navigation::Scope::ThisDatabase && !self.sidebar_search.query.trim().is_empty();
                if filtering && !super::commands::fuzzy(name, &self.sidebar_search.query)
                    && !super::commands::fuzzy(&format!("{name}.{}", c.name), &self.sidebar_search.query) { continue; }
                let selected = filtering && self.sidebar_search.tree_highlight.as_ref().is_some_and(|(table,column)| table == name && column.as_deref() == Some(c.name.as_str()));
                let fk=info.foreign_keys.iter().any(|k|k.columns.iter().any(|(n,_)|n==&c.name));
                let icon=if c.primary_key>0{Icon::Key}else if fk{Icon::Link}else{Icon::Columns};
                if tree_row(ui, &c.name, &c.declared_type, icon, selected, fk, (22.0, 0.0)).clicked() { column=Some(c.name.clone()); }
            }
            let mut links:Vec<(&str,String)>=info.foreign_keys.iter().map(|k|(k.target.as_str(),format!("Parent: {}",k.target))).collect();
            if let Some(graph)=&self.database.ui.diagram.data {
                for node in &graph.tables {
                    if node.inspection.as_ref().is_some_and(|i|i.foreign_keys.iter().any(|k|k.target==name)) {links.push((node.name.as_str(),format!("Referenced by: {}",node.name)));}
                }
            }
            for (target,label) in links {
                if ui.link(egui::RichText::new(label).color(crate::appearance::Tokens::of(ui.ctx()).relationship)).on_hover_text("Open table without a row filter. Select a record to follow a specific key.").clicked(){table = Some(target.to_owned());}
            }
        });
        if let Some(column) = column {
            self.highlight_sidebar_column(name, &column);
        }
        if let Some(table) = table {
            self.browse_table(table);
        }
    }

    fn highlight_sidebar_column(&mut self, name: &str, column: &str) {
        if self
            .database
            .document
            .table
            .as_ref()
            .is_none_or(|t| t.name != name)
            || self.database.document.view != View::Browse
        {
            let ticket = self.database.document.result.current;
            self.browse_table(name.into());
            if ticket == self.database.document.result.current {
                return;
            }
        }
        self.database.document.ui.nav = Nav::Table;
        self.database.document.ui.table_view = TableView::Data;
        self.database.document.tools.highlight_column = Some((name.into(), column.into()));
        self.database.document.tools.scroll_to_column = true;
    }

    pub(super) fn select_tool(&mut self, panel: ToolPanel) {
        if self.database.session.is_none()
            && !matches!(panel, ToolPanel::Schema | ToolPanel::Settings)
        {
            self.database.status = "Open a database to use this tool".into();
            return;
        }
        self.database.ui.sidebar = panel;
    }

    pub(super) fn navigator_utilities(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let width = ((ui.available_width() - 8.0) / 5.0).max(18.0);
            for (icon, label, name, panel) in [
                (Icon::Data, "Tables", "Tables", ToolPanel::Schema),
                (Icon::History, "History", "History", ToolPanel::History),
                (Icon::Saved, "Saved", "Saved queries", ToolPanel::Saved),
                (
                    Icon::Checks,
                    "Checks",
                    "Database checks",
                    ToolPanel::Diagnostics,
                ),
                (Icon::Settings, "Settings", "Settings", ToolPanel::Settings),
            ] {
                let selected = panel == self.database.ui.sidebar;
                let enabled = self.database.session.is_some() || panel == ToolPanel::Settings;
                if ui
                    .add_enabled_ui(enabled, |ui| {
                        icons::rail_button(ui, icon, label, name, selected, width)
                    })
                    .inner
                    .clicked()
                {
                    self.select_tool(panel);
                }
            }
        });
    }
}

fn tree_row(
    ui: &mut egui::Ui,
    name: &str,
    detail: &str,
    icon: Icon,
    selected: bool,
    related: bool,
    size: (f32, f32),
) -> egui::Response {
    let (height, indent) = size;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::TreeItem, ui.is_enabled(), selected, name)
    });
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    if selected || response.hovered() || response.has_focus() {
        painter.rect_filled(
            rect,
            4.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().faint_bg_color
            },
        );
    }
    if selected {
        painter.vline(
            rect.left() + 1.0,
            rect.y_range().shrink(3.0),
            egui::Stroke::new(2.0, tokens.primary),
        );
    }
    let color = if related {
        tokens.relationship
    } else {
        ui.visuals().text_color()
    };
    icons::paint(
        &painter,
        egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(indent + 9.0, 0.0),
            egui::vec2(12.0, 12.0),
        ),
        icon,
        if matches!(icon, Icon::Key | Icon::Link) {
            tokens.relationship
        } else if selected {
            tokens.primary
        } else {
            tokens.muted
        },
    );
    let right = painter.layout_no_wrap(
        detail.to_owned(),
        crate::appearance::monospace(ui.ctx(), 10.0),
        ui.visuals().weak_text_color(),
    );
    let clip = egui::Rect::from_min_max(
        rect.min + egui::vec2(indent + 22.0, 0.0),
        rect.max - egui::vec2(right.size().x + 10.0, 0.0),
    );
    painter.with_clip_rect(clip.intersect(ui.clip_rect())).text(
        rect.left_center() + egui::vec2(indent + 22.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        crate::appearance::proportional(ui.ctx(), 12.0),
        color,
    );
    painter.galley(
        rect.right_center() - egui::vec2(right.size().x + 4.0, right.size().y / 2.0),
        right,
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(name)
}
