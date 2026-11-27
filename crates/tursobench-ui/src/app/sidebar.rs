use super::{
    App, View,
    icons::{self, Icon},
    shell::{Nav, ToolPanel},
};
use eframe::egui;
use tursobench_core::Inspection;

impl App {
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
        let graph = self.database.ui.diagram.data.iter().flat_map(|g| &g.tables);
        let keys = graph.filter_map(|node| Some((node, &node.inspection.as_ref()?.foreign_keys)));
        keys.flat_map(|(node, keys)| {
            keys.iter().filter_map(move |fk| {
                if node.name == name {
                    Some(fk.target.clone())
                } else {
                    (fk.target == name).then(|| node.name.clone())
                }
            })
        })
        .collect()
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
                self.available_databases_ui(ui);
            });
    }

    pub(super) fn active_database_tree_ui(&mut self, ui: &mut egui::Ui) {
        match self.database.ui.sidebar {
            ToolPanel::Schema => self.schema_tree_ui(ui),
            ToolPanel::Diagnostics => self.diagnostics_ui(ui),
            ToolPanel::Saved => self.saved_queries_ui(ui),
            ToolPanel::History => self.query_history_ui(ui),
        }
    }

    pub(super) fn schema_tree_ui(&mut self, ui: &mut egui::Ui) {
        self.catalog_status(ui);
        let filter = if self.sidebar_search.filters_tree() {
            self.sidebar_search.query.trim().to_lowercase()
        } else {
            String::new()
        };
        let related = self
            .database
            .ui
            .hover_table
            .as_deref()
            .map(|name| self.related_tables(name))
            .unwrap_or_default();
        let mut hovered = None;
        for (kind, title, icon) in [
            ("table", "Tables", Icon::Data),
            ("view", "Views", Icon::Data),
            ("system", "System", Icon::System),
            ("index", "Indexes", Icon::Columns),
            ("trigger", "Triggers", Icon::System),
        ] {
            let objects = self.tree_objects(kind, &filter);
            if !filter.is_empty() && objects.is_empty() {
                continue;
            }
            let row_kind = if kind == "system" { "table" } else { kind };
            let group = egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} · {}", title.to_uppercase(), objects.len()))
                    .size(crate::appearance::text_size(ui.ctx(), 10.5))
                    .strong()
                    .color(crate::appearance::Tokens::of(ui.ctx()).muted),
            )
            .id_salt(("nav-group", kind))
            .open((!filter.is_empty()).then_some(true))
            .default_open(kind == "table" || kind == "view")
            .show(ui, |ui| {
                // Render every row and keep the last hovered one.
                objects
                    .into_iter()
                    .filter(|name| {
                        self.sidebar_object(ui, name, row_kind, icon, related.contains(name))
                    })
                    .last()
            });
            hovered = group.body_returned.flatten().or(hovered);
        }
        self.database.ui.hover_table = hovered;
    }

    fn catalog_status(&self, ui: &mut egui::Ui) {
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
    }

    /// The names in one tree group whose name, or a loaded column, matches `filter`.
    fn tree_objects(&self, group: &str, filter: &str) -> Vec<String> {
        let matches = |name: &str| {
            super::commands::fuzzy(name, filter)
                || self.loaded_inspection(name).is_some_and(|i| {
                    i.table
                        .columns
                        .iter()
                        .any(|c| super::commands::fuzzy(&format!("{name}.{}", c.name), filter))
                })
        };
        self.database
            .schema
            .iter()
            .filter(|o| {
                let system = o.name.starts_with("__turso_internal_");
                let in_group = if group == "system" {
                    o.kind == "table" && system
                } else {
                    o.kind == group && !system
                };
                in_group && matches(&o.name)
            })
            .map(|o| o.name.clone())
            .collect()
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
            return self.sidebar_definition(ui, name, icon);
        }
        let (selected, unfolded) = self.tree_object_state(name);
        let counted = self.database.tools.counts.get(name);
        let count = counted.map_or("—".into(), |n| n.to_string());
        let response = tree_row(ui, name, &count, icon, selected, related, (26.0, 22.0))
            .on_hover_text(match counted {
                Some(rows) => format!("Click to browse · the arrow lists columns\n{rows} rows counted"),
                None => "Click to browse · the arrow lists columns\n— rows not counted yet: Σ in Table data counts them".into(),
            });
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
        if arrow.clicked() || focused_key(ui, &response, egui::Key::ArrowRight) {
            self.database.ui.unfolded = if unfolded {
                None
            } else {
                Some(name.to_owned())
            };
            if !unfolded && self.loaded_inspection(name).is_none() {
                self.request_inspection(name.to_owned());
            }
        } else if response.clicked() || focused_key(ui, &response, egui::Key::Enter) {
            self.activate_table(name.to_owned());
        }
        if focused_key(ui, &response, egui::Key::ArrowLeft) {
            self.database.ui.unfolded = None;
        }
        if unfolded {
            self.sidebar_table(ui, name);
        }
        response.hovered()
    }
    /// Whether table `name`'s row is selected and its columns unfolded, by
    /// browsing, the disclosure arrow or a search of this database.
    fn tree_object_state(&self, name: &str) -> (bool, bool) {
        let search = &self.sidebar_search;
        let filtering = search.filters_tree();
        let keyboard = search
            .tree_highlight
            .as_ref()
            .filter(|(table, _)| filtering && table == name)
            .map(|(_, column)| column.is_some());
        let browsed = self.database.document.view == View::Browse
            && self.database.document.ui.nav == Nav::Table
            && self.database.ui.diagram.selected.as_deref() == Some(name);
        let matched_by_column = filtering
            && !super::commands::fuzzy(name, &search.query)
            && self.loaded_inspection(name).is_some();
        (
            keyboard == Some(false) || browsed,
            self.database.ui.unfolded.as_deref() == Some(name)
                || keyboard == Some(true)
                || matched_by_column,
        )
    }
    /// An index or trigger: a row that shows its definition in Structure.
    fn sidebar_definition(&mut self, ui: &mut egui::Ui, name: &str, icon: Icon) -> bool {
        let tools = &self.database.document.tools;
        let selected = tools.ddl.as_ref().is_some_and(|o| o.name == name);
        let response = tree_row(ui, name, "DDL", icon, selected, false, (22.0, 0.0))
            .on_hover_text("Click to show its definition in Structure");
        if response.has_focus() {
            self.database.tools.tree_focus = Some(response.id);
        }
        if response.clicked() || focused_key(ui, &response, egui::Key::Enter) {
            self.show_definition(name);
        }
        response.hovered()
    }
    fn sidebar_table(&mut self, ui: &mut egui::Ui, name: &str) {
        let Some(info) = self.loaded_inspection(name) else {
            let tools = &self.database.document.tools;
            if tools.inspection_ticket.is_some() {
                ui.small("Loading columns…");
            } else if !tools.inspection_error.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, &tools.inspection_error);
            } else if ui.small_button("Load columns").clicked() {
                self.request_inspection(name.into());
            }
            return;
        };
        let search = &self.sidebar_search;
        let filter = search.filters_tree().then_some(search.query.as_str());
        let highlight = filter
            .and(search.tree_highlight.as_ref())
            .filter(|(table, _)| table == name)
            .and_then(|(_, column)| column.as_deref());
        let mut table = None;
        let column = ui.indent(("columns", name), |ui| {
            let column = column_rows(ui, info, name, filter, highlight);
            let color = crate::appearance::Tokens::of(ui.ctx()).relationship;
            for (target, label) in table_links(info, self.database.ui.diagram.data.as_ref(), name) {
                if ui
                    .link(egui::RichText::new(label).color(color))
                    .on_hover_text(
                        "Open table without a row filter. Select a record to follow a specific key.",
                    )
                    .clicked()
                {
                    table = Some(target.to_owned());
                }
            }
            column
        });
        if let Some(column) = column.inner {
            self.show_table(name.to_owned(), Some(column));
        }
        if let Some(table) = table {
            self.browse_table(table);
        }
    }

    pub(super) fn select_tool(&mut self, panel: ToolPanel) {
        if self.database.session.is_none() && panel != ToolPanel::Schema {
            self.database.status = "Open a database to use this tool".into();
            return;
        }
        self.database.ui.sidebar = panel;
    }

    /// Tables, History, Saved queries and Checks belong to the selected database,
    /// so they sit directly under its row and exist only while it is open.
    pub(super) fn database_tools_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (icon, name, panel) in [
                (Icon::Data, "Tables", ToolPanel::Schema),
                (Icon::History, "History", ToolPanel::History),
                (Icon::Saved, "Saved queries", ToolPanel::Saved),
                (Icon::Checks, "Database checks", ToolPanel::Diagnostics),
            ] {
                if icons::control(ui, icon, name, "", panel == self.database.ui.sidebar, false)
                    .clicked()
                {
                    self.select_tool(panel);
                }
            }
        });
    }

    /// The foot of the sidebar holds application-wide controls only: icons, named on hover.
    pub(super) fn application_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if icons::control(
                ui,
                Icon::Settings,
                "Settings",
                "Ctrl ,",
                self.settings.open,
                false,
            )
            .clicked()
            {
                self.settings.open = !self.settings.open;
            }
            if icons::control(
                ui,
                Icon::Command,
                "Commands",
                "Ctrl K",
                self.database.ui.palette_open,
                false,
            )
            .on_hover_text("Common actions by name")
            .clicked()
            {
                self.toggle_palette();
            }
            if icons::control(
                ui,
                Icon::Help,
                "Keyboard and safety reference",
                "F1",
                self.database.tools.help,
                false,
            )
            .clicked()
            {
                self.database.tools.help = !self.database.tools.help;
            }
        });
    }
}

/// Whether the focused row's key was pressed, consuming it.
fn focused_key(ui: &egui::Ui, response: &egui::Response, key: egui::Key) -> bool {
    response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
}
/// A row for each of table `name`'s columns that `filter` keeps, with
/// `highlight` selected; returns the clicked column.
fn column_rows(
    ui: &mut egui::Ui,
    info: &Inspection,
    name: &str,
    filter: Option<&str>,
    highlight: Option<&str>,
) -> Option<String> {
    let mut clicked = None;
    for c in &info.table.columns {
        let full = format!("{name}.{}", c.name);
        if filter
            .is_some_and(|q| !super::commands::fuzzy(name, q) && !super::commands::fuzzy(&full, q))
        {
            continue;
        }
        let fk = info
            .foreign_keys
            .iter()
            .any(|k| k.columns.iter().any(|(n, _)| n == &c.name));
        let icon = if c.primary_key > 0 {
            Icon::Key
        } else if fk {
            Icon::Link
        } else {
            Icon::Columns
        };
        let selected = highlight == Some(c.name.as_str());
        if tree_row(
            ui,
            &c.name,
            &c.declared_type,
            icon,
            selected,
            fk,
            (22.0, 0.0),
        )
        .on_hover_text("Click to find this column in Table data")
        .clicked()
        {
            clicked = Some(c.name.clone());
        }
    }
    clicked
}
/// The tables a table links to: its parents, then the tables that reference it.
fn table_links<'a>(
    info: &'a Inspection,
    graph: Option<&'a tursobench_core::SchemaDiagram>,
    name: &str,
) -> Vec<(&'a str, String)> {
    let parents = info
        .foreign_keys
        .iter()
        .map(|k| (k.target.as_str(), format!("Parent: {}", k.target)));
    let children = graph.into_iter().flat_map(|g| &g.tables).filter(|node| {
        node.inspection
            .as_ref()
            .is_some_and(|i| i.foreign_keys.iter().any(|k| k.target == name))
    });
    parents
        .chain(children.map(|node| (node.name.as_str(), format!("Referenced by: {}", node.name))))
        .collect()
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
