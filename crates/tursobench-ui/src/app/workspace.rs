use super::{
    App, View,
    document::Document,
    icons::{self, Icon},
    shell::{DetailMode, Nav, TableView},
    tiling::{self, Kind, Node},
};
use eframe::egui;
use std::collections::BTreeMap;

/// Sixteen retained views reserve at most 128 MiB of engine row payloads (8 MiB
/// per view), in addition to existing bounded metadata, context and draft caches.
pub(super) const MAX_VIEWS: usize = 16;
pub(super) struct Workspace {
    pub enabled: bool,
    pub tree: Option<Node>,
    pub focused: u64,
    pub expanded: Option<u64>,
    pub next_id: u64,
    pub area: egui::Rect,
    pub close_pending: Option<u64>,
    #[cfg(test)]
    pub tiles: Vec<(u64, egui::Rect)>,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            enabled: false,
            tree: None,
            focused: 0,
            expanded: None,
            next_id: 1,
            area: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0)),
            close_pending: None,
            #[cfg(test)]
            tiles: vec![],
        }
    }
}
impl App {
    pub(super) fn document_ref(&self, id: u64) -> Option<&Document> {
        if self.document.id == id {
            Some(&self.document)
        } else {
            self.documents.get(&id)
        }
    }
    pub(super) fn window_ids(&self) -> Vec<u64> {
        self.workspace
            .tree
            .as_ref()
            .map_or_else(Vec::new, Node::ids)
    }
    pub(super) fn find_window(
        &self,
        kind: Kind,
        object: Option<&str>,
        source: Option<u64>,
    ) -> Option<u64> {
        self.window_ids().into_iter().find(|id| {
            self.document_ref(*id).is_some_and(|d| {
                d.kind == kind && d.object.as_deref() == object && d.source == source
            })
        })
    }
    pub(super) fn focus_window(&mut self, id: u64) {
        if !self.switch_document(id) {
            return;
        }
        self.workspace.focused = id;
        if let Some(tree) = &mut self.workspace.tree {
            tree.select(id);
        }
        if self.workspace.expanded.is_some() {
            self.workspace.expanded = Some(id);
        }
    }
    fn profiles(&self) -> BTreeMap<u64, (egui::Vec2, f32)> {
        self.documents
            .values()
            .chain(std::iter::once(&self.document))
            .map(|d| {
                let minimum = d.kind.minimum();
                let columns = d.tools.columns.widths.iter().sum::<f32>() + 60.0;
                let preferred = match d.kind {
                    Kind::Data | Kind::Result => columns.clamp(480.0, 1000.0),
                    Kind::Sql => 600.0,
                    _ => minimum.x,
                };
                (d.id, (minimum, preferred))
            })
            .collect()
    }
    pub(super) fn create_window(
        &mut self,
        kind: Kind,
        object: Option<String>,
        source: Option<u64>,
        duplicate: bool,
    ) -> Option<u64> {
        if !duplicate && let Some(id) = self.find_window(kind, object.as_deref(), source) {
            self.focus_window(id);
            self.workspace.expanded = None;
            return Some(id);
        }
        let ids = self.window_ids();
        if ids.len() >= MAX_VIEWS {
            self.status = format!(
                "At most {MAX_VIEWS} retained views; close a window before opening another"
            );
            return None;
        }
        let id = self.workspace.next_id;
        self.workspace.next_id += 1;
        let mut next = Document {
            id,
            kind,
            object,
            source,
            ..Default::default()
        };
        next.ui.nav = match kind {
            Kind::Map => Nav::Overview,
            Kind::Data | Kind::Structure | Kind::Record => Nav::Table,
            _ => Nav::Result,
        };
        next.ui.table_view = if kind == Kind::Structure {
            TableView::Structure
        } else {
            TableView::Data
        };
        next.ui.sql_open = kind == Kind::Sql;
        let old = std::mem::replace(&mut self.document, next);
        if ids.contains(&old.id) {
            self.documents.insert(old.id, old);
        }
        let profiles = self.profiles();
        if let Some(tree) = &mut self.workspace.tree {
            tree.insert(
                id,
                self.workspace.focused,
                self.workspace.area,
                &|id| profiles[&id].0,
                &|id| profiles[&id].1,
            );
        } else {
            self.workspace.tree = Some(Node::leaf(id));
        }
        self.workspace.focused = id;
        self.workspace.expanded = None;
        Some(id)
    }
    pub(super) fn open_structure_window(&mut self) {
        let name = self
            .document
            .object
            .clone()
            .or_else(|| self.document.table.as_ref().map(|t| t.name.clone()))
            .or_else(|| self.ui.diagram.selected.clone());
        let Some(name) = name else {
            self.status = "Choose a table before opening its structure".into();
            return;
        };
        if self
            .create_window(Kind::Structure, Some(name.clone()), None, false)
            .is_some()
        {
            self.request_inspection(name);
        }
    }
    pub(super) fn open_sql_window(&mut self, duplicate: bool) {
        self.create_window(Kind::Sql, None, None, duplicate);
    }
    pub(super) fn open_record_window(&mut self, row: usize, column: usize) {
        let Some(values) = self.document.result.rows.get(row).cloned() else {
            return;
        };
        let columns = self.document.result.columns.clone();
        let object = self
            .document
            .object
            .clone()
            .or_else(|| self.document.table.as_ref().map(|t| t.name.clone()));
        let source = self.document.id;
        if self
            .create_window(Kind::Record, object, Some(source), true)
            .is_some()
        {
            self.document.result.columns = columns;
            self.document.result.rows = vec![values];
            self.document.inspect = Some((0, column));
            self.document.tools.detail = Some(0);
            self.document.ui.inspector = DetailMode::Record;
            self.document.view = View::Query; // Pinned snapshots never stage edits against a different result.
        }
    }
    pub(super) fn expand_window(&mut self, id: u64) {
        self.focus_window(id);
        self.workspace.expanded = if self.workspace.expanded == Some(id) {
            None
        } else {
            Some(id)
        };
    }
    pub(super) fn retile(&mut self) {
        let profiles = self.profiles();
        let mut ids = self.window_ids();
        ids.sort_by_key(|id| match self.document_ref(*id).unwrap().kind {
            Kind::Data | Kind::Result => 0,
            Kind::Sql => 1,
            Kind::Structure => 2,
            Kind::Map => 3,
            Kind::Record => 4,
        });
        self.workspace.tree = ids.first().map(|id| Node::leaf(*id));
        for pair in ids.windows(2) {
            self.workspace.tree.as_mut().unwrap().insert(
                pair[1],
                pair[0],
                self.workspace.area,
                &|id| profiles[&id].0,
                &|id| profiles[&id].1,
            );
        }
        if let Some(tree) = &mut self.workspace.tree {
            tree.select(self.workspace.focused);
        }
        self.workspace.expanded = None;
    }
    pub(super) fn request_close_window(&mut self, id: u64) {
        if let Some(d) = self.document_ref(id) {
            if self.commit_owner == Some(id) && self.commit_ticket.is_some() {
                self.status =
                    "Wait for the staged commit outcome before closing this window".into();
                return;
            }
            if d.has_drafts()
                || d.active.is_some()
                || d.kind == Kind::Sql && !d.sql.trim().is_empty()
            {
                self.workspace.close_pending = Some(id);
                return;
            }
        }
        self.close_window(id);
    }
    fn close_window(&mut self, id: u64) {
        if let Some(d) = self.document_ref(id)
            && let Some(session) = &self.session
        {
            for ticket in [
                d.active,
                d.tools.inspection_ticket,
                d.tools
                    .pages
                    .pending
                    .as_ref()
                    .and_then(|p| p.result.current),
                d.tools
                    .context
                    .pending
                    .as_ref()
                    .and_then(|p| p.result.current),
            ]
            .into_iter()
            .flatten()
            {
                session.discard_results(ticket.request);
            }
        }
        self.workspace.tree = self.workspace.tree.take().and_then(|tree| tree.remove(id));
        self.workspace.close_pending = None;
        if self.workspace.expanded == Some(id) {
            self.workspace.expanded = None;
        }
        if self.document.id == id {
            if let Some(next) = self.window_ids().first().copied() {
                self.focus_window(next);
            } else {
                self.document = Document::default();
            }
        }
        self.documents.remove(&id);
        if self.workspace.focused == id
            && let Some(next) = self.window_ids().first().copied()
        {
            self.focus_window(next);
        }
    }
    pub(super) fn window_close_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.workspace.close_pending else {
            return;
        };
        let title = self
            .document_ref(id)
            .map_or_else(String::new, Document::title);
        egui::Window::new("Close view?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).show(ctx,|ui|{
            ui.strong(title);
            ui.label("Closing discards this view’s local edits and unsaved SQL. Any running SQL continues; closing a view does not roll back a transaction.");
            if ui.button("Close and discard this view").clicked(){self.close_window(id);}
            if ui.button("Keep view").clicked(){self.workspace.close_pending=None;}
        });
    }
    pub(super) fn window_navigation(&mut self, ui: &mut egui::Ui) {
        ui.strong("Windows");
        egui::ScrollArea::vertical()
            .id_salt("workspace-window-list")
            .show(ui, |ui| {
                for id in self.window_ids() {
                    let Some(d) = self.document_ref(id) else {
                        continue;
                    };
                    let title = d.title();
                    let dirty = d.has_drafts();
                    if ui
                        .add(
                            egui::Button::new(format!(
                                "{}{}",
                                title,
                                if dirty { " · staged" } else { "" }
                            ))
                            .selected(id == self.workspace.focused)
                            .truncate(),
                        )
                        .on_hover_text(title)
                        .clicked()
                    {
                        self.focus_window(id);
                    }
                }
            });
    }
    pub(super) fn tiled_workspace(&mut self, ui: &mut egui::Ui) {
        self.workspace.area = ui.available_rect_before_wrap();
        #[cfg(test)]
        {
            self.workspace.tiles.clear();
        }
        let Some(tree) = &self.workspace.tree else {
            ui.label("Open a table, Map or SQL from the right navigator.");
            return;
        };
        let profiles = self.profiles();
        let (tiles, dividers) = if let Some(id) = self.workspace.expanded {
            (
                vec![tiling::Tile {
                    ids: vec![id],
                    selected: id,
                    rect: self.workspace.area,
                }],
                vec![],
            )
        } else {
            tree.layout(self.workspace.area, self.workspace.focused, &|id| {
                profiles[&id].0
            })
        };
        for divider in dividers {
            let response = ui
                .interact(
                    divider.rect,
                    ui.id().with(("split", &divider.path)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(if divider.horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Resize tiled views")
            });
            let mut ratio = divider.ratio;
            if response.dragged()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let axis = usize::from(!divider.horizontal);
                ratio = (pos[axis] - divider.parent.min[axis])
                    / (divider.parent.size()[axis] - tiling::GAP);
            }
            if response.has_focus() {
                ratio += ui.input(|i| {
                    if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowDown) {
                        0.03
                    } else if i.key_pressed(egui::Key::ArrowLeft)
                        || i.key_pressed(egui::Key::ArrowUp)
                    {
                        -0.03
                    } else {
                        0.0
                    }
                });
            }
            if ratio != divider.ratio
                && let Some(tree) = &mut self.workspace.tree
            {
                tree.resize(
                    &divider.path,
                    ratio.clamp(*divider.range.start(), *divider.range.end()),
                );
            }
            if response.hovered() || response.has_focus() {
                ui.painter()
                    .rect_filled(divider.rect, 2.0, ui.visuals().selection.bg_fill);
            }
        }
        // Render focused last: retained keyboard/dialog state and diagnostics belong to it.
        let mut tiles = tiles;
        tiles.sort_by_key(|t| t.selected == self.workspace.focused);
        for tile in tiles {
            let id = tile.selected;
            if !self.switch_document(id) {
                continue;
            }
            #[cfg(test)]
            {
                self.workspace.tiles.push((id, tile.rect));
            }
            if ui.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| tile.rect.contains(p))
            }) {
                self.focus_window(id);
            }
            let focused = self.workspace.focused == id;
            let stroke = if focused {
                ui.visuals().selection.stroke
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            };
            ui.painter()
                .rect_filled(tile.rect, 3.0, ui.visuals().panel_fill);
            ui.painter()
                .rect_stroke(tile.rect, 3.0, stroke, egui::StrokeKind::Inside);
            let content = tile.rect.shrink(5.0);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(("window", id))
                    .max_rect(content),
                |ui| {
                    ui.set_clip_rect(content.intersect(ui.clip_rect()));
                    ui.set_min_size(content.size());
                    ui.set_max_size(content.size());
                    if tile.ids.len() > 1 {
                        egui::ScrollArea::horizontal()
                            .id_salt("group-tabs")
                            .max_height(31.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for tab in tile.ids {
                                        let label = self
                                            .document_ref(tab)
                                            .map_or_else(String::new, Document::title);
                                        if ui.selectable_label(tab == id, label).clicked() {
                                            self.focus_window(tab);
                                        }
                                    }
                                });
                            });
                        // A tab click affects the next frame, not the content that was laid out for this one.
                        self.switch_document(id);
                    }
                    let header = ui.horizontal(|ui| {
                        ui.add_sized(
                            [(ui.available_width() - 78.0).max(20.0), 28.0],
                            egui::Label::new(egui::RichText::new(self.document.title()).strong())
                                .truncate(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, Icon::Close, "Close view", "").clicked() {
                                self.request_close_window(id);
                            }
                            if icons::button(
                                ui,
                                Icon::Fit,
                                if self.workspace.expanded.is_some() {
                                    "Restore tiles"
                                } else {
                                    "Expand view"
                                },
                                "Ctrl Alt Enter",
                            )
                            .clicked()
                            {
                                self.expand_window(id);
                            }
                        });
                    });
                    #[cfg(test)]
                    {
                        if focused {
                            self.ui.view_header = header.response.rect;
                        }
                    }
                    if header
                        .response
                        .interact(egui::Sense::click())
                        .double_clicked()
                    {
                        self.expand_window(id);
                    }
                    ui.separator();
                    // A close action may already have removed this document.
                    if self.document.id != id {
                        return;
                    }
                    let body_rect = egui::Rect::from_min_max(
                        egui::pos2(content.left(), ui.cursor().top().min(content.bottom())),
                        content.max,
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new().id_salt("body").max_rect(body_rect),
                        |ui| {
                            ui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
                            match self.document.kind {
                                Kind::Map => self.diagram_ui(ui),
                                Kind::Structure => {
                                    egui::ScrollArea::both()
                                        .id_salt("structure-scroll")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| self.structure_inspector(ui));
                                }
                                Kind::Sql => self.editor_ui(ui),
                                Kind::Record => {
                                    egui::ScrollArea::both()
                                        .id_salt("record-scroll")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            if let Some(source) = self.document.source
                                                && ui.button("Focus source view").clicked()
                                            {
                                                self.focus_window(source);
                                                return;
                                            }
                                            ui.horizontal(|ui| {
                                                ui.selectable_value(
                                                    &mut self.document.ui.inspector,
                                                    DetailMode::Record,
                                                    "Record",
                                                );
                                                ui.selectable_value(
                                                    &mut self.document.ui.inspector,
                                                    DetailMode::Cell,
                                                    "Cell",
                                                );
                                            });
                                            self.inspector_content(ui);
                                        });
                                }
                                Kind::Data | Kind::Result => {
                                    if self.document.ui.filter_open
                                        || self.document.tools.filter_focus
                                    {
                                        self.filter_controls(ui);
                                    }
                                    self.grid_contents(ui);
                                }
                            }
                        },
                    );
                },
            );
        }
        self.switch_document(self.workspace.focused);
    }
}
