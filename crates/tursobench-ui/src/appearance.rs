use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{self, Receiver};
mod system;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    System,
    Light,
    Dark,
}
impl Mode {
    pub fn from_dark(dark: bool) -> Self {
        if dark { Self::Dark } else { Self::Light }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Palette {
    pub dark: Option<bool>,
    pub contrast: bool,
    pub colors: std::collections::BTreeMap<String, egui::Color32>,
}

pub struct Appearance {
    pub palette: Palette,
    rx: Option<Receiver<Palette>>,
    applied: Option<(Mode, bool, Palette)>,
}
impl Appearance {
    pub fn new(ctx: &egui::Context, observe: bool) -> Self {
        Self {
            palette: Palette::default(),
            rx: observe.then(|| system::observe(ctx.clone())),
            applied: None,
        }
    }
    pub fn update(&mut self, ctx: &egui::Context, mode: Mode) {
        if let Some(rx) = &self.rx {
            while let Ok(palette) = rx.try_recv() {
                self.palette = palette;
            }
        }
        let dark = match mode {
            Mode::System => self.palette.dark.unwrap_or_else(|| {
                ctx.system_theme().unwrap_or(egui::Theme::Dark) == egui::Theme::Dark
            }),
            Mode::Dark => true,
            Mode::Light => false,
        };
        let next = (mode, dark, self.palette.clone());
        if self.applied.as_ref() != Some(&next) {
            apply(ctx, dark, &self.palette);
            self.applied = Some(next);
        }
    }
}

fn apply(ctx: &egui::Context, dark: bool, palette: &Palette) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    // A forced scheme must never combine light text with a desktop's dark surfaces.
    let colors = if palette.dark == Some(dark) {
        palette.colors.clone()
    } else {
        Default::default()
    };
    let color = |names: &[&str], fallback| {
        names
            .iter()
            .find_map(|name| colors.get(*name).copied())
            .unwrap_or(fallback)
    };
    visuals.panel_fill = color(&["window_bg_color", "theme_bg_color"], visuals.panel_fill);
    visuals.window_fill = color(&["window_bg_color", "theme_bg_color"], visuals.window_fill);
    visuals.extreme_bg_color = color(
        &["view_bg_color", "theme_base_color"],
        visuals.extreme_bg_color,
    );
    visuals.faint_bg_color = color(
        &["card_bg_color", "insensitive_bg_color"],
        visuals.faint_bg_color,
    );
    let text = color(&["window_fg_color", "theme_fg_color"], visuals.text_color());
    visuals.override_text_color = Some(text);
    let accent = color(
        &["accent_color", "theme_selected_bg_color"],
        if dark {
            egui::Color32::from_rgb(94, 167, 255)
        } else {
            egui::Color32::from_rgb(20, 105, 202)
        },
    );
    visuals.selection.bg_fill = accent.gamma_multiply(if dark { 0.3 } else { 0.18 });
    visuals.selection.stroke = egui::Stroke::new(1.5, accent);
    visuals.hyperlink_color = accent;
    visuals.warn_fg_color = color(&["warning_color"], visuals.warn_fg_color);
    visuals.error_fg_color = color(
        &["error_color", "destructive_color"],
        visuals.error_fg_color,
    );
    let border = color(&["borders"], visuals.widgets.noninteractive.bg_stroke.color);
    let control = color(
        &["card_bg_color", "insensitive_bg_color"],
        visuals.widgets.inactive.bg_fill,
    );
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(3);
        widget.fg_stroke.color = text;
        widget.bg_stroke.color = if palette.contrast { text } else { border };
    }
    visuals.widgets.inactive.bg_fill = control;
    visuals.widgets.inactive.weak_bg_fill = control;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5, accent);
    visuals.window_stroke.color = if palette.contrast { text } else { border };
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(theme, |s| {
        s.spacing.item_spacing = egui::vec2(7.0, 6.0);
        s.spacing.button_padding = egui::vec2(9.0, 5.0);
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    });
}
#[cfg(test)]
pub fn theme(ctx: &egui::Context, dark: bool) {
    apply(ctx, dark, &Palette::default());
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
