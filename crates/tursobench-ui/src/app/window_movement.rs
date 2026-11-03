//! Header dragging previews a tree edit. Commit only on release in a valid target.
use super::{
    App,
    tiling::{Placement, Tile},
};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DropTarget {
    target: Option<u64>,
    placement: Placement,
}

fn edge_at(rect: egui::Rect, pointer: egui::Pos2) -> Placement {
    let offset = (pointer - rect.center()) / rect.size();
    if offset.x.abs() > offset.y.abs() {
        if offset.x < 0.0 {
            Placement::Left
        } else {
            Placement::Right
        }
    } else if offset.y < 0.0 {
        Placement::Above
    } else {
        Placement::Below
    }
}
fn drop_target(
    area: egui::Rect,
    tiles: &[Tile],
    source: u64,
    pointer: egui::Pos2,
    group: bool,
) -> Option<DropTarget> {
    if !area.contains(pointer) {
        return None;
    }
    // The outer band moves a view across the entire workspace, even across nested splits.
    if !area.shrink(24.0).contains(pointer) {
        return Some(DropTarget {
            target: None,
            placement: edge_at(area, pointer),
        });
    }
    let tile = tiles.iter().find(|tile| tile.rect.contains(pointer))?;
    let target = if tile.selected == source {
        tile.ids.iter().copied().find(|id| *id != source)?
    } else {
        tile.selected
    };
    let offset = (pointer - tile.rect.center()) / tile.rect.size();
    let placement = if offset.x.abs() <= 0.22 && offset.y.abs() <= 0.22 {
        if group {
            Placement::Tab
        } else {
            Placement::Swap
        }
    } else {
        edge_at(tile.rect, pointer)
    };
    Some(DropTarget {
        target: Some(target),
        placement,
    })
}
impl App {
    fn cancel_window_drag(&mut self) {
        self.workspace.dragging = None;
        self.workspace.expanded = self
            .workspace
            .drag_expanded
            .take()
            .filter(|id| self.document_ref(*id).is_some());
    }
    pub(super) fn move_window(&mut self, source: u64, target: Option<u64>, placement: Placement) {
        let Some(tree) = self
            .workspace
            .tree
            .as_ref()
            .and_then(|tree| tree.relocated(source, target, placement))
        else {
            return;
        };
        self.workspace.tree = Some(tree);
        self.workspace.expanded = None;
        self.workspace.dragging = None;
        self.workspace.drag_expanded = None;
        self.focus_window(source);
        self.status = format!("Window moved · {}", placement.label());
    }
    pub(super) fn movement_shortcuts(&mut self, ctx: &egui::Context) {
        if self.workspace.dragging.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.cancel_window_drag();
        }
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::ALT | egui::Modifiers::SHIFT;
        for (key, placement) in [
            (egui::Key::ArrowLeft, Placement::Left),
            (egui::Key::ArrowRight, Placement::Right),
            (egui::Key::ArrowUp, Placement::Above),
            (egui::Key::ArrowDown, Placement::Below),
        ] {
            if ctx.input_mut(|i| i.consume_key(modifiers, key)) {
                self.move_window(self.workspace.focused, None, placement);
            }
        }
    }
    pub(super) fn window_move_menu(&mut self, ui: &mut egui::Ui, source: u64) {
        ui.menu_button("Move to workspace edge", |ui| {
            ui.small("Ctrl Alt Shift + arrow");
            for placement in Placement::EDGES {
                if ui.button(placement.label()).clicked() {
                    self.move_window(source, None, placement);
                    ui.close();
                }
            }
        });
        ui.menu_button("Move relative to window", |ui| {
            for target in self.window_ids().into_iter().filter(|id| *id != source) {
                let Some(document) = self.document_ref(target) else {
                    continue;
                };
                ui.menu_button(document.title(), |ui| {
                    for placement in Placement::EDGES
                        .into_iter()
                        .chain([Placement::Tab, Placement::Swap])
                    {
                        if ui.button(placement.label()).clicked() {
                            self.move_window(source, Some(target), placement);
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    pub(super) fn window_handle(&mut self, id: u64, response: &egui::Response) {
        response.clone().on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text("Drag to move · edges split · center swaps · Shift+center groups tabs · Esc cancels. Right-click for movement options.");
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.workspace.dragging = Some(id);
            self.workspace.drag_expanded = self.workspace.expanded.take();
        }
        if response.double_clicked() {
            self.expand_window(id);
        }
        response.context_menu(|ui| self.window_move_menu(ui, id));
        #[cfg(test)]
        self.workspace.headers.push((id, response.rect));
    }
    pub(super) fn window_drop_preview(&mut self, ui: &egui::Ui, tiles: &[Tile]) {
        let Some(source) = self.workspace.dragging else {
            return;
        };
        if self.document_ref(source).is_none() {
            self.cancel_window_drag();
            return;
        }
        let (pointer, released, down, group) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.primary_down(),
                i.modifiers.shift,
            )
        });
        let target =
            pointer.and_then(|p| drop_target(self.workspace.area, tiles, source, p, group));
        let candidate = target.and_then(|target| {
            self.workspace
                .tree
                .as_ref()?
                .relocated(source, target.target, target.placement)
                .map(|tree| (target, tree))
        });
        if let Some((target, tree)) = &candidate {
            let profiles = self.profiles();
            let preview = tree
                .layout(self.workspace.area, source, &|id| profiles[&id].0)
                .0;
            if let Some(tile) = preview.iter().find(|t| t.ids.contains(&source)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("window-drop-preview"),
                ));
                let color = ui.visuals().selection.stroke.color;
                painter.rect_filled(tile.rect, 3.0, color.gamma_multiply(0.2));
                painter.rect_stroke(
                    tile.rect.shrink(1.0),
                    3.0,
                    egui::Stroke::new(2.0, color),
                    egui::StrokeKind::Inside,
                );
                let label = format!(
                    "{}{}{}",
                    target.placement.label(),
                    if target.target.is_none() {
                        " · entire workspace"
                    } else {
                        ""
                    },
                    if tile.ids.len() > 1 {
                        " · tab group at this size"
                    } else {
                        ""
                    }
                );
                let font = egui::FontId::proportional(14.0);
                let galley = painter.layout_no_wrap(label, font, ui.visuals().text_color());
                let rect = egui::Rect::from_center_size(
                    tile.rect.center(),
                    galley.size() + egui::vec2(20.0, 12.0),
                );
                painter.rect_filled(rect, 4.0, ui.visuals().window_fill);
                painter.galley(
                    rect.min + egui::vec2(10.0, 6.0),
                    galley,
                    ui.visuals().text_color(),
                );
            }
        }
        if released {
            if let Some((target, _)) = candidate {
                self.move_window(source, target.target, target.placement);
            } else {
                self.cancel_window_drag();
            }
        } else if !down {
            self.cancel_window_drag();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_zones_distinguish_workspace_edges_tile_edges_and_centers() {
        let area = egui::Rect::from_min_size(egui::pos2(300.0, 80.0), egui::vec2(1200.0, 800.0));
        let tile = Tile {
            ids: vec![2],
            selected: 2,
            rect: area,
        };
        assert_eq!(
            drop_target(area, std::slice::from_ref(&tile), 1, area.center(), false)
                .unwrap()
                .placement,
            Placement::Swap
        );
        assert_eq!(
            drop_target(area, std::slice::from_ref(&tile), 1, area.center(), true)
                .unwrap()
                .placement,
            Placement::Tab
        );
        let bottom = drop_target(
            area,
            std::slice::from_ref(&tile),
            1,
            area.center_bottom() - egui::vec2(0.0, 2.0),
            false,
        )
        .unwrap();
        assert_eq!(
            bottom,
            DropTarget {
                target: None,
                placement: Placement::Below
            }
        );
        let above = drop_target(
            area,
            std::slice::from_ref(&tile),
            1,
            area.center_top() + egui::vec2(0.0, 50.0),
            false,
        )
        .unwrap();
        assert_eq!(
            above,
            DropTarget {
                target: Some(2),
                placement: Placement::Above
            }
        );
        assert!(drop_target(area, &[tile], 1, egui::pos2(100.0, 300.0), false).is_none());
    }
}
