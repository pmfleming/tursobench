use super::{DatabaseKind, Entry};
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::Path;

pub(super) const ROW_HEIGHT: f32 = 34.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EngineMark {
    OpenedTurso,
    LikelyTurso,
    Other,
}

impl Entry {
    pub(super) fn engine_mark(&self) -> EngineMark {
        if !self.selectable() {
            EngineMark::Other
        } else if self.last_opened.is_some() {
            // History is recorded only after a successful Turso-engine open.
            EngineMark::OpenedTurso
        } else if self.kind == DatabaseKind::LikelyTurso {
            EngineMark::LikelyTurso
        } else {
            EngineMark::Other
        }
    }

    pub(super) fn engine_label(&self) -> &'static str {
        match self.engine_mark() {
            EngineMark::OpenedTurso => "Opened with Turso",
            EngineMark::LikelyTurso => "Likely Turso",
            EngineMark::Other => self.kind.label(),
        }
    }

    /// One list row: icon, file name, folder and badge; returns the row's
    /// response and whether its menu asked to forget the database.
    pub(super) fn row_ui(
        &self,
        ui: &mut egui::Ui,
        selected: bool,
        highlighted: bool,
        available: bool,
    ) -> (egui::Response, bool) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_HEIGHT),
            egui::Sense::click(),
        );
        if !ui.is_rect_visible(rect) {
            return (response, false);
        }
        let badge = self.badge();
        response.widget_info(|| {
            let badge = badge.map(|b| format!(" · {b}")).unwrap_or_default();
            let name = format!("{} · {}{badge}", self.path.display(), self.engine_label());
            egui::WidgetInfo::selected(egui::Role::ListBoxOption, available, selected, name)
        });
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let visuals = ui.visuals();
        if selected || highlighted || response.hovered() || response.has_focus() {
            let fill = if selected || highlighted {
                visuals.selection.bg_fill
            } else {
                visuals.faint_bg_color
            };
            painter.rect_filled(rect, 6.0, fill);
        }
        let icon = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(14.0, 0.0),
            egui::vec2(22.0, 22.0),
        );
        database_icon(ui, icon, self, available);
        let badge = badge.or((self.kind == DatabaseKind::LikelySqlite).then_some("SQLite"));
        let badge_width = badge.map_or(0.0, |badge| {
            let font = crate::appearance::proportional(ui.ctx(), 9.0);
            let galley = painter.layout_no_wrap(badge.into(), font, visuals.warn_fg_color);
            let width = galley.size().x + 8.0;
            let pos = rect.right_center() - egui::vec2(width, galley.size().y / 2.0);
            painter.galley(pos, galley, visuals.warn_fg_color);
            width
        });
        let name = self.path.file_name().unwrap_or(self.path.as_os_str());
        let color = if available {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        let text_width = (rect.width() - 34.0).max(1.0);
        for (text, y, size, color, width) in [
            (
                name.to_string_lossy().into_owned(),
                1.0,
                12.0,
                color,
                (text_width - badge_width).max(1.0),
            ),
            (
                folder_label(&self.path),
                18.0,
                10.0,
                visuals.weak_text_color(),
                text_width,
            ),
        ] {
            let text = egui::RichText::new(text).size(crate::appearance::text_size(ui.ctx(), size));
            let galley = egui::WidgetText::from(text).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                width,
                egui::TextStyle::Body,
            );
            painter.galley(rect.min + egui::vec2(30.0, y), galley, color);
        }
        let mut forget = false;
        response.context_menu(|ui| {
            ui.label(self.label());
            ui.label(self.engine_label());
            ui.small(self.kind.explanation());
            if let Some(bytes) = self.size {
                ui.small(format!("{} KiB", bytes.div_ceil(1024)));
            }
            if ui.button("Forget database").clicked() {
                forget = true;
                ui.close();
            }
        });
        (response.on_hover_text(self.path.to_string_lossy()), forget)
    }
}

/// Keep the two nearest containing folders; shallow and relative paths stay valid.
pub(in crate::app) fn folder_label(path: &Path) -> String {
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    folder
        .parent()
        .and_then(Path::parent)
        .and_then(|ancestor| folder.strip_prefix(ancestor).ok())
        .unwrap_or(folder)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn database_icon(ui: &egui::Ui, rect: egui::Rect, entry: &Entry, available: bool) {
    let mark = entry.engine_mark();
    if mark == EngineMark::Other {
        let color = if !available {
            ui.visuals().weak_text_color()
        } else if entry.kind == DatabaseKind::LikelySqlite {
            ui.visuals().warn_fg_color
        } else {
            ui.visuals().text_color()
        };
        icons::paint(ui.painter(), rect.shrink(5.0), Icon::Database, color);
        return;
    }

    turso_icon(ui, rect, mark == EngineMark::OpenedTurso, available);
}
pub(in crate::app) fn opened_database_icon(ui: &egui::Ui, rect: egui::Rect, available: bool) {
    turso_icon(ui, rect, true, available);
}
fn turso_icon(ui: &egui::Ui, rect: egui::Rect, checked: bool, available: bool) {
    let texture = turso_texture(ui.ctx(), ui.visuals().dark_mode);
    let size = texture.size_vec2();
    let scale = ((rect.width() - 2.0) / size.x).min((rect.height() - 2.0) / size.y);
    let logo_rect = egui::Rect::from_center_size(rect.center(), size * scale);
    ui.painter().image(
        texture.id(),
        logo_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        if available {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_white_alpha(100)
        },
    );
    if checked {
        let center = rect.right_bottom() - egui::vec2(3.0, 3.0);
        ui.painter()
            .circle_filled(center, 6.5, ui.visuals().panel_fill);
        ui.painter().circle_stroke(
            center,
            6.0,
            egui::Stroke::new(1.0, ui.visuals().hyperlink_color),
        );
        ui.painter().add(egui::Shape::line(
            vec![
                center + egui::vec2(-3.0, 0.0),
                center + egui::vec2(-1.0, 2.0),
                center + egui::vec2(3.0, -2.5),
            ],
            egui::Stroke::new(1.5, ui.visuals().hyperlink_color),
        ));
    }
}

fn turso_texture(ctx: &egui::Context, dark: bool) -> egui::TextureHandle {
    let id = egui::Id::unique(("official-turso-logomark", dark));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let bytes: &[u8] = if dark {
        include_bytes!("../../../assets/turso/turso-logomark-aqua.png")
    } else {
        include_bytes!("../../../assets/turso/turso-logomark-dark.png")
    };
    let icon = eframe::icon_data::from_png_bytes(bytes).expect("bundled Turso logomark");
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    let texture = ctx.load_texture(
        format!("turso-logomark-{dark}"),
        image,
        egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        },
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

#[cfg(test)]
mod tests {
    use super::{DatabaseKind, Entry, Path, egui, folder_label};
    use crate::app::{
        databases::State,
        test_ui::{control, nodes, texts},
    };

    #[test]
    fn subtitles_keep_only_the_two_containing_folders() {
        for (path, label) in [
            (
                "/home/demo/.local/share/clip-daemon/history/history.db",
                "clip-daemon/history",
            ),
            ("/home/demo/.local/state/agenda/agenda.db", "state/agenda"),
            ("/work/project/database.db", "work/project"),
            ("/work/database.db", "/work"),
            ("/database.db", "/"),
            ("database.db", "."),
            ("projects/données/数据库.db", "projects/données"),
        ] {
            assert_eq!(folder_label(Path::new(path)), label);
        }
    }

    #[test]
    fn row_tooltip_is_the_full_path_and_its_menu_describes_and_forgets_the_file() {
        let path = "/home/demo/.local/share/clip-daemon/history/history.db";
        let entry = Entry {
            state: State::Found,
            size: Some(4096),
            kind: DatabaseKind::LikelyTurso,
            ..Entry::remembered(path.into(), Some(1))
        };
        let screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        ));
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            crate::appearance::theme(&ctx, dark);
            let mut time = 0.0;
            let mut frame = |events| {
                let mut forget = false;
                // Long enough to show the tooltip, short enough for a click.
                time += 0.4;
                let input = egui::RawInput {
                    time: Some(time),
                    screen_rect,
                    events,
                    ..Default::default()
                };
                let mut output =
                    ctx.run_ui(input, |ui| forget = entry.row_ui(ui, false, false, true).1);
                output.textures_delta.clear();
                (output, forget)
            };
            let row = egui::pos2(100.0, 30.0);
            frame(vec![egui::Event::PointerMoved(row)]);
            let output = (0..3).map(|_| frame(vec![]).0).last().unwrap();
            let painted: Vec<_> = texts(&output).map(|t| t.galley.job.text.as_str()).collect();
            assert_eq!(painted, ["history.db", "clip-daemon/history", path]);
            assert!(nodes(&output).iter().any(|(_, node)| {
                node.label() == Some(&format!("{path} · Opened with Turso"))
            }));
            for pressed in [true, false] {
                frame(vec![egui::Event::PointerButton {
                    pos: row,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
            let (menu, _) = frame(vec![]);
            let painted: Vec<_> = texts(&menu).map(|t| t.galley.job.text.as_str()).collect();
            for line in [
                "Opened before · header checked",
                "Opened with Turso",
                "4 KiB",
            ] {
                assert!(painted.contains(&line), "{line}: {painted:?}");
            }
            let forget = control(&menu, "Forget database").center();
            let clicks =
                [true, false].map(|pressed| frame(crate::app::test_ui::pointer(forget, pressed)).1);
            assert!(clicks.contains(&true));
        }
    }
}
