use super::{
    App, View,
    icons::{self, Icon},
    shell::{Nav, TableView, ToolPanel},
};
use eframe::egui;
use tursobench_core::Inspection;

impl App {
    pub(super) fn overview(&mut self) {
        self.ui.nav = Nav::Overview;
        self.ui.inspector_open = false;
        if self.ui.diagram.data.is_none() {
            self.load_diagram();
        }
    }

    pub(super) fn loaded_inspection(&self, name: &str) -> Option<Inspection> {
        self.tools
            .inspection
            .as_ref()
            .filter(|i| i.table.name == name)
            .cloned()
            .or_else(|| {
                self.ui
                    .diagram
                    .data
                    .as_ref()?
                    .tables
                    .iter()
                    .find(|n| n.name == name)?
                    .inspection
                    .clone()
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
        ui.horizontal(|ui| {
            if icons::button(ui, Icon::Back, "Switch database", "Ctrl O").clicked() {
                self.request_close();
            }
            icons::paint(
                ui.painter(),
                egui::Rect::from_min_size(ui.cursor().min, egui::vec2(18.0, 18.0)),
                Icon::Database,
                ui.visuals().selection.stroke.color,
            );
            ui.add_space(22.0);
            let name = std::path::Path::new(&self.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
                .on_hover_text(&self.path);
        });
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                egui::Button::new("Search · Ctrl K"),
            )
            .clicked()
        {
            self.open_palette();
        }
        egui::Panel::bottom("sidebar-footer").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if icons::control(
                    ui,
                    Icon::Sql,
                    "SQL editor",
                    "Ctrl J",
                    self.ui.sql_open,
                    false,
                )
                .clicked()
                {
                    self.toggle_sql();
                }
                for (icon, name, panel) in [
                    (Icon::History, "History", ToolPanel::History),
                    (Icon::Saved, "Saved queries", ToolPanel::Saved),
                    (Icon::Checks, "Database checks", ToolPanel::Diagnostics),
                    (Icon::Settings, "Settings", ToolPanel::Settings),
                ] {
                    if icons::button(ui, icon, name, "").clicked() {
                        self.ui.sidebar = panel;
                        self.ui.sidebar_open = true;
                    }
                }
                if icons::button(ui, Icon::Help, "Help", "F1").clicked() {
                    self.tools.help = true;
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("navigation-tree").show(ui,|ui| {
            ui.horizontal(|ui| {
                if icons::control(ui,Icon::Map,"Overview","",self.ui.nav==Nav::Overview,true).clicked(){self.overview();}
                if icons::button(ui,Icon::Refresh,"Refresh catalog and map","").clicked(){self.refresh_catalog();self.load_diagram();}
            });
            if self.catalog_ticket.is_some(){ui.spinner();}
            if !self.catalog_error.is_empty(){ui.colored_label(ui.visuals().error_fg_color,&self.catalog_error);}
            if self.catalog.completion.as_ref().is_some_and(|c|c.delivery!=tursobench_core::Delivery::Complete||c.error.is_some()) {
                ui.colored_label(ui.visuals().warn_fg_color,"Catalog incomplete").on_hover_text("Retained metadata budget reached or catalog failed. Use SQL to inspect omitted objects.");
            }
            let filter=self.tools.schema_filter.to_lowercase();
            ui.add(egui::TextEdit::singleline(&mut self.tools.schema_filter).hint_text("Filter objects…").desired_width(ui.available_width()));
            let related=self.ui.hover_table.as_deref().map(|t|self.related_tables(t)).unwrap_or_default();
            let mut hovered=None;
            for (kind,title,icon) in [("table","Tables",Icon::Data),("view","Views",Icon::Data),("system","System",Icon::System)] {
                let objects:Vec<_>=self.schema.iter().filter(|o| {
                    let system=o.name.starts_with("__turso_internal_");
                    (if kind=="system"{o.kind=="table"&&system}else{o.kind==kind&&!system}) && o.name.to_lowercase().contains(&filter)
                }).cloned().collect();
                egui::CollapsingHeader::new(format!("{title} · {}",objects.len())).id_salt(("nav-group",kind)).default_open(kind!="system").show(ui,|ui| {
                    for object in objects {
                        let selected=self.view==View::Browse&&self.ui.nav==Nav::Table&&self.ui.diagram.selected.as_deref()==Some(&object.name);
                        let unfolded=self.ui.unfolded.as_deref()==Some(&object.name);
                        ui.horizontal(|ui| {
                            if icons::button(ui,if unfolded{Icon::Down}else{Icon::Next},if unfolded{"Collapse table"}else{"Expand table"},"").clicked(){
                                self.ui.unfolded=if unfolded{None}else{Some(object.name.clone())};
                                if !unfolded&&self.loaded_inspection(&object.name).is_none(){self.request_inspection(object.name.clone());}
                            }
                            let display=self.tools.counts.get(&object.name).map_or_else(||object.name.clone(),|n|format!("{} · {n}",object.name));
                            let label=if related.contains(&object.name){egui::RichText::new(&display).color(ui.visuals().hyperlink_color)}else{egui::RichText::new(&display)};
                            let response=ui.add_sized([ui.available_width(),28.0],egui::Button::new(label).selected(selected).truncate()).on_hover_text(&object.name);
                            let r=egui::Rect::from_center_size(response.rect.left_center()+egui::vec2(8.0,0.0),egui::vec2(10.0,10.0));
                            // The type is available in the tooltip and AccessKit name; the leading icon marks views/system.
                            if kind!="table"{icons::paint(ui.painter(),r,icon,ui.visuals().weak_text_color());}
                            if response.hovered(){hovered=Some(object.name.clone());}
                            if response.clicked(){self.browse_table(object.name.clone());}
                        });
                        if unfolded {self.sidebar_table(ui,&object.name);}
                    }
                });
            }
            self.ui.hover_table=hovered;
        });
    }

    fn sidebar_table(&mut self, ui: &mut egui::Ui, name: &str) {
        let Some(info) = self.loaded_inspection(name) else {
            if self.tools.inspection_ticket.is_some() {
                ui.small("Loading columns…");
            } else if !self.tools.inspection_error.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, &self.tools.inspection_error);
            } else if ui.small_button("Load columns").clicked() {
                self.request_inspection(name.into());
            }
            return;
        };
        ui.indent(("columns",name),|ui| {
            for c in &info.table.columns {
                ui.horizontal(|ui| {
                    let fk=info.foreign_keys.iter().any(|k|k.columns.iter().any(|(n,_)|n==&c.name));
                    let icon=if c.primary_key>0{Icon::Key}else if fk{Icon::Link}else{Icon::Columns};
                    if icons::button(ui,icon,&format!("{} · {}",c.name,c.declared_type),"").clicked(){self.highlight_sidebar_column(name,&c.name);}
                    if ui.add(egui::Button::new(egui::RichText::new(&c.name).monospace()).frame_when_inactive(false).truncate()).on_hover_text(&c.declared_type).clicked(){self.highlight_sidebar_column(name,&c.name);}
                });
            }
            let mut links:Vec<(String,String)>=info.foreign_keys.iter().map(|k|(k.target.clone(),format!("Parent: {}",k.target))).collect();
            if let Some(graph)=&self.ui.diagram.data {
                for node in &graph.tables {
                    if node.inspection.as_ref().is_some_and(|i|i.foreign_keys.iter().any(|k|k.target==name)) {links.push((node.name.clone(),format!("Referenced by: {}",node.name)));}
                }
            }
            for (target,label) in links {
                if ui.link(label).on_hover_text("Open table without a row filter. Select a record to follow a specific key.").clicked(){self.browse_table(target);}
            }
        });
    }

    fn highlight_sidebar_column(&mut self, name: &str, column: &str) {
        if self.table.as_ref().is_none_or(|t| t.name != name) || self.view != View::Browse {
            let ticket = self.result.current;
            self.browse_table(name.into());
            if ticket == self.result.current {
                return;
            }
        }
        self.ui.nav = Nav::Table;
        self.ui.table_view = TableView::Data;
        self.tools.highlight_column = Some((name.into(), column.into()));
        self.tools.scroll_to_column = true;
    }

    pub(super) fn tool_window(&mut self, ctx: &egui::Context) {
        if !self.ui.sidebar_open || self.ui.sidebar == ToolPanel::Schema {
            return;
        }
        let mut open = true;
        let title = match self.ui.sidebar {
            ToolPanel::History => "History",
            ToolPanel::Saved => "Saved queries",
            ToolPanel::Diagnostics => "Database checks",
            ToolPanel::Settings => "Settings",
            ToolPanel::Schema => unreachable!(),
        };
        egui::Window::new(title)
            .id(egui::Id::new("database-tools"))
            .open(&mut open)
            .default_width(480.0)
            .max_width((ctx.content_rect().width() - 40.0).max(200.0))
            .max_height((ctx.content_rect().height() - 80.0).max(100.0))
            .vscroll(true)
            .show(ctx, |ui| {
                if self.ui.sidebar == ToolPanel::Diagnostics {
                    self.diagnostics_ui(ui);
                } else {
                    self.history_controls(ui);
                }
            });
        self.ui.sidebar_open = open;
    }
}
