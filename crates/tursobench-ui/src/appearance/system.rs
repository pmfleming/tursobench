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
    let mut palette = Palette::default();
    if !cfg!(target_os = "linux") {
        return palette;
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| Path::new(&p).join(".config")))
    {
        // GTK 4 overrides GTK 3 where both supply supported named colors.
        for version in ["gtk-3.0", "gtk-4.0"] {
            palette.colors.extend(named_colors(&bounded_file(
                &config.join(version).join("gtk.css"),
            )));
            let settings = bounded_file(&config.join(version).join("settings.ini"));
            if let Some(theme) = settings
                .lines()
                .find_map(|l| l.strip_prefix("gtk-theme-name="))
            {
                palette.dark = Some(theme.to_lowercase().contains("dark"));
            }
        }
        if let Some(bg) = palette
            .colors
            .get("window_bg_color")
            .or_else(|| palette.colors.get("theme_bg_color"))
        {
            palette.dark = Some(
                u32::from(bg.r()) * 299 + u32::from(bg.g()) * 587 + u32::from(bg.b()) * 114
                    < 128_000,
            );
        }
    }
    let palette_scheme = palette.dark;
    if let Some(scheme) = portal("color-scheme") {
        match scheme.as_slice() {
            [kind, value] if kind == "u" && value == "1" => palette.dark = Some(true),
            [kind, value] if kind == "u" && value == "2" => palette.dark = Some(false),
            _ => {}
        }
    }
    if palette_scheme != palette.dark {
        palette.colors.clear();
    }
    palette.contrast = portal("contrast").is_some_and(|p| p == ["u", "1"]);
    if let Some(accent) = portal("accent-color")
        && accent.first().is_some_and(|s| s == "(ddd)")
        && accent.len() == 4
    {
        let channels: Option<Vec<f32>> = accent[1..]
            .iter()
            .map(|s| {
                s.parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            })
            .collect();
        if let Some(c) = channels {
            palette.colors.insert(
                "accent_color".into(),
                egui::Color32::from_rgb(
                    (c[0] * 255.0).round() as u8,
                    (c[1] * 255.0).round() as u8,
                    (c[2] * 255.0).round() as u8,
                ),
            );
        }
    }
    palette
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
    // Strip comments so commented-out declarations cannot override the current palette.
    let mut clean = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        clean.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("*/") else {
            rest = "";
            break;
        };
        rest = &rest[start + end + 4..];
    }
    clean.push_str(rest);
    let definitions: Vec<_> = clean
        .split("@define-color")
        .skip(1)
        .filter_map(|part| {
            part.split_once(';')
                .map(|(declaration, _)| declaration.trim())
        })
        .collect();
    // Resolve bounded chains of named aliases, including forward references.
    for _ in 0..8 {
        for declaration in &definitions {
            let Some((name, value)) = declaration.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim();
            let parsed = if let Some(alias) = value.strip_prefix('@') {
                colors.get(alias).copied()
            } else if let Some(hex) = value.strip_prefix('#') {
                match hex.len() {
                    6 => u32::from_str_radix(hex, 16)
                        .ok()
                        .map(|n| egui::Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)),
                    3 => u16::from_str_radix(hex, 16).ok().map(|n| {
                        egui::Color32::from_rgb(
                            ((n >> 8) & 15) as u8 * 17,
                            ((n >> 4) & 15) as u8 * 17,
                            (n & 15) as u8 * 17,
                        )
                    }),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(color) = parsed {
                colors.insert(name.into(), color);
            }
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;
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
