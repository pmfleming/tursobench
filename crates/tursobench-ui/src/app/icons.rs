//! Asset-free icons share native button interaction, focus, tooltip and AccessKit names.
use eframe::egui::{self, Pos2, Stroke};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Back,
    Next,
    Search,
    Database,
    Folder,
    Map,
    Data,
    Structure,
    Filter,
    Refresh,
    Count,
    Export,
    Copy,
    Sql,
    History,
    Saved,
    Checks,
    Settings,
    Desktop,
    Sun,
    Moon,
    Close,
    Disconnect,
    Popout,
    Lock,
    Edit,
    Fit,
    Arrange,
    Columns,
    System,
    Plus,
    Selection,
    Explain,
    Parameters,
    Complete,
    Undo,
    Trash,
    Key,
    Link,
    Up,
    Down,
    Minimize,
    Run,
    CloneRecord,
    BeginTransaction,
    CommitTransaction,
    Bytecode,
    ReviewEdits,
    CopyValue,
    FitColumns,
}

pub(super) fn button(ui: &mut egui::Ui, icon: Icon, name: &str, shortcut: &str) -> egui::Response {
    widget(ui, icon, name, shortcut, None, false, 1.0)
}

pub(super) const RIBBON_BUTTON_SIZE: f32 = 30.0;
pub(super) const RIBBON_TEXT_SIZE: f32 = 13.5;

/// A quiet, accessible placeholder that stays centered in the visible tile body.
pub(super) fn empty_state(ui: &mut egui::Ui, icon: Icon, description: &str) {
    let rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    empty_state_at(ui, rect, icon, description);
    ui.allocate_space(rect.size().max(egui::Vec2::ZERO));
}

pub(super) fn empty_state_at(ui: &mut egui::Ui, rect: egui::Rect, icon: Icon, description: &str) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let size = (rect.width().min(rect.height()) - 44.0).clamp(0.0, 40.0);
    if size <= 0.0 {
        return;
    }
    let glyph = egui::Rect::from_center_size(
        rect.center() - egui::vec2(0.0, 12.0),
        egui::Vec2::splat(size),
    );
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    painter.rect_filled(glyph.expand(8.0), 12.0, tokens.raised);
    paint(&painter, glyph, icon, tokens.muted);
    let galley = painter.layout(
        description.to_owned(),
        crate::appearance::proportional(ui.ctx(), 12.0),
        tokens.muted,
        (rect.width() - 24.0).max(1.0),
    );
    painter.galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            glyph.bottom() + 15.0,
        ),
        galley,
        tokens.muted,
    );
    let response = ui.interact(
        glyph,
        ui.scope_id().with("empty-state"),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, description));
    response.on_hover_text(description);
}

/// Compact tile actions keep the same native focus and accessibility behavior.
pub(super) fn ribbon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    name: &str,
    shortcut: &str,
    selected: Option<bool>,
) -> egui::Response {
    widget(
        ui,
        icon,
        name,
        shortcut,
        selected,
        false,
        RIBBON_BUTTON_SIZE / 28.0,
    )
}

pub(super) fn control(
    ui: &mut egui::Ui,
    icon: Icon,
    name: &str,
    shortcut: &str,
    selected: bool,
    label: bool,
) -> egui::Response {
    widget(ui, icon, name, shortcut, Some(selected), label, 1.0)
}

fn widget(
    ui: &mut egui::Ui,
    icon: Icon,
    name: &str,
    shortcut: &str,
    selected: Option<bool>,
    label: bool,
    scale: f32,
) -> egui::Response {
    let text = if label {
        format!("     {name}")
    } else {
        String::new()
    };
    let primary = matches!(icon, Icon::Run);
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let mut native = egui::Button::new(text)
        .frame_when_inactive(selected == Some(true) || primary)
        .min_size(egui::vec2(28.0, 28.0) * scale);
    if let Some(selected) = selected {
        native = native.selected(selected);
    }
    if primary {
        native = native.fill(tokens.primary);
    }
    let response = ui.add(native);
    response.widget_info(|| match selected {
        Some(selected) => {
            egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, name)
        }
        None => egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name),
    });
    let rect = egui::Rect::from_center_size(
        egui::pos2(
            response.rect.left() + 14.0 * scale,
            response.rect.center().y,
        ),
        egui::Vec2::splat(if scale > 1.0 { 18.0 } else { 16.0 }),
    );
    paint(
        ui.painter(),
        rect,
        icon,
        if primary {
            tokens.primary_ink
        } else {
            ui.style().interact(&response).fg_stroke.color
        },
    );
    response.on_hover_text(if shortcut.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {shortcut}")
    })
}

/// Compact, labelled navigation rail with full accessible names at narrow widths.
pub(super) fn rail_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    name: &str,
    selected: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, name)
    });
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let color = if selected {
        tokens.primary_text
    } else {
        tokens.muted
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if selected || response.hovered() || response.has_focus() {
        painter.rect_filled(
            rect,
            8.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                tokens.raised
            },
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            8.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    paint(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 14.0),
            egui::vec2(18.0, 18.0),
        ),
        icon,
        color,
    );
    let text = painter.layout_no_wrap(
        label.into(),
        crate::appearance::proportional(ui.ctx(), 9.0),
        color,
    );
    painter.galley(
        egui::pos2(rect.center().x - text.size().x / 2.0, rect.top() + 29.0),
        text,
        color,
    );
    response.on_hover_text(name)
}

pub(super) fn paint(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: egui::Color32) {
    let p =
        |x: f32, y: f32| rect.min + egui::vec2(x * rect.width() / 24.0, y * rect.height() / 24.0);
    let stroke = Stroke::new(1.4 * (rect.width() / 16.0).max(1.0), color);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect::<Vec<Pos2>>(),
            stroke,
        ));
    };
    let circle = |x, y, r| {
        painter.circle_stroke(p(x, y), r * rect.width() / 24.0, stroke);
    };
    let box_ = |x, y, w, h| {
        painter.rect_stroke(
            egui::Rect::from_min_max(p(x, y), p(x + w, y + h)),
            2.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    use Icon::*;
    let asset = match icon {
        Run => Some(include_str!("../../assets/icons/run.svg")),
        CloneRecord => Some(include_str!("../../assets/icons/clone-record.svg")),
        BeginTransaction => Some(include_str!("../../assets/icons/begin-transaction.svg")),
        CommitTransaction => Some(include_str!("../../assets/icons/commit-transaction.svg")),
        Bytecode => Some(include_str!("../../assets/icons/bytecode.svg")),
        ReviewEdits => Some(include_str!("../../assets/icons/review-edits.svg")),
        CopyValue => Some(include_str!("../../assets/icons/copy-value.svg")),
        FitColumns => Some(include_str!("../../assets/icons/fit-columns.svg")),
        _ => None,
    };
    if let Some(svg) = asset {
        // These bundled vector images deliberately use only polylines. Render
        // the asset geometry directly so currentColor follows the desktop theme
        // at any scale, without rasterisation or a second set of icon drawings.
        for part in svg.split("points=\"").skip(1) {
            let points: Vec<_> = part
                .split('"')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|pair| {
                    let (x, y) = pair.split_once(',')?;
                    Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))
                })
                .collect();
            line(&points);
        }
        return;
    }
    match icon {
        Back => {
            line(&[(19., 12.), (5., 12.)]);
            line(&[(11., 6.), (5., 12.), (11., 18.)]);
        }
        Next => {
            line(&[(5., 12.), (19., 12.)]);
            line(&[(13., 6.), (19., 12.), (13., 18.)]);
        }
        Up => line(&[(6., 15.), (12., 9.), (18., 15.)]),
        Minimize => line(&[(5., 12.), (19., 12.)]),
        Down => line(&[(6., 9.), (12., 15.), (18., 9.)]),
        Search => {
            circle(10., 10., 7.);
            line(&[(15., 15.), (21., 21.)]);
        }
        Database => {
            box_(4., 4., 16., 17.);
            line(&[(4., 9.), (20., 9.)]);
            line(&[(4., 15.), (20., 15.)]);
        }
        Folder => line(&[
            (3., 20.),
            (3., 5.),
            (10., 5.),
            (13., 8.),
            (21., 8.),
            (21., 20.),
            (3., 20.),
        ]),
        Map => {
            box_(2., 3., 8., 6.);
            box_(15., 3., 7., 6.);
            box_(8., 16., 8., 6.);
            line(&[(6., 9.), (6., 12.), (19., 12.), (19., 9.)]);
            line(&[(12., 12.), (12., 16.)]);
        }
        Data => {
            box_(3., 3., 18., 18.);
            line(&[(3., 9.), (21., 9.)]);
            line(&[(3., 15.), (21., 15.)]);
            line(&[(9., 3.), (9., 21.)]);
        }
        Structure => {
            for y in [6., 12., 18.] {
                circle(4., y, 1.);
                line(&[(9., y), (21., y)]);
            }
        }
        Filter => line(&[
            (3., 4.),
            (21., 4.),
            (14., 12.),
            (14., 19.),
            (10., 21.),
            (10., 12.),
            (3., 4.),
        ]),
        Refresh => {
            line(&[(21., 4.), (21., 10.), (15., 10.)]);
            line(&[
                (21., 10.),
                (17., 5.),
                (9., 4.),
                (4., 9.),
                (4., 16.),
                (10., 21.),
                (17., 20.),
                (21., 16.),
            ]);
        }
        Count => line(&[
            (19., 7.),
            (19., 4.),
            (5., 4.),
            (12., 12.),
            (5., 20.),
            (19., 20.),
            (19., 17.),
        ]),
        Export => {
            line(&[(12., 3.), (12., 16.)]);
            line(&[(6., 10.), (12., 16.), (18., 10.)]);
            line(&[(3., 16.), (3., 21.), (21., 21.), (21., 16.)]);
        }
        Copy => {
            box_(9., 9., 12., 12.);
            line(&[(5., 16.), (3., 16.), (3., 3.), (16., 3.), (16., 5.)]);
        }
        Sql => {
            box_(2., 4., 20., 16.);
            line(&[(6., 9.), (10., 12.), (6., 15.)]);
            line(&[(13., 16.), (18., 16.)]);
        }
        History => {
            circle(12., 12., 9.);
            line(&[(12., 6.), (12., 12.), (17., 15.)]);
        }
        Saved => line(&[
            (6., 21.),
            (6., 3.),
            (18., 3.),
            (18., 21.),
            (12., 16.),
            (6., 21.),
        ]),
        Checks => line(&[
            (2., 12.),
            (6., 12.),
            (9., 4.),
            (15., 20.),
            (18., 12.),
            (22., 12.),
        ]),
        Settings => {
            for (x, y) in [(4., 14.), (12., 8.), (20., 16.)] {
                line(&[(x, 3.), (x, 21.)]);
                circle(x, y, 2.5);
            }
        }
        Desktop => {
            box_(3., 4., 18., 13.);
            line(&[(12., 17.), (12., 21.)]);
            line(&[(8., 21.), (16., 21.)]);
        }
        Sun => {
            circle(12., 12., 4.);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::FRAC_PI_4;
                line(&[
                    (12. + 7. * angle.cos(), 12. + 7. * angle.sin()),
                    (12. + 10. * angle.cos(), 12. + 10. * angle.sin()),
                ]);
            }
        }
        Moon => {
            for points in [
                [p(17., 3.), p(-1., 3.), p(-1., 21.), p(17., 21.)],
                [p(17., 21.), p(5., 17.), p(5., 7.), p(17., 3.)],
            ] {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    points,
                    false,
                    egui::Color32::TRANSPARENT,
                    stroke,
                ));
            }
        }
        Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(6., 18.), (18., 6.)]);
        }
        Disconnect => {
            let arc: Vec<_> = (0..=24)
                .map(|step| {
                    let angle = (-50.0 + step as f32 * 280.0 / 24.0).to_radians();
                    (12.0 + 9.0 * angle.cos(), 12.0 + 9.0 * angle.sin())
                })
                .collect();
            line(&arc);
            line(&[(12., 2.), (12., 12.)]);
        }
        Popout => {
            line(&[(14., 3.), (21., 3.), (21., 10.)]);
            line(&[(10., 14.), (21., 3.)]);
            line(&[(10., 4.), (3., 4.), (3., 21.), (20., 21.), (20., 14.)]);
        }
        Lock => {
            box_(4., 11., 16., 11.);
            line(&[
                (8., 11.),
                (8., 5.),
                (10., 3.),
                (14., 3.),
                (16., 5.),
                (16., 11.),
            ]);
        }
        Edit => line(&[
            (4., 20.),
            (6., 14.),
            (17., 3.),
            (21., 7.),
            (10., 18.),
            (4., 20.),
        ]),
        Fit => {
            for (x, y, dx, dy) in [
                (3., 3., 5., 5.),
                (21., 3., -5., 5.),
                (3., 21., 5., -5.),
                (21., 21., -5., -5.),
            ] {
                line(&[(x, y + dy), (x, y), (x + dx, y)]);
            }
        }
        Arrange => {
            for (x, y) in [(3., 3.), (14., 3.), (3., 14.), (14., 14.)] {
                box_(x, y, 7., 7.);
            }
        }
        Columns => {
            box_(3., 3., 18., 18.);
            line(&[(12., 3.), (12., 21.)]);
        }
        System => line(&[
            (12., 2.),
            (21., 6.),
            (20., 15.),
            (16., 20.),
            (12., 23.),
            (8., 20.),
            (4., 15.),
            (3., 6.),
            (12., 2.),
        ]),
        Plus => {
            line(&[(4., 12.), (20., 12.)]);
            line(&[(12., 4.), (12., 20.)]);
        }
        Selection => {
            for y in [6., 12., 18.] {
                line(&[(2., y), (8., y)]);
            }
            line(&[(13., 6.), (21., 12.), (13., 18.), (13., 6.)]);
        }
        Explain => {
            box_(2., 3., 7., 7.);
            box_(15., 15., 7., 7.);
            line(&[(6., 10.), (6., 18.), (15., 18.)]);
        }
        Parameters => {
            line(&[
                (8., 3.),
                (5., 3.),
                (5., 10.),
                (2., 12.),
                (5., 14.),
                (5., 21.),
                (8., 21.),
            ]);
            line(&[
                (16., 3.),
                (19., 3.),
                (19., 10.),
                (22., 12.),
                (19., 14.),
                (19., 21.),
                (16., 21.),
            ]);
        }
        Complete => {
            line(&[(3., 6.), (21., 6.)]);
            line(&[(3., 12.), (16., 12.)]);
            line(&[(3., 18.), (12., 18.)]);
        }
        Undo => {
            line(&[(9., 4.), (4., 9.), (9., 14.)]);
            line(&[
                (4., 9.),
                (17., 9.),
                (21., 13.),
                (21., 17.),
                (17., 21.),
                (12., 21.),
            ]);
        }
        Trash => {
            line(&[(3., 6.), (21., 6.)]);
            line(&[(8., 6.), (8., 3.), (16., 3.), (16., 6.)]);
            line(&[(5., 6.), (6., 21.), (18., 21.), (19., 6.)]);
        }
        Key => {
            circle(7., 16., 5.);
            line(&[(11., 12.), (21., 2.)]);
            line(&[(16., 7.), (20., 11.)]);
        }
        Run | CloneRecord | BeginTransaction | CommitTransaction | Bytecode | ReviewEdits
        | CopyValue | FitColumns => unreachable!("asset rendered above"),
        Link => {
            line(&[
                (10., 15.),
                (6., 19.),
                (2., 15.),
                (2., 12.),
                (8., 6.),
                (12., 6.),
                (16., 10.),
            ]);
            line(&[
                (8., 14.),
                (14., 8.),
                (18., 5.),
                (22., 9.),
                (22., 12.),
                (16., 18.),
                (12., 18.),
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_ribbon_buttons_keep_their_hit_target_and_accessibility() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let normal = button(ui, Icon::Columns, "Normal columns", "");
            let large = ribbon_button(ui, Icon::Columns, "Ribbon columns", "", Some(true));
            assert_eq!(large.rect.size(), egui::Vec2::splat(RIBBON_BUTTON_SIZE));
            assert!(large.rect.width() >= normal.rect.width());
        });
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.unwrap();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Ribbon columns")
                    && n.toggled() == Some(egui::accesskit::Toggled::True))
        );
        assert!(output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Rect(rect) => (rect.stroke.width - 1.575).abs() < 0.01,
            _ => false,
        }));
    }

    #[test]
    fn painter_buttons_expose_names_and_selected_state_to_accesskit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            control(ui, Icon::Structure, "Structure", "Ctrl 2", true, false);
            button(ui, Icon::Export, "Export retained rows", "Ctrl E");
        });
        output.textures_delta.clear();
        let update = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility tree");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Structure")
                    && n.toggled() == Some(egui::accesskit::Toggled::True))
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Export retained rows") && n.toggled().is_none())
        );
        output.textures_delta.clear();
    }
}
