use super::Palette;
use eframe::egui;
use std::{collections::BTreeMap, io::Read, path::Path, sync::mpsc, time::Duration};

/// Desktop IO is bounded and off the render thread. A dropped receiver stops the observer.
pub(super) fn observe(ctx: egui::Context) -> mpsc::Receiver<Palette> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut delivered = None;
        loop {
            if !publish_palette(&tx, &mut delivered, read(), || ctx.request_repaint()) {
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    rx
}

/// Keep checking receiver lifetime, but only wake the UI for a changed palette.
fn publish_palette(
    tx: &mpsc::SyncSender<Palette>,
    delivered: &mut Option<Palette>,
    palette: Palette,
    wake: impl FnOnce(),
) -> bool {
    let changed = delivered.as_ref() != Some(&palette);
    match tx.try_send(palette.clone()) {
        Ok(()) => *delivered = Some(palette),
        Err(mpsc::TrySendError::Disconnected(_)) => return false,
        // Wake to drain the stale sample; retry the current palette on the next poll.
        Err(mpsc::TrySendError::Full(_)) => {}
    }
    if changed {
        wake();
    }
    true
}

fn bounded_file(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(128 * 1024).read_to_string(&mut text);
    }
    text
}

fn read() -> Palette {
    if !cfg!(target_os = "linux") {
        return Palette::default();
    }
    let mut palette =
        crate::storage::config_home().map_or_else(Palette::default, |config| gtk_palette(&config));
    desktop_settings(&mut palette, portal);
    palette
}

/// Named colours and the scheme from the GTK 3 and 4 settings under `config`.
fn gtk_palette(config: &Path) -> Palette {
    let mut palette = Palette::default();
    // GTK 4 overrides GTK 3 where both supply supported named colors.
    for version in ["gtk-3.0", "gtk-4.0"] {
        let folder = config.join(version);
        let css = bounded_file(&folder.join("gtk.css"));
        palette.colors.extend(named_colors(&css));
        let settings = bounded_file(&folder.join("settings.ini"));
        if let Some(theme) = settings
            .lines()
            .find_map(|l| l.strip_prefix("gtk-theme-name="))
        {
            palette.dark = Some(theme.to_lowercase().contains("dark"));
        }
    }
    let colors = &palette.colors;
    if let Some(bg) = colors
        .get("window_bg_color")
        .or_else(|| colors.get("theme_bg_color"))
    {
        let luma = u32::from(bg.r()) * 299 + u32::from(bg.g()) * 587 + u32::from(bg.b()) * 114;
        palette.dark = Some(luma < 128_000);
    }
    palette
}

/// The desktop portal's scheme, contrast and accent. GTK colours meant for
/// the other scheme are dropped.
fn desktop_settings(palette: &mut Palette, portal: impl Fn(&str) -> Option<Vec<String>>) {
    let gtk_scheme = palette.dark;
    match portal("color-scheme").as_deref() {
        Some([kind, value]) if kind == "u" && value == "1" => palette.dark = Some(true),
        Some([kind, value]) if kind == "u" && value == "2" => palette.dark = Some(false),
        _ => {}
    }
    if gtk_scheme != palette.dark {
        palette.colors.clear();
    }
    palette.contrast = portal("contrast").is_some_and(|p| p == ["u", "1"]);
    if let Some(accent) = portal("accent-color").as_deref().and_then(portal_color) {
        palette.colors.insert("accent_color".into(), accent);
    }
}

/// A portal colour, `(ddd)` and three channels from zero to one.
fn portal_color(value: &[String]) -> Option<egui::Color32> {
    let [kind, r, g, b] = value else {
        return None;
    };
    if kind != "(ddd)" {
        return None;
    }
    let channel = |s: &String| {
        let v = s.parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v))?;
        Some((v * 255.0).round() as u8)
    };
    Some(egui::Color32::from_rgb(
        channel(r)?,
        channel(g)?,
        channel(b)?,
    ))
}

fn portal(key: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("busctl")
        .args([
            "--user",
            "--timeout=1",
            "call",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            "Read",
            "ss",
            "org.freedesktop.appearance",
            key,
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = std::str::from_utf8(&output.stdout).ok()?;
    Some(text.split_whitespace().skip(1).map(str::to_owned).collect())
}

/// Deliberately a named-color adapter, not a GTK CSS interpreter. Unsupported values fall back.
fn named_colors(css: &str) -> BTreeMap<String, egui::Color32> {
    let mut colors = BTreeMap::new();
    let css = without_comments(css);
    let definitions: Vec<_> = css
        .split("@define-color")
        .skip(1)
        .filter_map(|part| {
            let (declaration, _) = part.split_once(';')?;
            let (name, value) = declaration.trim().split_once(char::is_whitespace)?;
            Some((name, value.trim()))
        })
        .collect();
    // Resolve bounded chains of named aliases, including forward references.
    for _ in 0..8 {
        for &(name, value) in &definitions {
            let parsed = match value.strip_prefix('@') {
                Some(alias) => colors.get(alias).copied(),
                None => value.strip_prefix('#').and_then(hex_color),
            };
            if let Some(color) = parsed {
                colors.insert(name.into(), color);
            }
        }
    }
    colors
}

/// Commented-out declarations must not override the current palette.
fn without_comments(css: &str) -> String {
    let mut clean = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        clean.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("*/") else {
            return clean;
        };
        rest = &rest[start + end + 4..];
    }
    clean + rest
}

/// `rrggbb` or `rgb` hexadecimal digits.
fn hex_color(hex: &str) -> Option<egui::Color32> {
    let n = u32::from_str_radix(hex, 16).ok()?;
    let nibble = |shift: u32| ((n >> shift) & 15) as u8 * 17;
    match hex.len() {
        6 => Some(egui::Color32::from_rgb(
            (n >> 16) as u8,
            (n >> 8) as u8,
            n as u8,
        )),
        3 => Some(egui::Color32::from_rgb(nibble(8), nibble(4), nibble(0))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Palette, desktop_settings, egui, gtk_palette, mpsc, named_colors, publish_palette,
    };
    #[test]
    fn unchanged_palettes_sleep_and_changes_survive_a_full_mailbox() {
        let (tx, rx) = mpsc::sync_channel(1);
        let mut delivered = None;
        let wakes = std::cell::Cell::new(0);
        let wake = || wakes.set(wakes.get() + 1);
        let light = Palette {
            dark: Some(false),
            ..Default::default()
        };
        let dark = Palette {
            dark: Some(true),
            ..Default::default()
        };

        assert!(publish_palette(&tx, &mut delivered, light.clone(), wake));
        assert_eq!(wakes.get(), 1);
        assert_eq!(rx.try_recv().unwrap(), light);
        // An idle UI need not drain unchanged samples.
        for _ in 0..3 {
            assert!(publish_palette(&tx, &mut delivered, light.clone(), wake));
        }
        assert_eq!(wakes.get(), 1);
        assert!(publish_palette(&tx, &mut delivered, dark.clone(), wake));
        assert_eq!(wakes.get(), 2);
        assert_eq!(rx.try_recv().unwrap(), light);
        assert!(publish_palette(&tx, &mut delivered, dark.clone(), wake));
        assert_eq!(rx.try_recv().unwrap(), dark);
        assert_eq!(wakes.get(), 3);
        assert!(publish_palette(&tx, &mut delivered, dark.clone(), wake));
        assert_eq!(wakes.get(), 3);
        drop(rx);
        assert!(!publish_palette(&tx, &mut delivered, dark, wake));
    }
    #[test]
    fn named_colors_resolve_aliases_without_executing_css() {
        let colors = named_colors(
            "@define-color accent_color @blue; /* @define-color blue #f00; */ @define-color blue #2f8cff; @define-color invalid url(secret); @define-color cycle @cycle;",
        );
        assert_eq!(
            colors["accent_color"],
            egui::Color32::from_rgb(47, 140, 255)
        );
        assert!(!colors.contains_key("invalid"));
        assert!(!colors.contains_key("cycle"));
        let short =
            named_colors("@define-color a #fff; @define-color b #12345; /* @define-color c #000;");
        assert_eq!(short.get("a"), Some(&egui::Color32::WHITE));
        assert_eq!(short.len(), 1);
    }
    #[test]
    fn gtk_settings_and_portal_choose_the_scheme_and_accent() {
        let config = tempfile::tempdir().unwrap();
        let write = |file: &str, text: &str| {
            let path = config.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "gtk-4.0/settings.ini",
            "[Settings]\ngtk-theme-name=Adwaita-dark\n",
        );
        write("gtk-3.0/gtk.css", "@define-color accent_color #2f8cff;");
        assert_eq!(gtk_palette(config.path()).dark, Some(true));
        // A light window background outweighs the theme name.
        write("gtk-4.0/gtk.css", "@define-color window_bg_color #fafafa;");
        let mut palette = gtk_palette(config.path());
        let portal = |scheme: &'static str, accent: &'static str| {
            move |key: &str| {
                let value = match key {
                    "color-scheme" => scheme,
                    "contrast" => "u 1",
                    _ => accent,
                };
                Some(value.split_whitespace().map(str::to_owned).collect())
            }
        };
        // Without a scheme or a valid accent, GTK's choices stand.
        desktop_settings(&mut palette, portal("u 0", "(ddd) 2 0 0"));
        assert_eq!((palette.dark, palette.colors.len()), (Some(false), 2));
        // The portal's scheme wins and drops colours made for the other one.
        desktop_settings(&mut palette, portal("u 1", "(ddd) 1 0.5 0"));
        assert!(palette.dark == Some(true) && palette.contrast);
        let accent = egui::Color32::from_rgb(255, 128, 0);
        assert_eq!(palette.colors.values().collect::<Vec<_>>(), [&accent]);
    }
    #[test]
    fn explicit_scheme_does_not_mix_incompatible_surfaces() {
        let ctx = egui::Context::default();
        let mut palette = Palette {
            dark: Some(true),
            ..Default::default()
        };
        palette
            .colors
            .insert("window_bg_color".into(), egui::Color32::BLACK);
        palette
            .colors
            .insert("window_fg_color".into(), egui::Color32::WHITE);
        super::super::apply(&ctx, true, &palette);
        assert_eq!(ctx.global_style().visuals.panel_fill, egui::Color32::BLACK);
        super::super::apply(&ctx, false, &palette);
        assert_ne!(ctx.global_style().visuals.panel_fill, egui::Color32::BLACK);
        assert!(!ctx.global_style().visuals.dark_mode);
    }
}
