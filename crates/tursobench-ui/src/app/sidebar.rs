use super::{
    App, View,
    icons::{self, Icon},
    shell::{Nav, TableView, ToolPanel},
};
use eframe::egui;
use tursobench_core::Inspection;

impl App {
    pub(super) fn overview(&mut self) {
        if self.workspace.enabled {
            self.create_window(super::tiling::Kind::Map, None, None, false);
        }
        self.document.ui.nav = Nav::Overview;
        self.document.ui.inspector_open = false;
        if self.ui.diagram.data.is_none() {
            self.load_diagram();
        }
    }

    pub(super) fn activate_table(&mut self, name: String) {
        if self.workspace.enabled {
            self.browse_table(name);
            return;
        }
        if self.document.view == View::Browse
            && self
                .document
                .table
                .as_ref()
                .is_some_and(|table| table.name == name)
        {
            self.document.ui.nav = Nav::Table;
            self.document.tools.ddl = None;
            self.ui.unfolded = Some(name);
        } else {
            self.browse_table(name);
        }
    }

    pub(super) fn loaded_inspection(&self, name: &str) -> Option<&Inspection> {
        self.document
            .tools
            .inspection
            .as_ref()
            .filter(|i| i.table.name == name)
            .or_else(|| {
                self.ui
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
        if let Some(graph) = &self.ui.diagram.data {
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
        let name = std::path::Path::new(&self.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        ui.menu_button(format!("{name} ▾"), |ui| {
            ui.label("Recent databases");
            let recent: Vec<_> = self
                .preferences
                .discovery
                .recent
                .iter()
                .take(10)
                .map(|r| r.path.clone())
                .collect();
            for path in recent {
                if ui.button(path.display().to_string()).clicked() {
                    // Never transfer access consent or open another file before Closed.
                    self.pending_database = Some(path);
                    self.request_close();
                    ui.close();
                }
            }
            if ui.button("Open file… · Ctrl O").clicked() {
                self.request_close();
                ui.close();
            }
            if ui.button("Close database").clicked() {
                self.request_close();
                ui.close();
            }
        });
        if self.ui.sidebar != ToolPanel::Schema {
            egui::ScrollArea::vertical()
                .id_salt("sidebar-tool")
                .show(ui, |ui| {
                    if self.ui.sidebar == ToolPanel::Diagnostics {
                        self.diagnostics_ui(ui);
                    } else {
                        self.history_controls(ui);
                    }
                });
            return;
        }
        let find = ui.add(
            egui::TextEdit::singleline(&mut self.tools.schema_filter)
                .hint_text("Find tables or columns…")
                .desired_width(ui.available_width()),
        );
        if find.has_focus() {
            self.tools.tree_focus = Some(find.id);
        }
        ui.menu_button("Tables ⋯", |ui| {
            if ui.button("Refresh schema · F5").clicked() {
                self.refresh_catalog();
                self.load_diagram();
                ui.close();
            }
        });
        egui::ScrollArea::vertical().id_salt("navigation-tree").show(ui,|ui| {
            if self.catalog_ticket.is_some(){ui.spinner();}
            if !self.catalog_error.is_empty(){ui.colored_label(ui.visuals().error_fg_color,&self.catalog_error);}
            if self.catalog.completion.as_ref().is_some_and(|c|c.delivery!=tursobench_core::Delivery::Complete||c.error.is_some()) {
                ui.colored_label(ui.visuals().warn_fg_color,"Catalog incomplete").on_hover_text("Retained metadata budget reached or catalog failed. Use SQL to inspect omitted objects.");
            }
            let filter=self.tools.schema_filter.to_lowercase();
            let related=self.ui.hover_table.as_deref().map(|t|self.related_tables(t)).unwrap_or_default();
            let mut hovered=None;
            for (kind,title,icon) in [("table","Tables",Icon::Data),("view","Views",Icon::Data),("system","System",Icon::System),("index","Indexes",Icon::Columns),("trigger","Triggers",Icon::System)] {
                let objects:Vec<_>=self.schema.iter().filter(|o| {
                    let system=o.name.starts_with("__turso_internal_");
                    (if kind=="system"{o.kind=="table"&&system}else{o.kind==kind&&!system}) && (o.name.to_lowercase().contains(&filter) || self.loaded_inspection(&o.name).is_some_and(|i| i.table.columns.iter().any(|c| c.name.to_lowercase().contains(&filter))))
                }).map(|o| o.name.clone()).collect();
                egui::CollapsingHeader::new(format!("{title} · {}",objects.len())).id_salt(("nav-group",kind)).default_open(kind=="table" || kind=="view" || !filter.is_empty()).show(ui,|ui| {
                    for name in objects {
                        if self.sidebar_object(ui, &name, if kind == "system" { "table" } else { kind }, icon, related.contains(&name)) {
                            hovered = Some(name);
                        }
                    }
                });
            }
            self.ui.hover_table=hovered;
        });
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
                .document
                .tools
                .ddl
                .as_ref()
                .is_some_and(|o| o.name == name);
            let response = tree_row(ui, name, "DDL", icon, selected, false, (22.0, 0.0));
            if response.has_focus() {
                self.tools.tree_focus = Some(response.id);
            }
            if (response.clicked()
                || response.has_focus()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)))
                && let Some(object) = self.schema.iter().find(|o| o.name == name).cloned()
            {
                if self.workspace.enabled
                    && self
                        .create_window(
                            super::tiling::Kind::Structure,
                            Some(object.name.clone()),
                            None,
                            false,
                        )
                        .is_none()
                {
                    return response.hovered();
                }
                self.ui.diagram.selected = Some(object.table.clone());
                self.document.tools.ddl = Some(object);
                self.document.ui.nav = Nav::Table;
                self.document.ui.table_view = TableView::Structure;
            }
            return response.hovered();
        }
        let mut hovered = false;
        let selected = self.document.view == View::Browse
            && self.document.ui.nav == Nav::Table
            && self.ui.diagram.selected.as_deref() == Some(name);
        let unfolded = self.ui.unfolded.as_deref() == Some(name);
        let count = self
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
            self.tools.tree_focus = Some(response.id);
        }
        if arrow.clicked()
            || response.has_focus()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
        {
            self.ui.unfolded = if unfolded {
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
            self.ui.unfolded = None;
        }
        hovered |= response.hovered();
        if unfolded {
            self.sidebar_table(ui, name);
        }
        hovered
    }

    fn sidebar_table(&mut self, ui: &mut egui::Ui, name: &str) {
        let Some(info) = self.loaded_inspection(name) else {
            if self.document.tools.inspection_ticket.is_some() {
                ui.small("Loading columns…");
            } else if !self.document.tools.inspection_error.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    &self.document.tools.inspection_error,
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
                let fk=info.foreign_keys.iter().any(|k|k.columns.iter().any(|(n,_)|n==&c.name));
                let icon=if c.primary_key>0{Icon::Key}else if fk{Icon::Link}else{Icon::Columns};
                if tree_row(ui, &c.name, &c.declared_type, icon, false, fk, (22.0, 0.0)).clicked() { column=Some(c.name.clone()); }
            }
            let mut links:Vec<(&str,String)>=info.foreign_keys.iter().map(|k|(k.target.as_str(),format!("Parent: {}",k.target))).collect();
            if let Some(graph)=&self.ui.diagram.data {
                for node in &graph.tables {
                    if node.inspection.as_ref().is_some_and(|i|i.foreign_keys.iter().any(|k|k.target==name)) {links.push((node.name.as_str(),format!("Referenced by: {}",node.name)));}
                }
            }
            for (target,label) in links {
                if ui.link(label).on_hover_text("Open table without a row filter. Select a record to follow a specific key.").clicked(){table = Some(target.to_owned());}
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
        if self.document.table.as_ref().is_none_or(|t| t.name != name)
            || self.document.view != View::Browse
        {
            let ticket = self.document.result.current;
            self.browse_table(name.into());
            if ticket == self.document.result.current {
                return;
            }
        }
        self.document.ui.nav = Nav::Table;
        self.document.ui.table_view = TableView::Data;
        self.document.tools.highlight_column = Some((name.into(), column.into()));
        self.document.tools.scroll_to_column = true;
    }

    pub(super) fn select_tool(&mut self, panel: ToolPanel) {
        if panel == ToolPanel::Settings {
            if self.document.ui.nav != Nav::Settings {
                self.ui.settings_return = Some(self.document.ui.nav);
            }
            self.document.ui.nav = Nav::Settings;
        } else {
            self.ui.sidebar = panel;
            if self.document.ui.nav == Nav::Settings {
                self.document.ui.nav = self.ui.settings_return.take().unwrap_or(
                    if self.document.view == View::Browse {
                        Nav::Table
                    } else {
                        Nav::Result
                    },
                );
            }
        }
    }

    pub(super) fn activity_rail(&mut self, ui: &mut egui::Ui) {
        for (icon, name, panel) in [
            (Icon::Data, "Tables", ToolPanel::Schema),
            (Icon::History, "History", ToolPanel::History),
            (Icon::Saved, "Saved queries", ToolPanel::Saved),
            (Icon::Checks, "Database checks", ToolPanel::Diagnostics),
        ] {
            let selected = self.ui.sidebar == panel && self.document.ui.nav != Nav::Settings;
            if icons::control(ui, icon, name, "", selected, false).clicked() {
                self.select_tool(panel);
            }
            if panel == ToolPanel::Schema {
                if icons::control(
                    ui,
                    Icon::Map,
                    "Map",
                    "",
                    self.document.ui.nav == Nav::Overview,
                    false,
                )
                .clicked()
                {
                    self.ui.sidebar = ToolPanel::Schema;
                    self.overview();
                }
                if icons::control(
                    ui,
                    Icon::Sql,
                    "SQL editor",
                    "Ctrl J",
                    self.document.ui.sql_open,
                    false,
                )
                .clicked()
                {
                    self.toggle_sql();
                }
            }
        }
        ui.add_space((ui.available_height() - 72.0).max(0.0));
        if icons::control(
            ui,
            Icon::Settings,
            "Settings",
            "Ctrl ,",
            self.document.ui.nav == Nav::Settings,
            false,
        )
        .clicked()
        {
            self.select_tool(ToolPanel::Settings);
        }
        if icons::button(ui, Icon::Help, "Help", "F1").clicked() {
            self.tools.help = true;
        }
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
    let color = if related {
        ui.visuals().hyperlink_color
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
        color,
    );
    let right = painter.layout_no_wrap(
        detail.to_owned(),
        egui::FontId::monospace(10.0),
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
        egui::FontId::proportional(12.0),
        color,
    );
    painter.galley(
        rect.right_center() - egui::vec2(right.size().x + 4.0, right.size().y / 2.0),
        right,
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(name)
}
