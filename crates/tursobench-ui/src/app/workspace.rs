//! Fixed visible panes over bounded, independently owned request/result buffers.
use super::{
    App, View,
    document::Document,
    icons::{self, Icon},
    shell::{DetailMode, Nav, TableView},
    tiling::{self, Kind},
};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
pub(super) const MAX_VIEWS: usize = 16;
pub(super) struct Workspace {
    pub enabled: bool,
    pub focused: u64,
    pub active: Kind,
    pub expanded: Option<Kind>,
    pub splits: tiling::Splits,
    pub minimized: BTreeSet<Kind>,
    pub visible: BTreeMap<Kind, u64>,
    pub next_id: u64,
    pub area: egui::Rect,
    #[cfg(test)]
    pub tiles: Vec<(u64, egui::Rect)>,
    #[cfg(test)]
    pub pane_rects: Vec<(Kind, egui::Rect)>,
    #[cfg(test)]
    pub controls: Vec<(Kind, egui::Rect, egui::Rect)>,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            enabled: false,
            focused: 0,
            active: Kind::Map,
            expanded: None,
            splits: Default::default(),
            minimized: Default::default(),
            visible: Default::default(),
            next_id: 1,
            area: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0)),
            #[cfg(test)]
            tiles: vec![],
            #[cfg(test)]
            pane_rects: vec![],
            #[cfg(test)]
            controls: vec![],
        }
    }
}
impl App {
    pub(super) fn document_ref(&self, id: u64) -> Option<&Document> {
        if self.database.document.id == id {
            Some(&self.database.document)
        } else {
            self.database.documents.get(&id)
        }
    }
    pub(super) fn window_ids(&self) -> Vec<u64> {
        self.database
            .documents
            .keys()
            .copied()
            .chain(std::iter::once(self.database.document.id))
            .collect()
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
    pub(super) fn pane_document(&self, kind: Kind) -> Option<u64> {
        match kind {
            Kind::Record => self.database.workspace.visible.get(&Kind::Data).copied(),
            Kind::Structure => self
                .database
                .workspace
                .visible
                .get(&Kind::Structure)
                .or(self.database.workspace.visible.get(&Kind::Data))
                .copied(),
            _ => self.database.workspace.visible.get(&kind).copied(),
        }
    }
    pub(super) fn focus_pane(&mut self, kind: Kind) {
        self.database.workspace.active = kind.pane();
        self.database.workspace.minimized.remove(&kind.pane());
        if self.database.workspace.expanded.is_some() {
            self.database.workspace.expanded = Some(kind.pane());
        }
        if let Some(id) = self.pane_document(kind) {
            self.switch_document(id);
            self.database.workspace.focused = id;
        }
    }
    pub(super) fn focus_window(&mut self, id: u64) {
        if !self.switch_document(id) {
            return;
        }
        let kind = self.database.document.kind;
        self.database.workspace.visible.insert(kind, id);
        // Structure follows the newly selected table until a separate schema object is inspected.
        if kind == Kind::Data {
            self.database.workspace.visible.remove(&Kind::Structure);
        }
        if kind == Kind::Result
            && let Some(source) = self.database.document.source
            && self
                .document_ref(source)
                .is_some_and(|d| d.kind == Kind::Sql)
        {
            self.database.workspace.visible.insert(Kind::Sql, source);
        }
        self.database.workspace.focused = id;
        self.database.workspace.active = kind.pane();
        self.database.workspace.minimized.remove(&kind.pane());
        if self.database.workspace.expanded.is_some() {
            self.database.workspace.expanded = Some(kind.pane());
        }
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
            return Some(id);
        }
        if self.window_ids().len() >= MAX_VIEWS {
            // Evict only idle, hidden, clean results; never discard an editor or draft.
            let victim = self
                .database
                .documents
                .iter()
                .find(|(id, d)| {
                    !self
                        .database
                        .workspace
                        .visible
                        .values()
                        .any(|shown| shown == *id)
                        && !d.has_drafts()
                        && d.active.is_none()
                        && d.tools.inspection_ticket.is_none()
                        && d.tools.pages.pending.is_none()
                        && d.tools.context.pending.is_none()
                        && d.kind != Kind::Sql
                })
                .map(|(id, _)| *id);
            if let Some(id) = victim {
                self.database.documents.remove(&id);
            } else {
                self.database.status="All retained views are busy or contain unsaved SQL; finish an operation before opening another".into();
                return None;
            }
        }
        let id = self.database.workspace.next_id;
        self.database.workspace.next_id += 1;
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
        if kind == Kind::Sql {
            next.sql.clear();
        }
        let previous = std::mem::replace(&mut self.database.document, next);
        self.database.documents.insert(previous.id, previous);
        self.focus_window(id);
        Some(id)
    }
    pub(super) fn initialize_workspace(&mut self) {
        self.database.workspace.enabled = true;
        self.database.document.kind = Kind::Map;
        self.database
            .workspace
            .visible
            .insert(Kind::Map, self.database.document.id);
        self.database.workspace.focused = self.database.document.id;
        self.open_sql_window(false);
        self.focus_pane(Kind::Map);
    }
    pub(super) fn open_structure_window(&mut self) {
        if self.database.document.kind == Kind::Data {
            self.database.workspace.visible.remove(&Kind::Structure);
            if let Some(name) = self.database.document.object.clone() {
                self.request_inspection(name);
            }
            self.focus_pane(Kind::Structure);
            return;
        }
        let Some(name) = self.database.document.object.clone().or(self
            .database
            .ui
            .diagram
            .selected
            .clone())
        else {
            self.focus_pane(Kind::Structure);
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
        if !duplicate && let Some(id) = self.pane_document(Kind::Sql) {
            self.focus_window(id);
            return;
        }
        self.create_window(Kind::Sql, None, None, duplicate);
    }
    pub(super) fn open_record_window(&mut self, row: usize, column: usize) {
        if self.database.document.result.rows.get(row).is_none() {
            return;
        }
        self.database.document.inspect = Some((row, column));
        self.database.document.tools.detail = Some(row);
        self.database.document.ui.inspector = DetailMode::Record;
        if self.database.document.kind == Kind::Data {
            self.focus_pane(Kind::Record);
        } else {
            self.database.document.ui.inspector_open = true;
            self.database.document.ui.inspector_detached = true;
        }
    }
    pub(super) fn expand_window(&mut self, id: u64) {
        let kind = if id == self.database.workspace.focused {
            self.database.workspace.active
        } else {
            self.document_ref(id).map_or(Kind::Data, |d| d.kind.pane())
        };
        self.expand_pane(kind);
    }
    pub(super) fn expand_pane(&mut self, kind: Kind) {
        let restore = self.database.workspace.expanded == Some(kind);
        self.focus_pane(kind);
        self.database.workspace.expanded = if restore { None } else { Some(kind) };
    }
    pub(super) fn minimize_pane(&mut self, kind: Kind) {
        self.database.workspace.minimized.insert(kind);
        if self.database.workspace.expanded == Some(kind) {
            self.database.workspace.expanded = None;
        }
    }
    pub(super) fn retile(&mut self) {
        self.database.workspace.minimized.clear();
        self.database.workspace.expanded = None;
    }
    pub(super) fn tiled_workspace(&mut self, ui: &mut egui::Ui) {
        self.database.workspace.area = ui.available_rect_before_wrap();
        #[cfg(test)]
        {
            self.database.workspace.tiles.clear();
            self.database.workspace.pane_rects.clear();
            self.database.workspace.controls.clear();
        }
        let mut panes = tiling::layout(
            self.database.workspace.area,
            self.database.workspace.expanded,
            &self.database.workspace.minimized,
            &self.database.workspace.splits,
        );
        if self.workspace_dividers(ui, &panes) {
            panes = tiling::layout(
                self.database.workspace.area,
                self.database.workspace.expanded,
                &self.database.workspace.minimized,
                &self.database.workspace.splits,
            );
        }
        for (kind, rect) in panes {
            let owner = self.pane_document(kind);
            if let Some(id) = owner {
                self.switch_document(id);
            }
            if !self.database.workspace.minimized.contains(&kind)
                && ui.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
                })
            {
                self.focus_pane(kind);
            }
            #[cfg(test)]
            {
                self.database.workspace.pane_rects.push((kind, rect));
                if let Some(id) = owner {
                    self.database.workspace.tiles.push((id, rect));
                }
            }
            let focused = self.database.workspace.active == kind;
            let tokens = crate::appearance::Tokens::of(ui.ctx());
            ui.painter()
                .rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);
            let header = egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.right(), (rect.top() + tiling::BAR).min(rect.bottom())),
            );
            ui.painter().rect_filled(
                header,
                egui::CornerRadius {
                    nw: 10,
                    ne: 10,
                    sw: 0,
                    se: 0,
                },
                tokens.raised,
            );
            ui.painter().rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(
                    1.0,
                    if focused {
                        tokens.primary.gamma_multiply(0.5)
                    } else {
                        tokens.line
                    },
                ),
                egui::StrokeKind::Inside,
            );
            if focused {
                ui.painter().hline(
                    rect.x_range().shrink(8.0),
                    rect.top() + 1.0,
                    egui::Stroke::new(2.0, tokens.primary),
                );
            }
            let content = rect.shrink(4.0);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(("pane", kind))
                    .max_rect(content),
                |ui| {
                    ui.set_clip_rect(content.intersect(ui.clip_rect()));
                    ui.set_min_size(content.size());
                    ui.set_max_size(content.size());
                    if self.database.workspace.minimized.contains(&kind) {
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized(
                                    [
                                        (ui.available_width()
                                            - icons::RIBBON_BUTTON_SIZE
                                            - ui.spacing().item_spacing.x)
                                            .max(0.0),
                                        icons::RIBBON_BUTTON_SIZE,
                                    ],
                                    egui::Button::new(
                                        egui::RichText::new(kind.label())
                                            .size(crate::appearance::text_size(
                                                ui.ctx(),
                                                icons::RIBBON_TEXT_SIZE,
                                            ))
                                            .strong(),
                                    )
                                    .frame(false)
                                    .truncate(),
                                )
                                .clicked()
                            {
                                self.focus_pane(kind);
                            }
                            if icons::ribbon_button(ui, Icon::Fit, "Expand to workspace", "", None)
                                .clicked()
                            {
                                self.expand_pane(kind);
                            }
                        });
                        return;
                    }
                    self.pane_ribbon(ui, kind, owner.is_some());
                    if self.database.workspace.minimized.contains(&kind) {
                        return;
                    }
                    let divider_y = ui.cursor().top().min(content.bottom());
                    ui.painter().hline(
                        content.x_range(),
                        divider_y,
                        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                    ui.add_space(5.0);
                    let body = egui::Rect::from_min_max(
                        egui::pos2(content.left(), ui.cursor().top().min(content.bottom())),
                        content.max,
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new().id_salt("body").max_rect(body),
                        |ui| {
                            ui.set_clip_rect(body.intersect(ui.clip_rect()));
                            ui.set_max_size(body.size());
                            if let Some(id) = self.pane_document(kind) {
                                self.switch_document(id);
                                self.pane_content(ui, kind);
                            } else {
                                let (icon, description) = match kind {
                                    Kind::Record => (Icon::Selection, "Select a row to inspect"),
                                    Kind::Structure => {
                                        (Icon::Structure, "Select a table to inspect its structure")
                                    }
                                    Kind::Map => (Icon::Map, "No relationships to display"),
                                    Kind::Sql => (Icon::Sql, "Empty SQL editor"),
                                    Kind::Data | Kind::Result => (Icon::Data, "No rows to display"),
                                };
                                icons::empty_state(ui, icon, description);
                            }
                        },
                    );
                },
            );
        }
        self.switch_document(self.database.workspace.focused);
    }
    fn workspace_dividers(&mut self, ui: &mut egui::Ui, panes: &[(Kind, egui::Rect)]) -> bool {
        let mut changed = false;
        for (divider, rect) in tiling::dividers(
            panes,
            &self.database.workspace.minimized,
            self.database.workspace.area,
        ) {
            let response = ui
                .interact(
                    rect,
                    ui.scope_id().with(("workspace-divider", divider)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(if divider.vertical() {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Splitter, ui.is_enabled(), divider.label())
            });
            if response.dragged() {
                let delta = if response.drag_started() {
                    response
                        .interact_pointer_pos()
                        .zip(ui.input(|i| i.pointer.press_origin()))
                        .map_or(egui::Vec2::ZERO, |(p, origin)| p - origin)
                } else {
                    response.drag_delta()
                };
                let movement = if divider.vertical() { delta.x } else { delta.y };
                if movement != 0.0 {
                    self.database.workspace.splits.resize(
                        divider,
                        delta,
                        panes,
                        &self.database.workspace.minimized,
                    );
                    changed = true;
                    ui.ctx().request_repaint();
                }
            }
            if response.hovered() || response.dragged() {
                let color = crate::appearance::Tokens::of(ui.ctx()).primary;
                let line = if divider.vertical() {
                    [rect.center_top(), rect.center_bottom()]
                } else {
                    [rect.left_center(), rect.right_center()]
                };
                ui.painter()
                    .line_segment(line, egui::Stroke::new(2.0, color));
            }
            if response.drag_stopped() {
                self.save_workspace();
            }
        }
        changed
    }
    fn pane_content(&mut self, ui: &mut egui::Ui, kind: Kind) {
        match kind {
            Kind::Map => self.diagram_ui(ui),
            Kind::Structure => {
                egui::ScrollArea::both()
                    .id_salt("structure-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.structure_inspector(ui));
            }
            Kind::Record => {
                if self.database.document.has_drafts()
                    || self.database.commit_owner == Some(self.database.document.id)
                {
                    egui::Panel::bottom("record-edits")
                        .resizable(false)
                        .show(ui, |ui| self.staged_edit_dock(ui));
                }
                let row = self
                    .database
                    .document
                    .inspect
                    .map(|(r, _)| r)
                    .or(self.database.document.tools.detail)
                    .or_else(|| (!self.database.document.result.rows.is_empty()).then_some(0));
                if let Some(row) = row
                    && self.related_visible()
                {
                    egui::Panel::bottom("record-related")
                        .resizable(false)
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 0)))
                        .show(ui, |ui| self.related_list(ui, row));
                }
                egui::ScrollArea::vertical()
                    .id_salt("record-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(row) = row {
                            self.record_inspector(ui, row);
                            if ui.available_width() > 600.0 {
                                self.wide_related(ui);
                            }
                        } else {
                            icons::empty_state(ui, Icon::Selection, "Select a row to inspect");
                        }
                    });
            }
            Kind::Sql => self.sql_pane(ui),
            Kind::Data | Kind::Result => {
                if self.database.document.has_drafts()
                    || self.database.commit_owner == Some(self.database.document.id)
                {
                    egui::Panel::bottom("staged-edit-dock")
                        .resizable(false)
                        .show(ui, |ui| self.staged_edit_dock(ui));
                }
                if self.database.document.ui.filter_open
                    || self.database.document.tools.filter_focus
                {
                    self.filter_controls(ui);
                }
                self.grid_contents(ui);
            }
        }
    }
    pub(super) fn sql_result_window(&self, editor: u64) -> Option<u64> {
        self.find_window(Kind::Result, None, Some(editor))
            .or_else(|| {
                self.database
                    .workspace
                    .visible
                    .get(&Kind::Result)
                    .copied()
                    .filter(|id| {
                        self.document_ref(*id)
                            .and_then(|d| d.source)
                            .and_then(|id| self.document_ref(id))
                            .is_some_and(|d| d.kind != Kind::Sql)
                    })
            })
    }
    fn sql_pane(&mut self, ui: &mut egui::Ui) {
        let editor = self.database.document.id;
        if let Some(result) = self.sql_result_window(editor) {
            let height = (ui.available_height() * 0.44).max(45.0);
            egui::Panel::top("sql-editor")
                .resizable(false)
                .exact_size(height)
                .show(ui, |ui| self.editor_ui(ui));
            self.switch_document(result);
            if ui.rect_contains_pointer(ui.available_rect_before_wrap())
                && ui.input(|i| i.pointer.any_pressed())
            {
                self.focus_window(result);
            }
            self.grid_contents(ui);
            self.switch_document(editor);
        } else {
            self.editor_ui(ui);
        }
    }
}

/// Persist pane visibility and reopenable identities only: never row buffers,
/// staged edits, SQL drafts or parameter values. Results/records remain session-only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct Snapshot {
    pub database: String,
    pub views: Vec<SavedView>,
    focused: u64,
    expanded: Option<u64>,
    #[serde(default)]
    minimized: BTreeSet<Kind>,
    #[serde(default)]
    active: Option<Kind>,
    #[serde(default)]
    splits: tiling::Splits,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct SavedView {
    id: u64,
    kind: Kind,
    pub object: Option<String>,
}
impl Snapshot {
    pub fn valid(&self) -> bool {
        if self.database.len() > 4096
            || !std::path::Path::new(&self.database).is_absolute()
            || self.views.is_empty()
            || self.views.len() > MAX_VIEWS
            || !self.splits.valid()
        {
            return false;
        }
        let ids: Vec<_> = self.views.iter().map(|v| v.id).collect();
        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        ids.len() == unique.len()
            && unique.contains(&self.focused)
            && self.expanded.is_none_or(|id| unique.contains(&id))
            && self.minimized.iter().all(|k| Kind::PANES.contains(k))
            && self.active.is_none_or(|k| Kind::PANES.contains(&k))
            && self.views.iter().filter(|v| v.kind == Kind::Map).count() <= 1
            && self.views.iter().all(|v| {
                v.id < u64::MAX - 1024
                    && match v.kind {
                        Kind::Map | Kind::Sql => v.object.is_none(),
                        Kind::Data | Kind::Structure => v
                            .object
                            .as_ref()
                            .is_some_and(|s| !s.is_empty() && s.len() <= 4096),
                        Kind::Result | Kind::Record => false,
                    }
            })
    }
}
impl App {
    pub(super) fn workspace_snapshot(&self) -> Option<Snapshot> {
        let views: Vec<_> = self
            .database
            .workspace
            .visible
            .values()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|id| {
                let d = self.document_ref(id)?;
                matches!(d.kind, Kind::Map | Kind::Sql | Kind::Data | Kind::Structure).then(|| {
                    SavedView {
                        id,
                        kind: d.kind,
                        object: d.object.clone(),
                    }
                })
            })
            .collect();
        let focused = if views
            .iter()
            .any(|v| v.id == self.database.workspace.focused)
        {
            self.database.workspace.focused
        } else {
            views.first()?.id
        };
        let saved = Snapshot {
            database: self.database.tools.database_scope.clone(),
            views,
            focused,
            expanded: self.database.workspace.expanded.map(|_| focused),
            active: Some(self.database.workspace.active),
            minimized: self.database.workspace.minimized.clone(),
            splits: self.database.workspace.splits,
        };
        saved.valid().then_some(saved)
    }
    pub(super) fn save_workspace(&mut self) {
        if !self.database.workspace.enabled {
            return;
        }
        self.preferences
            .workspace_layouts
            .retain(|s| s.database != self.database.tools.database_scope);
        if let Some(snapshot) = self.workspace_snapshot() {
            self.preferences.workspace_layouts.insert(0, snapshot);
        }
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }
    pub(super) fn restore_workspace(&mut self) {
        let Some(saved) = self
            .preferences
            .workspace_layouts
            .iter()
            .find(|s| s.database == self.database.tools.database_scope && s.valid())
            .cloned()
        else {
            return;
        };
        self.database.documents.clear();
        self.database.workspace.visible.clear();
        for view in saved.views {
            let mut document = Document {
                id: view.id,
                kind: view.kind,
                object: view.object,
                needs_load: matches!(view.kind, Kind::Data | Kind::Structure),
                ..Default::default()
            };
            document.ui.nav = match view.kind {
                Kind::Map => Nav::Overview,
                Kind::Sql => Nav::Result,
                _ => Nav::Table,
            };
            document.ui.table_view = if view.kind == Kind::Structure {
                TableView::Structure
            } else {
                TableView::Data
            };
            document.ui.sql_open = view.kind == Kind::Sql;
            if view.kind == Kind::Sql {
                document.sql.clear();
            }
            self.database.workspace.visible.insert(view.kind, view.id);
            self.database.documents.insert(view.id, document);
        }
        self.database.document = self.database.documents.remove(&saved.focused).unwrap();
        self.database
            .workspace
            .visible
            .insert(self.database.document.kind, saved.focused);
        self.database.workspace.focused = saved.focused;
        self.database.workspace.active = saved.active.unwrap_or(self.database.document.kind.pane());
        self.database.workspace.expanded = saved.expanded.map(|_| self.database.workspace.active);
        self.database.workspace.next_id = self.window_ids().into_iter().max().unwrap() + 1;
        let active = self.database.workspace.active;
        if !self.database.workspace.visible.contains_key(&Kind::Map) {
            self.create_window(Kind::Map, None, None, false);
        }
        if !self.database.workspace.visible.contains_key(&Kind::Sql) {
            self.open_sql_window(false);
        }
        self.database.workspace.active = active;
        self.database.workspace.expanded = saved.expanded.map(|_| active);
        self.database.workspace.minimized = saved.minimized;
        self.database.workspace.splits = saved.splits;
        if let Some(id) = self.pane_document(active) {
            self.switch_document(id);
            self.database.workspace.focused = id;
        }
        self.database.status =
            "Workspace layout restored; SQL recovery is controlled in Settings. Results and records are session-only.".into();
    }
    pub(super) fn load_restored_view(&mut self, ctx: &egui::Context) {
        if !self.requests_enabled()
            || self.executing()
            || self.database.catalog_ticket.is_some()
            || self.database.ui.diagram.ticket.is_some()
            || self.database.document.tools.inspection_ticket.is_some()
            || self
                .database
                .documents
                .values()
                .any(|d| d.tools.inspection_ticket.is_some())
        {
            return;
        }
        let Some(id) = self
            .window_ids()
            .into_iter()
            .find(|id| self.document_ref(*id).is_some_and(|d| d.needs_load))
        else {
            return;
        };
        let focused = self.database.document.id;
        self.switch_document(id);
        let name = self.database.document.object.clone().unwrap();
        // Metadata has arrived before restoring table views. Missing objects stay visible with a clear explanation.
        if let Some(object) = self.database.schema.iter().find(|o| o.name == name) {
            if self.database.document.kind == Kind::Data
                && matches!(object.kind.as_str(), "table" | "view")
            {
                let browse = tursobench_core::Browse {
                    options: Default::default(),
                    table: name,
                    offset: 0,
                    sort: None,
                    descending: false,
                    filters: vec![],
                };
                self.database.document.browse = Some(browse.clone());
                self.database.document.view = View::Browse;
                self.database.document.needs_load = self
                    .submit(tursobench_db::Operation::Browse(browse), View::Browse)
                    .is_none();
            } else if matches!(object.kind.as_str(), "index" | "trigger") {
                self.database.document.tools.ddl = Some(object.clone());
                self.database.document.needs_load = false;
            } else {
                self.request_inspection(name);
                self.database.document.needs_load =
                    self.database.document.tools.inspection_ticket.is_none();
            }
        } else {
            self.database.document.needs_load = false;
            self.database.document.tools.inspection_error =
                format!("{name} is no longer in the database catalog");
        }
        self.switch_document(focused);
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}
