//! Fixed visible panes over bounded, independently owned request/result buffers.
use super::{
    App,
    document::Document,
    icons::{self, Icon},
    presentation,
    shell::{DetailMode, Nav, TableView},
    tiling::{self, Kind},
};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
pub(super) const MAX_VIEWS: usize = 16;
/// Width of the strip at the left of a tile's header that drags the tile.
const GRIP: f32 = 12.0;
pub(super) fn tile_icon(kind: Kind) -> Icon {
    match kind {
        Kind::Map => Icon::Map,
        Kind::Data | Kind::Result => Icon::Data,
        Kind::Record => Icon::Selection,
        Kind::Structure => Icon::Structure,
        Kind::Sql => Icon::Sql,
        Kind::Chat => Icon::Assistant,
    }
}
pub(super) struct Workspace {
    pub enabled: bool,
    pub focused: u64,
    pub active: Kind,
    pub expanded: Option<Kind>,
    pub layout: tiling::Layout,
    pub minimized: BTreeSet<Kind>,
    /// The tile being dragged to another place by its grip.
    pub drag: Option<TileDrag>,
    pub visible: BTreeMap<Kind, u64>,
    pub next_id: u64,
    pub area: egui::Rect,
    #[cfg(test)]
    pub tiles: Vec<(u64, egui::Rect)>,
    #[cfg(test)]
    pub pane_rects: Vec<(Kind, egui::Rect)>,
    #[cfg(test)]
    pub controls: Vec<(Kind, egui::Rect, egui::Rect)>,
    #[cfg(test)]
    pub grips: Vec<(Kind, egui::Rect)>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TileDrag {
    pub kind: Kind,
    /// Where it would land if released now.
    pub target: Option<tiling::Target>,
    /// Escape was pressed; the rest of the drag does nothing.
    pub cancelled: bool,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            enabled: false,
            focused: 0,
            active: Kind::Map,
            expanded: None,
            layout: Default::default(),
            minimized: Default::default(),
            drag: None,
            visible: Default::default(),
            next_id: 1,
            area: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0)),
            #[cfg(test)]
            tiles: vec![],
            #[cfg(test)]
            pane_rects: vec![],
            #[cfg(test)]
            controls: vec![],
            #[cfg(test)]
            grips: vec![],
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
        self.activate_pane(kind);
        if let Some(id) = self.pane_document(kind) {
            self.switch_document(id);
            self.database.workspace.focused = id;
        }
    }
    /// Make `kind`'s pane active and restored. Another tile asking for attention
    /// makes an expanded chat give the workspace back to the tiles.
    fn activate_pane(&mut self, kind: Kind) {
        let workspace = &mut self.database.workspace;
        if kind.pane() != Kind::Chat && workspace.expanded == Some(Kind::Chat) {
            workspace.expanded = None;
        }
        workspace.active = kind.pane();
        workspace.minimized.remove(&kind.pane());
        if workspace.expanded.is_some() {
            workspace.expanded = Some(kind.pane());
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
        self.activate_pane(kind);
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
            next.set_sql(String::new());
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
        // The assistant starts as it was last left, in any database.
        if !self.preferences.panels.assistant {
            self.database.workspace.minimized.insert(Kind::Chat);
        }
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
        self.select_cell(row, column, false);
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
    /// Fold a column to a line, or open the tiles that were open before it
    /// folded (all of them if none were).
    pub(super) fn toggle_column(&mut self, column: usize) {
        let workspace = &mut self.database.workspace;
        if column >= workspace.layout.columns.len() {
            return;
        }
        if workspace.layout.all_minimized(column, &workspace.minimized) {
            workspace.layout.unfold(column, &mut workspace.minimized);
        } else {
            workspace.layout.fold(column, &mut workspace.minimized);
            if workspace.expanded.is_some_and(|kind| {
                workspace.layout.columns[column]
                    .tiles
                    .contains(&kind.pane())
            }) {
                workspace.expanded = None;
            }
        }
        self.save_workspace();
    }
    /// Fold or reopen the column that holds the focused tile.
    pub(super) fn toggle_tiles_column(&mut self) {
        let workspace = &self.database.workspace;
        if let Some(column) = workspace.layout.column_of(workspace.active) {
            self.toggle_column(column);
        }
    }
    /// Restore every minimized tile. The assistant stays as it is: it is shown
    /// and hidden on its own.
    pub(super) fn retile(&mut self) {
        self.database
            .workspace
            .minimized
            .retain(|kind| *kind == Kind::Chat);
        self.database.workspace.expanded = None;
    }
    /// Whether "Restore all tiles" has anything to restore.
    pub(super) fn tiles_restorable(&self) -> bool {
        let workspace = &self.database.workspace;
        workspace.expanded.is_some() || workspace.minimized.iter().any(|kind| *kind != Kind::Chat)
    }
    /// Return this database to the default layout. Open tables stay open.
    pub(super) fn reset_layout(&mut self) {
        let workspace = &mut self.database.workspace;
        workspace.layout = Default::default();
        workspace.drag = None;
        self.retile();
        self.save_workspace();
        self.database.status = "Layout reset to the default for this database".into();
    }
    /// Move the focused tile one step: into the neighbouring column, up or
    /// down its own, or right from the last column into a new one.
    pub(super) fn move_focused_tile(&mut self, direction: tiling::Direction) {
        let workspace = &self.database.workspace;
        if workspace.expanded.is_some() {
            return;
        }
        let kind = workspace.active;
        if let Some(target) = workspace.layout.step(
            kind,
            direction,
            workspace.area.width(),
            &workspace.minimized,
        ) {
            self.move_tile(kind, target);
        }
    }
    pub(super) fn move_tile(&mut self, kind: Kind, target: tiling::Target) {
        if self.database.workspace.layout.move_tile(kind, target) {
            // A moved line stays folded; focusing its tile would open it.
            if !matches!(target, tiling::Target::Column(_)) {
                self.focus_pane(kind);
            }
            self.save_workspace();
        }
    }
    pub(super) fn workspace_frame(&self) -> tiling::Frame {
        let workspace = &self.database.workspace;
        tiling::layout(
            workspace.area,
            workspace.expanded,
            &workspace.minimized,
            &workspace.layout,
        )
    }
    pub(super) fn tiled_workspace(&mut self, ui: &mut egui::Ui) {
        self.database.workspace.area = ui.available_rect_before_wrap();
        #[cfg(test)]
        {
            self.database.workspace.tiles.clear();
            self.database.workspace.pane_rects.clear();
            self.database.workspace.controls.clear();
            self.database.workspace.grips.clear();
        }
        let mut frame = self.workspace_frame();
        if self.workspace_dividers(ui, &frame) {
            frame = self.workspace_frame();
        }
        let pointer_on_workspace = ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| ui.ctx().layer_id_at(p) == Some(ui.layer_id()));
        let hidden = self.fold_lines(ui, &frame);
        for (kind, rect) in frame.tiles.iter().copied() {
            if hidden.contains(&kind) {
                continue;
            }
            // The assistant has no document of its own; it sees the focused one.
            if kind == Kind::Chat {
                self.switch_document(self.database.workspace.focused);
            }
            let owner = self.pane_document(kind);
            if let Some(id) = owner {
                self.switch_document(id);
            }
            // A click in a foreground dialog must keep its document owner;
            // it must not focus whichever tile happens to sit underneath.
            if pointer_on_workspace
                && !self.database.workspace.minimized.contains(&kind)
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
                if kind == Kind::Chat {
                    self.database.ui.assistant_rect = rect;
                }
                if let Some(id) = owner {
                    self.database.workspace.tiles.push((id, rect));
                }
            }
            tile_chrome(ui, rect, self.database.workspace.active == kind);
            let content = rect.shrink(4.0);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(("pane", kind))
                    .max_rect(content),
                |ui| {
                    ui.set_clip_rect(content.intersect(ui.clip_rect()));
                    ui.set_min_size(content.size());
                    ui.set_max_size(content.size());
                    self.tile_contents(ui, kind, owner.is_some(), content);
                },
            );
            self.tile_grip(ui, kind, rect);
        }
        self.tile_drag(ui, &frame);
        self.switch_document(self.database.workspace.focused);
    }
    /// Draw each column whose tiles are all minimized as one line instead, and
    /// return the tiles those lines hide.
    fn fold_lines(&mut self, ui: &mut egui::Ui, frame: &tiling::Frame) -> BTreeSet<Kind> {
        let mut hidden = BTreeSet::new();
        for (column, lane) in frame.columns.iter().enumerate() {
            if !lane.folded {
                continue;
            }
            let workspace = &mut self.database.workspace;
            let tiles = &workspace.layout.columns[column].tiles;
            hidden.extend(tiles.iter().copied());
            let name = format!("Show {}", workspace.layout.names(column));
            let sense = egui::Sense::click_and_drag();
            let line = presentation::fold_line(ui, lane.rect, ("column", column), &name, sense);
            // Dragging a line moves its column; any of its tiles stands for it.
            if line.drag_started() {
                workspace.drag = Some(TileDrag {
                    kind: tiles[0],
                    target: None,
                    cancelled: false,
                });
            }
            if line.clicked() {
                self.toggle_column(column);
            }
        }
        hidden
    }
    /// A minimized tile is only its title bar; an open one has its ribbon and body.
    fn tile_contents(&mut self, ui: &mut egui::Ui, kind: Kind, owned: bool, content: egui::Rect) {
        if self.database.workspace.minimized.contains(&kind) {
            ui.horizontal(|ui| {
                self.minimized_pane_title(ui, kind);
                if icons::ribbon_button(ui, Icon::Fit, "Expand to workspace", "", None).clicked() {
                    self.expand_pane(kind);
                }
            });
            return;
        }
        self.pane_ribbon(ui, kind, owned);
        // The ribbon can minimize the tile.
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
                self.tile_body(ui, kind);
            },
        );
    }
    fn tile_body(&mut self, ui: &mut egui::Ui, kind: Kind) {
        if kind == Kind::Chat {
            self.assistant_body(ui);
        } else if let Some(id) = self.pane_document(kind) {
            self.switch_document(id);
            self.pane_content(ui, kind);
        } else {
            let description = match kind {
                Kind::Record => "Select a row to inspect",
                Kind::Structure => "Select a table to inspect its structure",
                Kind::Map => "No relationships to display",
                Kind::Sql => "Empty SQL editor",
                Kind::Data | Kind::Result | Kind::Chat => "No rows to display",
            };
            icons::empty_state(ui, tile_icon(kind), description);
        }
    }
    /// The grip at the left of a tile's header. Dragging it moves the tile.
    fn tile_grip(&mut self, ui: &mut egui::Ui, kind: Kind, rect: egui::Rect) {
        if self.database.workspace.expanded.is_some() {
            return;
        }
        let grip = egui::Rect::from_min_max(
            rect.min + egui::vec2(1.0, 1.0),
            egui::pos2(
                rect.left() + GRIP,
                (rect.top() + tiling::BAR - 4.0).min(rect.bottom()),
            ),
        );
        #[cfg(test)]
        self.database.workspace.grips.push((kind, grip));
        let name = format!("Move {}", kind.label());
        let response = ui
            .interact(
                grip,
                ui.scope_id().with(("tile-grip", kind)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(format!(
                "{name}\nDrag onto a column, or past the last one to start a column\nAlt Shift arrows move the focused tile"
            ));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        let held = self
            .database
            .workspace
            .drag
            .is_some_and(|drag| drag.kind == kind && !drag.cancelled);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let color = if response.hovered() || held {
            tokens.primary
        } else {
            tokens.muted
        };
        for dy in [-6.0, 0.0, 6.0] {
            ui.painter()
                .circle_filled(grip.center() + egui::vec2(0.0, dy), 1.5, color);
        }
        if response.drag_started() {
            self.database.workspace.drag = Some(TileDrag {
                kind,
                target: None,
                cancelled: false,
            });
        }
    }
    /// Follow a tile dragged by its grip: mark where it would land and move it
    /// there on release. Escape leaves it where it was.
    fn tile_drag(&mut self, ui: &mut egui::Ui, frame: &tiling::Frame) {
        let Some(mut drag) = self.database.workspace.drag else {
            return;
        };
        let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        let workspace = &mut self.database.workspace;
        drag.target = pointer.filter(|_| !drag.cancelled).and_then(|pointer| {
            tiling::drop_target(
                pointer,
                drag.kind,
                frame,
                &workspace.layout,
                &workspace.minimized,
                workspace.area,
            )
        });
        if !down {
            workspace.drag = None;
            if let Some(target) = drag.target {
                self.move_tile(drag.kind, target);
            }
            return;
        }
        workspace.drag = Some(drag);
        let workspace = &self.database.workspace;
        ui.ctx().request_repaint();
        if drag.cancelled {
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("tile-drag"),
        ));
        let column = workspace.layout.column_of(drag.kind);
        let line = column
            .and_then(|c| frame.columns.get(c))
            .filter(|lane| lane.folded);
        if let Some(rect) = line.map(|lane| lane.rect).or_else(|| frame.rect(drag.kind)) {
            painter.rect_filled(rect, 10.0, ui.visuals().panel_fill.gamma_multiply(0.6));
        }
        if let Some(target) = drag.target {
            let marker = tiling::marker(target, frame, &workspace.layout, workspace.area);
            painter.rect_filled(marker.expand(3.0), 5.0, tokens.primary.gamma_multiply(0.25));
            painter.rect_filled(marker, 2.0, tokens.primary);
        }
        if let Some(pointer) = pointer {
            // A small copy of the tile's header follows the pointer.
            let label = painter.layout_no_wrap(
                match (line, column) {
                    (Some(_), Some(column)) => workspace.layout.names(column),
                    _ => drag.kind.label().into(),
                },
                crate::appearance::proportional(ui.ctx(), icons::RIBBON_TEXT_SIZE),
                ui.visuals().text_color(),
            );
            let ghost = egui::Rect::from_min_size(
                pointer + egui::vec2(12.0, 10.0),
                egui::vec2(label.size().x + 52.0, 30.0),
            );
            painter.rect(
                ghost,
                8.0,
                tokens.raised,
                egui::Stroke::new(1.0, tokens.primary.gamma_multiply(0.5)),
                egui::StrokeKind::Inside,
            );
            for dy in [-6.0, 0.0, 6.0] {
                painter.circle_filled(
                    ghost.left_center() + egui::vec2(8.0, dy),
                    1.5,
                    tokens.primary,
                );
            }
            presentation::icon_at(
                &painter,
                ghost.left_center() + egui::vec2(26.0, 0.0),
                16.0,
                tile_icon(drag.kind),
                ui.visuals().text_color(),
            );
            painter.galley(
                egui::pos2(ghost.left() + 40.0, ghost.center().y - label.size().y / 2.0),
                label,
                ui.visuals().text_color(),
            );
        }
    }
    fn minimized_pane_title(&mut self, ui: &mut egui::Ui, kind: Kind) {
        let document = &self.database.document;
        let name = document.object.as_deref().unwrap_or("");
        let title = match kind {
            Kind::Record => format!(
                "Record · {name} · {} / {}",
                document.inspect.map_or(0, |(r, _)| r + 1),
                document.result.rows.len()
            ),
            Kind::Structure => format!("Structure · {name}"),
            _ => kind.label().into(),
        };
        let width =
            (ui.available_width() - icons::RIBBON_BUTTON_SIZE - ui.spacing().item_spacing.x)
                .max(0.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(width, icons::RIBBON_BUTTON_SIZE),
            egui::Sense::click(),
        );
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
        }
        let mut right = rect.right() - 4.0;
        let mut description = title.clone();
        if kind == Kind::Structure
            && let Some(info) = self.loaded_inspection(name)
        {
            let counts = [
                (Icon::Columns, info.table.columns.len(), "columns"),
                (Icon::Complete, info.indexes.len(), "indexes"),
                (
                    Icon::Link,
                    super::diagram::ForeignMap {
                        info,
                        graph: self.database.ui.diagram.data.as_ref(),
                        incomplete: self.schema_incomplete(),
                    }
                    .count(),
                    "foreign keys",
                ),
            ];
            for (icon, count, label) in counts.into_iter().rev() {
                description.push_str(&format!(" · {count} {label}"));
                let text = count.to_string();
                let group_width = 22.0 + presentation::text_width(ui, text.as_str(), 11.0);
                let area = presentation::strip_before(rect, right, group_width);
                presentation::icon_at(
                    ui.painter(),
                    area.left_center() + egui::vec2(7.0, 0.0),
                    13.0,
                    icon,
                    ui.visuals().weak_text_color(),
                );
                super::presentation::text_at(
                    ui,
                    egui::Rect::from_min_max(area.min + egui::vec2(18.0, 0.0), area.max),
                    &text,
                    ui.visuals().weak_text_color(),
                    11.0,
                );
                right -= group_width + 4.0;
            }
        }
        super::presentation::text_at(
            ui,
            egui::Rect::from_min_max(
                rect.min + egui::vec2(10.0, 0.0),
                egui::pos2(right.max(rect.left() + 11.0), rect.bottom()),
            ),
            &title,
            ui.visuals().text_color(),
            icons::RIBBON_TEXT_SIZE,
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                ui.is_enabled(),
                if matches!(kind, Kind::Record | Kind::Structure) {
                    format!("Restore {description}")
                } else {
                    kind.label().into()
                },
            )
        });
        if response
            .on_hover_text(format!("{description}\nClick to restore this tile"))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            self.focus_pane(kind);
        }
    }
    fn workspace_dividers(&mut self, ui: &mut egui::Ui, frame: &tiling::Frame) -> bool {
        let mut changed = false;
        let workspace = &self.database.workspace;
        for (divider, rect) in tiling::dividers(
            frame,
            &workspace.layout,
            &workspace.minimized,
            workspace.area,
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
            if let Some(delta) = divider_drag(ui, &response)
                && (if divider.vertical() { delta.x } else { delta.y }) != 0.0
            {
                let workspace = &mut self.database.workspace;
                workspace
                    .layout
                    .resize(divider, delta, frame, &workspace.minimized);
                changed = true;
                ui.ctx().request_repaint();
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
                changed |= self.fold_dragged_column(ui, frame, divider);
                self.save_workspace();
            }
        }
        changed
    }
    /// Dragging a column divider far into a column folds that column to a line.
    /// Returns whether the divider ended in such a column.
    fn fold_dragged_column(
        &mut self,
        ui: &egui::Ui,
        frame: &tiling::Frame,
        divider: tiling::Divider,
    ) -> bool {
        let edge = tiling::MIN_WIDTH / 2.0;
        let tiling::Divider::Columns(left) = divider else {
            return false;
        };
        let (Some(x), Some(a), Some(b)) = (
            ui.input(|i| i.pointer.latest_pos()).map(|p| p.x),
            frame.columns.get(left),
            frame.columns.get(left + 1),
        ) else {
            return false;
        };
        let column = if x > b.rect.right() - edge {
            left + 1
        } else if x < a.rect.left() + edge {
            left
        } else {
            return false;
        };
        let workspace = &self.database.workspace;
        if !workspace.layout.all_minimized(column, &workspace.minimized) {
            self.toggle_column(column);
        }
        true
    }
    fn pane_content(&mut self, ui: &mut egui::Ui, kind: Kind) {
        match kind {
            Kind::Map => self.diagram_ui(ui),
            Kind::Structure => self.structure_inspector(ui),
            Kind::Record => {
                if self.database.document.has_drafts()
                    || self.database.commit_owner == Some(self.database.document.id)
                {
                    egui::Panel::bottom("record-edits")
                        .resizable(false)
                        .show(ui, |ui| self.staged_edit_dock(ui));
                }
                // The tile's second level: one value filling the body.
                if self.value_open() {
                    self.value_view_ui(ui);
                    return;
                }
                let row = self
                    .database
                    .document
                    .inspect
                    .map(|(r, _)| r)
                    .or(self.database.document.tools.detail)
                    .or_else(|| (!self.database.document.result.rows.is_empty()).then_some(0));
                if let Some(row) = row
                    && (self.related_visible()
                        || self
                            .database
                            .document
                            .browse_inspection()
                            .is_some_and(|info| !info.foreign_keys.is_empty()))
                {
                    let relationship_height = (ui.available_height() * 0.35 - 24.0).max(0.0);
                    egui::Panel::bottom("record-relationships")
                        .resizable(false)
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 0)))
                        .show(ui, |ui| {
                            self.relationships_line(ui, row, relationship_height)
                        });
                }
                egui::ScrollArea::vertical()
                    .id_salt("record-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(row) = row {
                            self.record_inspector(ui, row);
                        } else {
                            icons::empty_state(ui, Icon::Selection, "Select a row to inspect");
                        }
                    });
            }
            Kind::Sql => self.sql_pane(ui),
            Kind::Chat => self.assistant_body(ui),
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
            // The editor keeps a saved share of the tile; the line between the
            // editor and its results drags to change it.
            let total = ui.available_height();
            let range = 45.0..=(total - 80.0).max(45.0);
            let height = (total * self.database.workspace.layout.sql_editor)
                .clamp(*range.start(), *range.end());
            let panel = egui::Panel::top("sql-editor")
                .resizable(false)
                .exact_size(height)
                .show(ui, |ui| self.editor_ui(ui));
            let edge = panel.response.rect;
            let handle = egui::Rect::from_x_y_ranges(
                edge.x_range(),
                (edge.bottom() - 4.0)..=(edge.bottom() + 4.0),
            );
            let response = ui
                .interact(handle, ui.scope_id().with("sql-split"), egui::Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeVertical);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Splitter,
                    ui.is_enabled(),
                    "Resize SQL and results",
                )
            });
            if response.dragged() && total > 0.0 {
                let height = (height + response.drag_delta().y).clamp(*range.start(), *range.end());
                self.database.workspace.layout.sql_editor = (height / total).clamp(0.01, 0.99);
                ui.ctx().request_repaint();
            }
            if response.hovered() || response.dragged() {
                ui.painter().line_segment(
                    [handle.left_center(), handle.right_center()],
                    egui::Stroke::new(2.0, crate::appearance::Tokens::of(ui.ctx()).primary),
                );
            }
            if response.drag_stopped() {
                self.save_workspace();
            }
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
    /// Where every tile sits. Absent in layouts saved before tiles could move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout: Option<tiling::Layout>,
    /// The two-column splits those older layouts saved instead.
    #[serde(default, skip_serializing)]
    splits: Option<tiling::Splits>,
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
            || self.layout.as_ref().is_some_and(|layout| !layout.valid())
            || self.splits.is_some_and(|splits| !splits.valid())
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
                        Kind::Result | Kind::Record | Kind::Chat => false,
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
            layout: Some(self.database.workspace.layout.clone()),
            splits: None,
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
        // A database opened for the first time starts with the assistant as it was last left.
        self.preferences.panels.assistant =
            !self.database.workspace.minimized.contains(&Kind::Chat);
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
                document.set_sql(String::new());
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
        // A layout saved before the assistant was a tile says nothing about it.
        if saved.layout.is_none() && !self.preferences.panels.assistant {
            self.database.workspace.minimized.insert(Kind::Chat);
        }
        self.database.workspace.layout = saved
            .layout
            .or(saved.splits.map(Into::into))
            .unwrap_or_default();
        self.database.workspace.drag = None;
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
                self.database.document.needs_load = !self.browse_first_page(name);
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
/// A tile's card: its background, header band and outline, accented when focused.
fn tile_chrome(ui: &egui::Ui, rect: egui::Rect, focused: bool) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);
    let header = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (rect.top() + tiling::BAR).min(rect.bottom())),
    );
    let top = egui::CornerRadius {
        nw: 10,
        ne: 10,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(header, top, tokens.raised);
    let outline = if focused {
        tokens.primary.gamma_multiply(0.5)
    } else {
        tokens.line
    };
    painter.rect_stroke(
        rect,
        10.0,
        egui::Stroke::new(1.0, outline),
        egui::StrokeKind::Inside,
    );
    if focused {
        painter.hline(
            rect.x_range().shrink(8.0),
            rect.top() + 1.0,
            egui::Stroke::new(2.0, tokens.primary),
        );
    }
}
/// How far a divider moved this frame. The first frame of a drag counts from
/// where the press began, so the movement before egui recognised the drag is kept.
fn divider_drag(ui: &egui::Ui, response: &egui::Response) -> Option<egui::Vec2> {
    if !response.dragged() {
        return None;
    }
    Some(if response.drag_started() {
        response
            .interact_pointer_pos()
            .zip(ui.input(|i| i.pointer.press_origin()))
            .map_or(egui::Vec2::ZERO, |(p, origin)| p - origin)
    } else {
        response.drag_delta()
    })
}
