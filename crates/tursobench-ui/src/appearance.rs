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
    applied: Option<(Mode, bool, Palette, u16)>,
}
impl Appearance {
    pub fn new(ctx: &egui::Context, observe: bool) -> Self {
        Self {
            palette: Palette::default(),
            rx: observe.then(|| system::observe(ctx.clone())),
            applied: None,
        }
    }
    pub fn update(&mut self, ctx: &egui::Context, mode: Mode, font_size: u16) {
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
        let next = (mode, dark, self.palette.clone(), font_size);
        if self.applied.as_ref() != Some(&next) {
            apply(ctx, dark, &self.palette);
            ctx.style_mut_of(
                if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                },
                |style| {
                    for font in style.text_styles.values_mut() {
                        font.size *= f32::from(font_size.clamp(10, 20)) / 14.0;
                    }
                },
            );
            self.applied = Some(next);
        }
    }
}

/// The four interface roles follow the desktop: accent, neutral surfaces,
/// warning/pending, and relationship/information. Custom painters use the same
/// resolved colours as native widgets, including live palette changes.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub primary: egui::Color32,
    pub primary_ink: egui::Color32,
    pub primary_text: egui::Color32,
    pub pending: egui::Color32,
    pub relationship: egui::Color32,
    pub raised: egui::Color32,
    pub raised_hover: egui::Color32,
    pub line: egui::Color32,
    pub line_soft: egui::Color32,
    pub line_strong: egui::Color32,
    pub muted: egui::Color32,
    pub faint: egui::Color32,
}
impl Tokens {
    pub fn of(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp(egui::Id::unique("interface-tokens")))
            .unwrap_or_else(|| Self::from_visuals(&ctx.global_style().visuals))
    }
    fn from_visuals(v: &egui::Visuals) -> Self {
        let bg = v.panel_fill;
        let fg = v.text_color();
        Self {
            primary: v.hyperlink_color,
            primary_ink: contrasting_ink(v.hyperlink_color),
            primary_text: v.hyperlink_color,
            pending: v.warn_fg_color,
            relationship: v.hyperlink_color,
            raised: mix(bg, fg, 0.045),
            raised_hover: mix(bg, fg, 0.09),
            line: mix(bg, fg, 0.13),
            line_soft: mix(bg, fg, 0.065),
            line_strong: mix(bg, fg, 0.24),
            muted: mix(bg, fg, 0.65),
            faint: mix(bg, fg, 0.46),
        }
    }
}

pub fn mix(a: egui::Color32, b: egui::Color32, amount: f32) -> egui::Color32 {
    let channel =
        |a: u8, b: u8| (f32::from(a) * (1.0 - amount) + f32::from(b) * amount).round() as u8;
    egui::Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}
fn luminance(c: egui::Color32) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}
fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn contrasting_ink(fill: egui::Color32) -> egui::Color32 {
    if contrast(fill, egui::Color32::BLACK) > contrast(fill, egui::Color32::WHITE) {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}
// Keep theme hues but make their small labels legible on all interface surfaces.
fn readable(
    color: egui::Color32,
    surfaces: &[egui::Color32],
    text: egui::Color32,
) -> egui::Color32 {
    (0..=20)
        .map(|step| mix(color, text, step as f32 / 20.0))
        .find(|c| surfaces.iter().all(|bg| contrast(*c, *bg) >= 4.5))
        .unwrap_or(text)
}

fn apply(ctx: &egui::Context, dark: bool, palette: &Palette) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    // Forced schemes never combine light text with a desktop's dark surfaces.
    let colors = if palette.dark == Some(dark) {
        palette.colors.clone()
    } else {
        Default::default()
    };
    let color = |names: &[&str], fallback| {
        names
            .iter()
            .find_map(|name| colors.get(*name).copied())
            .or_else(|| {
                // An explicit opposite scheme keeps the desktop's neutral hue,
                // adapting its brightness rather than importing a fixed palette.
                (palette.dark.is_some() && palette.dark != Some(dark))
                    .then(|| {
                        names
                            .iter()
                            .find_map(|name| palette.colors.get(*name).copied())
                            .map(|source| {
                                let gray = ((u16::from(source.r())
                                    + u16::from(source.g())
                                    + u16::from(source.b()))
                                    / 3) as u8;
                                mix(egui::Color32::from_gray(255 - gray), source, 0.04)
                            })
                    })
                    .flatten()
            })
            .unwrap_or(fallback)
    };
    visuals.panel_fill = color(
        &["window_bg_color", "theme_bg_color"],
        if dark {
            egui::Color32::from_rgb(20, 27, 24)
        } else {
            egui::Color32::from_rgb(246, 248, 246)
        },
    );
    visuals.window_fill = visuals.panel_fill;
    visuals.extreme_bg_color = color(
        &["view_bg_color", "theme_base_color"],
        if dark {
            egui::Color32::from_rgb(12, 18, 16)
        } else {
            egui::Color32::WHITE
        },
    );
    let text = color(
        &["window_fg_color", "theme_fg_color"],
        if dark {
            egui::Color32::from_rgb(226, 236, 230)
        } else {
            egui::Color32::from_rgb(37, 54, 45)
        },
    );
    visuals.override_text_color = Some(text);
    // Accent and semantic hues remain desktop-derived even with an explicit
    // opposite scheme; only incompatible surface/text colours are excluded.
    let semantic = |names: &[&str], fallback| {
        names
            .iter()
            .find_map(|name| palette.colors.get(*name).copied())
            .unwrap_or(fallback)
    };
    visuals.hyperlink_color = semantic(
        &["accent_color", "theme_selected_bg_color"],
        if dark {
            egui::Color32::from_rgb(135, 211, 173)
        } else {
            egui::Color32::from_rgb(28, 121, 82)
        },
    );
    let mut tokens = Tokens::from_visuals(&visuals);
    tokens.raised = color(&["card_bg_color", "insensitive_bg_color"], tokens.raised);
    tokens.raised_hover = mix(tokens.raised, text, 0.055);
    tokens.line = color(&["borders"], tokens.line);
    tokens.line_strong = mix(tokens.line, text, 0.16);
    tokens.line_soft = mix(visuals.extreme_bg_color, tokens.line, 0.55);
    let surfaces = [visuals.panel_fill, visuals.extreme_bg_color, tokens.raised];
    tokens.primary_text = readable(tokens.primary, &surfaces, text);
    visuals.hyperlink_color = tokens.primary_text;
    tokens.pending = readable(
        semantic(
            &["warning_color", "warning_bg_color"],
            visuals.warn_fg_color,
        ),
        &surfaces,
        text,
    );
    // Home Manager exports success_color; themes may instead export a link/info
    // role. Without either, derive a distinct hue from the current accent.
    let mut alternative = egui::ecolor::Hsva::from(tokens.primary);
    alternative.h = (alternative.h + 0.22) % 1.0;
    tokens.relationship = readable(
        semantic(
            &["link_color", "info_color", "success_color"],
            alternative.into(),
        ),
        &surfaces,
        text,
    );
    tokens.muted = readable(tokens.muted, &surfaces, text);
    tokens.faint = readable(tokens.faint, &surfaces, text);
    tokens.primary_ink = semantic(
        &["accent_fg_color", "theme_selected_fg_color"],
        tokens.primary_ink,
    );
    if contrast(tokens.primary, tokens.primary_ink) < 4.5 {
        tokens.primary_ink = contrasting_ink(tokens.primary);
    }
    visuals.warn_fg_color = tokens.pending;
    visuals.error_fg_color = readable(
        semantic(
            &["error_color", "destructive_color"],
            visuals.error_fg_color,
        ),
        &surfaces,
        text,
    );
    visuals.weak_text_color = Some(tokens.muted);
    visuals.faint_bg_color = tokens.raised;
    visuals.selection.bg_fill = tokens
        .primary
        .gamma_multiply(if dark { 0.13 } else { 0.10 });
    visuals.selection.stroke = egui::Stroke::new(1.0, tokens.primary);
    if palette.contrast {
        tokens.line = text;
        tokens.line_soft = text;
        tokens.line_strong = text;
        tokens.muted = text;
        tokens.faint = text;
        visuals.weak_text_color = Some(text);
    }
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(6);
        widget.fg_stroke.color = text;
        widget.bg_stroke = egui::Stroke::new(1.0, tokens.line);
    }
    visuals.widgets.noninteractive.bg_fill = visuals.panel_fill;
    visuals.widgets.inactive.bg_fill = tokens.raised;
    visuals.widgets.inactive.weak_bg_fill = tokens.raised;
    visuals.widgets.hovered.bg_fill = tokens.raised_hover;
    visuals.widgets.hovered.weak_bg_fill = tokens.raised_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, tokens.line_strong);
    visuals.widgets.active.bg_fill = visuals.selection.bg_fill;
    visuals.widgets.active.weak_bg_fill = visuals.selection.bg_fill;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, tokens.primary);
    visuals.widgets.open = visuals.widgets.active;
    visuals.window_stroke = egui::Stroke::new(1.0, tokens.line_strong);
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.menu_corner_radius = egui::CornerRadius::same(10);
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    ctx.set_visuals(visuals);
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("interface-tokens"), tokens));
    ctx.style_mut_of(theme, |s| {
        s.text_styles = egui::Style::default().text_styles;
        s.spacing.item_spacing = egui::vec2(7.0, 6.0);
        s.spacing.button_padding = egui::vec2(11.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.menu_margin = egui::Margin::same(5);
        s.spacing.window_margin = egui::Margin::same(16);
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
        // egui draws window titles in the heading style. Nothing else uses it,
        // so it matches the tile ribbons and panel headings instead of
        // towering over them.
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(14.0));
    });
}

/// Custom-painted text follows the same size preference as native egui widgets.
pub fn text_size(ctx: &egui::Context, base: f32) -> f32 {
    base * ctx.global_style().text_styles[&egui::TextStyle::Body].size / 14.0
}

pub fn proportional(ctx: &egui::Context, base: f32) -> egui::FontId {
    egui::FontId::proportional(text_size(ctx, base))
}

pub fn monospace(ctx: &egui::Context, base: f32) -> egui::FontId {
    egui::FontId::monospace(text_size(ctx, base))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_roles_follow_palette_changes_and_remain_readable_in_both_schemes() {
        let ctx = egui::Context::default();
        let mut palette = Palette {
            dark: Some(true),
            ..Default::default()
        };
        for (name, rgb) in [
            ("window_bg_color", (0, 0, 0)),
            ("window_fg_color", (215, 222, 232)),
            ("card_bg_color", (20, 27, 34)),
            ("accent_color", (47, 140, 255)),
            ("warning_color", (245, 158, 11)),
            ("success_color", (63, 185, 80)),
        ] {
            palette
                .colors
                .insert(name.into(), egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2));
        }
        for dark in [true, false] {
            apply(&ctx, dark, &palette);
            let t = Tokens::of(&ctx);
            let v = &ctx.global_style().visuals;
            assert_eq!(t.primary, palette.colors["accent_color"]);
            assert!(contrast(t.primary, t.primary_ink) >= 4.5);
            for bg in [v.panel_fill, v.extreme_bg_color, t.raised] {
                assert!(
                    contrast(t.primary_text, bg) >= 4.5,
                    "dark={dark} primary={:?} bg={bg:?} text={:?} ratio={}",
                    t.primary_text,
                    v.text_color(),
                    contrast(t.primary_text, bg)
                );
                assert!(contrast(t.pending, bg) >= 4.5);
                assert!(contrast(t.relationship, bg) >= 4.5);
                assert!(contrast(t.muted, bg) >= 4.5);
            }
            assert_ne!(t.relationship, t.primary);
        }
        palette.colors.insert(
            "success_color".into(),
            egui::Color32::from_rgb(180, 130, 240),
        );
        apply(&ctx, true, &palette);
        assert_eq!(
            Tokens::of(&ctx).relationship,
            palette.colors["success_color"]
        );
        palette.contrast = true;
        apply(&ctx, true, &palette);
        assert_eq!(
            Tokens::of(&ctx).line,
            ctx.global_style().visuals.text_color()
        );
    }
}
