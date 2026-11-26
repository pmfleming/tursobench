//! The paged hex lens: offsets, bytes and characters drawn one galley a line,
//! with the first line kept as a `u64` so any size of value scrolls exactly.
use super::{preview, view::View};
use crate::app::presentation;
use eframe::egui;

/// The height of one line of hex.
pub(in crate::app) const LINE: f32 = 16.0;

/// Where each line, byte and character of the hex lens is.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct Grid {
    pub rect: egui::Rect,
    pub glyph: f32,
    pub per: usize,
    /// Hexadecimal digits in each offset.
    pub digits: usize,
}
impl Grid {
    /// The grid for a value `len` bytes long in `rect`, with as many bytes a
    /// line as fit: 8, 16 or 32.
    pub fn new(ui: &egui::Ui, rect: egui::Rect, len: u64) -> Self {
        let digits = if len <= 0x1_0000 {
            4
        } else if len <= 0x100_0000 {
            6
        } else {
            8
        };
        let glyph = ui
            .painter()
            .layout_no_wrap("0".into(), preview::font(ui), egui::Color32::WHITE)
            .size()
            .x;
        let room = rect.width() - (digits - 4) as f32 * glyph;
        Self {
            rect,
            glyph,
            per: preview::bytes_per_line(ui, room),
            digits,
        }
    }
    pub fn lines(&self) -> usize {
        ((self.rect.height() / LINE).floor() as usize).max(1)
    }
    fn line_top(&self, line: usize) -> f32 {
        self.rect.top() + line as f32 * LINE
    }
    /// The character column where byte `i` of a line starts in the hex column.
    fn hex_column(&self, i: usize) -> usize {
        self.digits + 2 + i * 3 + i / 8
    }
    /// The character column where the characters column starts.
    fn text_column(&self) -> usize {
        self.hex_column(self.per - 1) + 2 + 2
    }
    /// The rectangle of byte `i` of line `line`, in either column.
    pub fn cell(&self, line: usize, i: usize, characters: bool) -> egui::Rect {
        let (column, width) = if characters {
            (self.text_column() + i, 1.0)
        } else {
            (self.hex_column(i), 2.0)
        };
        egui::Rect::from_min_size(
            egui::pos2(
                self.rect.left() + column as f32 * self.glyph,
                self.line_top(line),
            ),
            egui::vec2(width * self.glyph, LINE),
        )
    }
    /// The byte under `pos` when the first line shown is `top`, and whether
    /// it is in the characters column.
    pub fn hit(&self, pos: egui::Pos2, top: u64) -> Option<(u64, bool)> {
        if !self.rect.contains(pos) {
            return None;
        }
        let line = ((pos.y - self.rect.top()) / LINE) as u64;
        let column = ((pos.x - self.rect.left()) / self.glyph).floor() as usize;
        let first = self.hex_column(0);
        let text = self.text_column();
        let i = if column >= text {
            Some((column - text, true))
        } else if column >= first {
            // Eight bytes take 25 characters: "xx " each and one more space.
            let within = column - first;
            Some(((within / 25) * 8 + (within % 25 / 3).min(7), false))
        } else {
            None
        }?;
        (i.0 < self.per).then(|| ((top + line) * self.per as u64 + i.0 as u64, i.1))
    }
}

/// Paint the lines of `view` shown in `grid`: selected bytes on the grid's
/// selection fill, the cursor outlined, unloaded lines as placeholders.
pub(in crate::app) fn paint(
    ui: &egui::Ui,
    grid: &Grid,
    view: &View,
    color: egui::Color32,
    changed: &dyn Fn(u64) -> bool,
) {
    let painter = ui
        .painter()
        .with_clip_rect(grid.rect.intersect(ui.clip_rect()));
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let selection = ui.visuals().selection;
    let len = view.source.len();
    let per = grid.per as u64;
    for line in 0..grid.lines() {
        let offset = (view.top + line as u64) * per;
        if offset >= len.max(1) {
            break;
        }
        let row = egui::Rect::from_min_size(
            egui::pos2(grid.rect.left(), grid.line_top(line)),
            egui::vec2(grid.rect.width(), LINE),
        );
        let Some(bytes) = view.bytes(offset, grid.per) else {
            presentation::placeholder_row(&painter, row, line, tokens.raised);
            continue;
        };
        for i in 0..bytes.len() {
            let at = offset + i as u64;
            if view.selection().contains(&at) && len > 0 {
                for characters in [false, true] {
                    painter.rect_filled(grid.cell(line, i, characters), 0.0, selection.bg_fill);
                }
            }
            if at == view.cursor {
                let cell = grid.cell(line, i, view.characters).expand(1.0);
                painter.rect_stroke(cell, 1.0, selection.stroke, egui::StrokeKind::Inside);
            }
        }
        let colors = preview::Colors {
            text: color,
            faint: tokens.faint,
            pending: tokens.pending,
        };
        let marks: Vec<bool> = (0..bytes.len())
            .map(|i| changed(offset + i as u64))
            .collect();
        let job =
            preview::hex_line_marked(ui, offset, grid.digits, &bytes, grid.per, colors, &marks);
        let galley = painter.layout_job(job);
        let y = grid.line_top(line) + (LINE - galley.size().y) / 2.0;
        painter.galley(egui::pos2(grid.rect.left(), y), galley, color);
    }
}

/// A thin scroll bar for `total` lines of which `shown` are visible from
/// `top`; returns the first line a drag asks for.
pub(in crate::app) fn scrollbar(
    ui: &mut egui::Ui,
    track: egui::Rect,
    top: u64,
    shown: usize,
    total: u64,
) -> Option<i64> {
    if total <= shown as u64 {
        return None;
    }
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let fraction = |line: u64| (line as f64 / total as f64) as f32;
    let height = (track.height() * fraction(shown as u64)).max(12.0);
    let y = track.top()
        + (track.height() - height) * (top as f64 / (total - shown as u64) as f64) as f32;
    let thumb = egui::Rect::from_min_size(
        egui::pos2(track.left(), y),
        egui::vec2(track.width(), height),
    );
    let response = ui.interact(
        track,
        ui.scope_id().with("value-scroll"),
        egui::Sense::drag(),
    );
    let color = if response.dragged() || response.hovered() {
        tokens.muted
    } else {
        tokens.line_strong
    };
    ui.painter().rect_filled(thumb, track.width() / 2.0, color);
    let pointer = response
        .interact_pointer_pos()
        .filter(|_| response.dragged())?;
    let along =
        ((pointer.y - track.top() - height / 2.0) / (track.height() - height)).clamp(0.0, 1.0);
    Some((f64::from(along) * (total - shown as u64) as f64).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::Grid;
    use eframe::egui;

    #[test]
    fn bytes_and_characters_are_hit_where_they_are_drawn() {
        let ctx = egui::Context::default();
        crate::app::test_ui::render(&ctx, egui::vec2(800.0, 400.0), vec![], |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(560.0, 160.0));
            let grid = Grid::new(ui, rect, 1000);
            assert_eq!((grid.per, grid.digits, grid.lines()), (16, 4, 10));
            for (line, i) in [(0, 0), (0, 7), (0, 8), (0, 15), (3, 9), (9, 15)] {
                for characters in [false, true] {
                    let cell = grid.cell(line, i, characters);
                    let at = (line * 16 + i) as u64 + 5 * 16;
                    assert_eq!(
                        grid.hit(cell.center(), 5),
                        Some((at, characters)),
                        "{line} {i} {characters}"
                    );
                }
            }
            // The gap between two groups of eight belongs to the eighth byte.
            let gap = grid.cell(0, 7, false).right_center() + egui::vec2(grid.glyph * 1.5, 0.0);
            assert_eq!(grid.hit(gap, 0), Some((7, false)));
            // Offsets and space outside the grid hit nothing.
            assert_eq!(grid.hit(rect.left_center() + egui::vec2(2.0, 0.0), 0), None);
            assert_eq!(grid.hit(egui::pos2(5.0, 5.0), 0), None);
            let narrow = Grid::new(
                ui,
                egui::Rect::from_min_size(rect.min, egui::vec2(300.0, 100.0)),
                100_000,
            );
            assert_eq!((narrow.per, narrow.digits), (8, 6));
        });
    }
}
