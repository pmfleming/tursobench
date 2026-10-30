use eframe::egui;
use std::sync::mpsc::{self, Receiver};
pub fn theme(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let rgb = egui::Color32::from_rgb;
    visuals.panel_fill = if dark {
        rgb(20, 29, 32)
    } else {
        rgb(244, 247, 247)
    };
    visuals.window_fill = if dark {
        rgb(29, 41, 46)
    } else {
        rgb(252, 253, 253)
    };
    visuals.extreme_bg_color = if dark {
        rgb(13, 21, 24)
    } else {
        rgb(255, 255, 255)
    };
    visuals.faint_bg_color = if dark {
        rgb(25, 36, 40)
    } else {
        rgb(235, 242, 243)
    };
    visuals.override_text_color = Some(if dark {
        rgb(220, 230, 232)
    } else {
        rgb(22, 32, 36)
    });
    visuals.selection.bg_fill = if dark {
        rgb(22, 58, 63)
    } else {
        rgb(211, 236, 238)
    };
    visuals.selection.stroke = egui::Stroke::new(
        1.5,
        if dark {
            rgb(76, 184, 194)
        } else {
            rgb(11, 116, 128)
        },
    );
    visuals.hyperlink_color = visuals.selection.stroke.color;
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.set_visuals(visuals);
    ctx.style_mut_of(
        if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        },
        |s| {
            s.spacing.item_spacing = egui::vec2(7.0, 6.0);
            s.spacing.button_padding = egui::vec2(9.0, 5.0);
            s.text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
            s.text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
            s.text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        },
    );
}
/// Font discovery/file loading stays off the UI thread. No network or bundled new fonts.
pub fn system_fonts(ctx: egui::Context) -> Receiver<(egui::FontDefinitions, String)> {
    let (tx, rx) = mpsc::sync_channel(1);
    if cfg!(test) {
        return rx;
    }
    std::thread::spawn(move || {
        let mut fonts = egui::FontDefinitions::default();
        let mut loaded = Vec::new();
        for (family, generic) in [
            (egui::FontFamily::Proportional, "sans-serif"),
            (egui::FontFamily::Monospace, "monospace"),
        ] {
            let path = std::process::Command::new("fc-match")
                .args(["-f", "%{file}\n%{index}\n", generic])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok());
            let Some(path) = path else {
                continue;
            };
            let mut lines = path.lines();
            let Some(path) = lines.next() else {
                continue;
            };
            let index = lines
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
                & 0xffff;
            if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 16 * 1024 * 1024)
                && let Ok(bytes) = std::fs::read(path)
                && bytes.len() <= 16 * 1024 * 1024
                && skrifa::FontRef::from_index(&bytes, index).is_ok()
            {
                let name = format!("system-{generic}");
                let mut data = egui::FontData::from_owned(bytes);
                data.index = index;
                fonts.font_data.insert(name.clone(), data.into());
                fonts.families.entry(family).or_default().insert(0, name);
                loaded.push(generic);
            }
        }
        let label = if loaded.is_empty() {
            "Built-in font fallback (system font discovery unavailable)".into()
        } else {
            format!(
                "System fonts: {} · built-in glyph fallbacks",
                loaded.join(", ")
            )
        };
        let _ = tx.send((fonts, label));
        ctx.request_repaint();
    });
    rx
}
