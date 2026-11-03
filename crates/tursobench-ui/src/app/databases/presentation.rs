use super::{DatabaseKind, Entry};
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::Path;

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
}

/// Keep the two nearest containing folders; shallow and relative paths stay valid.
pub(super) fn folder_label(path: &Path) -> String {
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
    if mark == EngineMark::OpenedTurso {
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
    use super::{DatabaseKind, EngineMark, Entry, Path, folder_label};
    use crate::{
        app::databases::{Chooser, State},
        discovery::Availability,
        storage::DiscoveryPreferences,
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
    fn only_successful_open_history_gets_the_checked_turso_mark() {
        let mut entry = Entry {
            path: "/work/example.db".into(),
            state: State::Found,
            size: None,
            last_opened: None,
            kind: DatabaseKind::Unknown,
        };
        assert_eq!(entry.engine_mark(), EngineMark::Other);
        entry.kind = DatabaseKind::LikelyTurso;
        assert_eq!(entry.engine_mark(), EngineMark::LikelyTurso);
        entry.kind = DatabaseKind::LikelySqlite;
        assert_eq!(entry.engine_mark(), EngineMark::Other);

        let mut prefs = DiscoveryPreferences::default();
        prefs.record_open(entry.path.clone(), 1);
        let restored = Chooser::new(&prefs);
        assert_eq!(restored.entries[0].engine_mark(), EngineMark::OpenedTurso);
        entry.last_opened = Some(1);
        assert_eq!(entry.engine_mark(), EngineMark::OpenedTurso);
        entry.state = State::Checked(Availability::NotDatabase);
        assert_eq!(entry.engine_mark(), EngineMark::Other);
    }
}
