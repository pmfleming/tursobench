//! Asset-free icons share native button interaction, focus, tooltip and AccessKit names.
use eframe::egui::{self, Pos2, Stroke};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Next,
    Trigger,
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
    /// Copy with lines of text: the whole conversation.
    CopyAll,
    Sql,
    History,
    Saved,
    Checks,
    Settings,
    Desktop,
    Sun,
    Moon,
    Close,
    Lock,
    Edit,
    Fit,
    Arrange,
    RestoreTiles,
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
    RunAll,
    CloneRecord,
    BeginTransaction,
    CommitTransaction,
    Bytecode,
    ReviewEdits,
    CopyValue,
    FitColumns,
    Help,
    Command,
    EditCells,
    Compare,
    Gauge,
    Profile,
    Transfer,
    Sidebar,
    /// The sidebar mirrored: the Record, Structure and SQL column.
    Assistant,
    Send,
    Stop,
    /// A tick: the chosen item in a list.
    Check,
    /// A five-point star: favourites.
    Star,
}

pub(super) fn button(ui: &mut egui::Ui, icon: Icon, name: &str, shortcut: &str) -> egui::Response {
    widget(ui, icon, name, shortcut, None, false, 1.0)
}

/// The same access-mode symbols, names and colours are used throughout the app.
pub(super) struct AccessMode {
    pub icon: Icon,
    pub name: &'static str,
    pub color: egui::Color32,
}

pub(super) fn access_mode(ctx: &egui::Context, writable: bool) -> AccessMode {
    let tokens = crate::appearance::Tokens::of(ctx);
    if writable {
        AccessMode {
            icon: Icon::Edit,
            name: "Read/write",
            color: tokens.pending,
        }
    } else {
        AccessMode {
            icon: Icon::Lock,
            name: "Read only",
            color: tokens.muted,
        }
    }
}

pub(super) fn access_button(
    ui: &mut egui::Ui,
    writable: bool,
    selected: Option<bool>,
) -> egui::Response {
    let mode = access_mode(ui.ctx(), writable);
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.fg_stroke.color = mode.color;
        }
        widget(ui, mode.icon, mode.name, "", selected, false, 1.0)
    })
    .inner
}

pub(super) fn access_indicator(ui: &mut egui::Ui, rect: egui::Rect, writable: bool, enabled: bool) {
    let mode = access_mode(ui.ctx(), writable);
    paint(
        ui.painter(),
        rect,
        mode.icon,
        if enabled {
            mode.color
        } else {
            mode.color.gamma_multiply(0.5)
        },
    );
    named_image(ui, rect, "database-access", enabled, mode.name);
}

/// A painted glyph that names itself on hover and to assistive technology.
fn named_image(ui: &egui::Ui, rect: egui::Rect, salt: &str, enabled: bool, name: &str) {
    let response = ui.interact(rect, ui.scope_id().with(salt), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, enabled, name));
    response.on_hover_text(name);
}

/// A glyph that explains itself on hover, such as a read-only lock.
pub(super) fn hint(ui: &mut egui::Ui, icon: Icon, size: f32, tooltip: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint(ui.painter(), rect, icon, ui.visuals().weak_text_color());
    response.on_hover_text(tooltip);
}

pub(super) const RIBBON_BUTTON_SIZE: f32 = 30.0;
pub(super) const RIBBON_TEXT_SIZE: f32 = 13.5;

/// Ribbon text and button height for the rest of `ui`.
pub(super) fn ribbon_style(ui: &mut egui::Ui) {
    ui.style_mut().override_font_id =
        Some(crate::appearance::proportional(ui.ctx(), RIBBON_TEXT_SIZE));
    ui.spacing_mut().interact_size.y = RIBBON_BUTTON_SIZE;
}

/// The map's zoom as a percentage; clicking it resets to 100%.
pub(super) fn zoom_button(ui: &mut egui::Ui, zoom: f32, label: &str) -> egui::Response {
    let percent = zoom * 100.0;
    let text = if percent < 1.0 {
        format!("{percent:.2}%")
    } else {
        format!("{percent:.0}%")
    };
    let size = crate::appearance::text_size(ui.ctx(), RIBBON_TEXT_SIZE);
    let response = ui.add(
        egui::Button::new(egui::RichText::new(text).size(size))
            .min_size(egui::Vec2::splat(RIBBON_BUTTON_SIZE)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response.on_hover_text("Current zoom · click to reset to 100%")
}

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
    named_image(ui, glyph, "empty-state", true, description);
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
    // A chosen mode reads at a glance: accent outline and accent icon.
    let chosen = selected == Some(true) && !primary;
    if chosen {
        native = native.stroke(egui::Stroke::new(1.0, tokens.primary.gamma_multiply(0.6)));
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
        } else if chosen {
            tokens.primary
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
pub(super) fn paint(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: egui::Color32) {
    let pen = Pen {
        painter,
        rect,
        stroke: Stroke::new(1.4 * (rect.width() / 16.0).max(1.0), color),
    };
    for mark in marks(icon) {
        pen.draw(mark);
    }
}

/// One stroke of an icon, in a 24-unit square scaled to the icon's rect.
enum Mark {
    /// An open polyline in SVG `points` syntax: `"x,y x,y …"`.
    Line(&'static str),
    Circle(f32, f32, f32),
    /// A rounded outline from its top-left corner, width and height.
    Rect(f32, f32, f32, f32),
    /// Every polyline of a bundled SVG asset.
    Svg(&'static str),
    /// Geometry that is computed rather than listed.
    Drawn(fn(&Pen)),
}

use Mark::{Circle, Drawn, Line, Rect, Svg};

fn marks(icon: Icon) -> &'static [Mark] {
    match icon {
        Icon::Next => &[Line("5,12 19,12"), Line("13,6 19,12 13,18")],
        Icon::Trigger => &[Line("13,2 5,14 12,14 11,22 19,10 12,10 13,2")],
        Icon::Search => &[Circle(10., 10., 7.), Line("15,15 21,21")],
        Icon::Database => &[Rect(4., 4., 16., 17.), Line("4,9 20,9"), Line("4,15 20,15")],
        Icon::Folder => &[Line("3,20 3,5 10,5 13,8 21,8 21,20 3,20")],
        Icon::Map => &[
            Rect(2., 3., 8., 6.),
            Rect(15., 3., 7., 6.),
            Rect(8., 16., 8., 6.),
            Line("6,9 6,12 19,12 19,9"),
            Line("12,12 12,16"),
        ],
        Icon::Data => &[
            Rect(3., 3., 18., 18.),
            Line("3,9 21,9"),
            Line("3,15 21,15"),
            Line("9,3 9,21"),
        ],
        Icon::Structure => &[
            Circle(4., 6., 1.),
            Line("9,6 21,6"),
            Circle(4., 12., 1.),
            Line("9,12 21,12"),
            Circle(4., 18., 1.),
            Line("9,18 21,18"),
        ],
        Icon::Filter => &[Line("3,4 21,4 14,12 14,19 10,21 10,12 3,4")],
        Icon::Refresh => &[
            Line("21,4 21,10 15,10"),
            Line("21,10 17,5 9,4 4,9 4,16 10,21 17,20 21,16"),
        ],
        Icon::Count => &[Line("19,7 19,4 5,4 12,12 5,20 19,20 19,17")],
        Icon::Export => &[
            Line("12,3 12,16"),
            Line("6,10 12,16 18,10"),
            Line("3,16 3,21 21,21 21,16"),
        ],
        Icon::Copy => &[Rect(9., 9., 12., 12.), Line("5,16 3,16 3,3 16,3 16,5")],
        Icon::CopyAll => &[
            Rect(9., 9., 12., 12.),
            Line("5,16 3,16 3,3 16,3 16,5"),
            Line("12,13 18,13"),
            Line("12,17 16,17"),
        ],
        Icon::Sql => &[
            Rect(2., 4., 20., 16.),
            Line("6,9 10,12 6,15"),
            Line("13,16 18,16"),
        ],
        Icon::History => &[Circle(12., 12., 9.), Line("12,6 12,12 17,15")],
        Icon::Saved => &[Line("6,21 6,3 18,3 18,21 12,16 6,21")],
        Icon::Checks => &[Line("2,12 6,12 9,4 15,20 18,12 22,12")],
        Icon::Settings => &[
            Line("4,3 4,21"),
            Circle(4., 14., 2.5),
            Line("12,3 12,21"),
            Circle(12., 8., 2.5),
            Line("20,3 20,21"),
            Circle(20., 16., 2.5),
        ],
        Icon::Desktop => &[
            Rect(3., 4., 18., 13.),
            Line("12,17 12,21"),
            Line("8,21 16,21"),
        ],
        Icon::Sun => &[Circle(12., 12., 4.), Drawn(sun_rays)],
        Icon::Moon => &[Drawn(moon)],
        Icon::Close => &[Line("6,6 18,18"), Line("6,18 18,6")],
        Icon::Lock => &[
            Rect(4., 11., 16., 11.),
            Line("8,11 8,5 10,3 14,3 16,5 16,11"),
        ],
        Icon::Edit => &[Line("4,20 5,15 17,3 21,7 9,19 4,20"), Line("15,5 19,9")],
        // Four corner brackets.
        Icon::Fit => &[
            Line("3,8 3,3 8,3"),
            Line("21,8 21,3 16,3"),
            Line("3,16 3,21 8,21"),
            Line("21,16 21,21 16,21"),
        ],
        Icon::Arrange => &[
            Rect(3., 3., 7., 7.),
            Rect(14., 3., 7., 7.),
            Rect(3., 14., 7., 7.),
            Rect(14., 14., 7., 7.),
        ],
        Icon::RestoreTiles => &[
            Rect(3., 3., 18., 18.),
            Line("12,3 12,21"),
            Line("3,10 12,10"),
            Line("12,9 21,9"),
            Line("12,15 21,15"),
        ],
        Icon::Columns => &[Rect(3., 3., 18., 18.), Line("12,3 12,21")],
        Icon::System => &[Line("12,2 21,6 20,15 16,20 12,23 8,20 4,15 3,6 12,2")],
        Icon::Plus => &[Line("4,12 20,12"), Line("12,4 12,20")],
        Icon::Selection => &[
            Line("2,6 8,6"),
            Line("2,12 8,12"),
            Line("2,18 8,18"),
            Line("13,6 21,12 13,18 13,6"),
        ],
        Icon::Explain => &[
            Rect(2., 3., 7., 7.),
            Rect(15., 15., 7., 7.),
            Line("6,10 6,18 15,18"),
        ],
        Icon::Parameters => &[
            Line("8,3 5,3 5,10 2,12 5,14 5,21 8,21"),
            Line("16,3 19,3 19,10 22,12 19,14 19,21 16,21"),
        ],
        Icon::Complete => &[Line("3,6 21,6"), Line("3,12 16,12"), Line("3,18 12,18")],
        Icon::Undo => &[
            Line("9,4 4,9 9,14"),
            Line("4,9 17,9 21,13 21,17 17,21 12,21"),
        ],
        Icon::Trash => &[
            Line("3,6 21,6"),
            Line("8,6 8,3 16,3 16,6"),
            Line("5,6 6,21 18,21 19,6"),
        ],
        Icon::Key => &[Circle(7., 16., 5.), Line("11,12 21,2"), Line("16,7 20,11")],
        Icon::Link => &[
            Line("10,15 6,19 2,15 2,12 8,6 12,6 16,10"),
            Line("8,14 14,8 18,5 22,9 22,12 16,18 12,18"),
        ],
        Icon::Up => &[Line("6,15 12,9 18,15")],
        Icon::Down => &[Line("6,9 12,15 18,9")],
        Icon::Minimize => &[Line("5,12 19,12")],
        Icon::Run => &[Svg(include_str!("../../assets/icons/run.svg"))],
        Icon::RunAll => &[Svg(include_str!("../../assets/icons/run-all.svg"))],
        Icon::CloneRecord => &[Svg(include_str!("../../assets/icons/clone-record.svg"))],
        Icon::BeginTransaction => &[Svg(include_str!(
            "../../assets/icons/begin-transaction.svg"
        ))],
        Icon::CommitTransaction => &[Svg(include_str!(
            "../../assets/icons/commit-transaction.svg"
        ))],
        Icon::Bytecode => &[Svg(include_str!("../../assets/icons/bytecode.svg"))],
        Icon::ReviewEdits => &[Svg(include_str!("../../assets/icons/review-edits.svg"))],
        Icon::CopyValue => &[Svg(include_str!("../../assets/icons/copy-value.svg"))],
        Icon::FitColumns => &[Svg(include_str!("../../assets/icons/fit-columns.svg"))],
        Icon::Help => &[
            Circle(12., 12., 9.),
            Line("9,9.5 9.6,7.8 11,7 13,7 14.4,7.8 15,9.5 14.2,11.2 12,12.6 12,14"),
            Line("12,16.6 12,17.2"),
        ],
        // The looped-square command sign: four corner loops joined by two bars each way.
        Icon::Command => &[
            Circle(6., 6., 3.),
            Circle(18., 6., 3.),
            Circle(6., 18., 3.),
            Circle(18., 18., 3.),
            Line("9,6 9,18"),
            Line("15,6 15,18"),
            Line("6,9 18,9"),
            Line("6,15 18,15"),
        ],
        // A cell holding a text cursor.
        Icon::EditCells => &[
            Rect(3., 6., 18., 12.),
            Line("9,9.5 9,14.5"),
            Line("7.5,9.5 10.5,9.5"),
            Line("7.5,14.5 10.5,14.5"),
        ],
        Icon::Compare => &[
            Line("4,4 4,20"),
            Line("20,4 20,20"),
            Line("8,9 16,9"),
            Line("14,7 16,9 14,11"),
            Line("16,15 8,15"),
            Line("10,13 8,15 10,17"),
        ],
        Icon::Gauge => &[
            Line("3,17 4.2,12.5 7.5,9.2 12,8 16.5,9.2 19.8,12.5 21,17"),
            Line("12,17 16,12"),
            Line("3,20 21,20"),
        ],
        Icon::Profile => &[
            Line("3,20 21,20"),
            Line("6,20 6,13"),
            Line("12,20 12,4"),
            Line("18,20 18,9"),
        ],
        Icon::Transfer => &[
            Line("8,20 8,4"),
            Line("4,8 8,4 12,8"),
            Line("16,4 16,20"),
            Line("12,16 16,20 20,16"),
        ],
        Icon::Sidebar => &[
            Rect(3., 4., 18., 16.),
            Line("9,4 9,20"),
            Line("5,8 7,8"),
            Line("5,11 7,11"),
        ],
        // A speech bubble holding a spark: a conversation with an agent.
        Icon::Assistant => &[
            Line("4,5 20,5 20,16 11,16 6,20 7,16 4,16 4,5"),
            Line("12,7.5 12,13.5"),
            Line("9,10.5 15,10.5"),
        ],
        Icon::Send => &[Line("3,12 21,4 14,21 11,13 3,12"), Line("11,13 21,4")],
        Icon::Stop => &[Rect(6., 6., 12., 12.)],
        Icon::Check => &[Line("4,12.5 9.5,18 20,6.5")],
        Icon::Star => &[Drawn(|pen| pen.line(star_points(12., 12.6, 9.5)))],
    }
}

/// Strokes in 24-unit icon space, scaled to one icon's rect.
struct Pen<'a> {
    painter: &'a egui::Painter,
    rect: egui::Rect,
    stroke: Stroke,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        icon_point(self.rect, x, y)
    }

    fn line(&self, points: impl IntoIterator<Item = (f32, f32)>) {
        let points = points.into_iter().map(|(x, y)| self.at(x, y)).collect();
        self.painter.add(egui::Shape::line(points, self.stroke));
    }

    fn draw(&self, mark: &Mark) {
        match *mark {
            Line(points) => self.line(polyline(points)),
            Circle(x, y, r) => {
                self.painter.circle_stroke(
                    self.at(x, y),
                    r * self.rect.width() / 24.0,
                    self.stroke,
                );
            }
            Rect(x, y, w, h) => {
                self.painter.rect_stroke(
                    egui::Rect::from_min_max(self.at(x, y), self.at(x + w, y + h)),
                    2.0,
                    self.stroke,
                    egui::StrokeKind::Inside,
                );
            }
            // These bundled vector images deliberately use only polylines.
            // Render the asset geometry directly so currentColor follows the
            // desktop theme at any scale, without rasterisation or a second
            // set of icon drawings.
            Svg(svg) => {
                for part in svg.split("points=\"").skip(1) {
                    self.line(polyline(part.split('"').next().unwrap_or_default()));
                }
            }
            Drawn(draw) => draw(self),
        }
    }
}

fn icon_point(rect: egui::Rect, x: f32, y: f32) -> Pos2 {
    rect.min + egui::vec2(x * rect.width() / 24.0, y * rect.height() / 24.0)
}

/// Points in SVG `points` syntax; malformed pairs are skipped.
fn polyline(points: &str) -> impl Iterator<Item = (f32, f32)> + '_ {
    points.split_whitespace().filter_map(|pair| {
        let (x, y) = pair.split_once(',')?;
        Some((x.parse().ok()?, y.parse().ok()?))
    })
}

fn sun_rays(pen: &Pen) {
    for index in 0..8 {
        let angle = index as f32 * std::f32::consts::FRAC_PI_4;
        pen.line([
            (12. + 7. * angle.cos(), 12. + 7. * angle.sin()),
            (12. + 10. * angle.cos(), 12. + 10. * angle.sin()),
        ]);
    }
}

fn moon(pen: &Pen) {
    for points in [
        [(17., 3.), (-1., 3.), (-1., 21.), (17., 21.)],
        [(17., 21.), (5., 17.), (5., 7.), (17., 3.)],
    ] {
        pen.painter
            .add(egui::epaint::CubicBezierShape::from_points_stroke(
                points.map(|(x, y)| pen.at(x, y)),
                false,
                egui::Color32::TRANSPARENT,
                pen.stroke,
            ));
    }
}

/// A closed five-point star in 24-unit icon space, starting at the top point.
fn star_points(cx: f32, cy: f32, outer: f32) -> Vec<(f32, f32)> {
    (0..=10)
        .map(|i| {
            let radius = if i % 2 == 0 { outer } else { outer * 0.42 };
            let angle = std::f32::consts::PI * (i as f32 / 5.0 - 0.5);
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// A filled star: a chosen favourite.
pub(super) fn paint_filled_star(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let p = |(x, y)| icon_point(rect, x, y);
    let points = star_points(12., 12.6, 9.5);
    let centre = p((12., 12.6));
    // The star is not convex: fill it as triangles around its centre.
    for pair in points.windows(2) {
        painter.add(egui::Shape::convex_polygon(
            vec![centre, p(pair[0]), p(pair[1])],
            color,
            Stroke::NONE,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{Icon, RIBBON_BUTTON_SIZE, button, control, egui, ribbon_button};
    #[test]
    fn painter_buttons_expose_names_and_selected_state_to_accesskit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let normal = button(ui, Icon::Columns, "Normal columns", "");
            let large = ribbon_button(ui, Icon::Columns, "Ribbon columns", "", Some(true));
            assert_eq!(large.rect.size(), egui::Vec2::splat(RIBBON_BUTTON_SIZE));
            assert!(large.rect.width() >= normal.rect.width());
            control(ui, Icon::Structure, "Structure", "Ctrl 2", true, false);
            button(ui, Icon::Export, "Export retained rows", "Ctrl E");
        });
        output.textures_delta.clear();
        let update = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility tree");
        assert!(output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Rect(rect) => (rect.stroke.width - 1.575).abs() < 0.01,
            _ => false,
        }));
        let on = Some(egui::accesskit::Toggled::True);
        for (name, toggled) in [
            ("Ribbon columns", on),
            ("Structure", on),
            ("Export retained rows", None),
        ] {
            let node = update.nodes.iter().find(|(_, n)| n.label() == Some(name));
            assert_eq!(node.map(|(_, n)| n.toggled()), Some(toggled), "{name}");
        }
    }
}
