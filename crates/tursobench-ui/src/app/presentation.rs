//! Shared visual primitives; callers retain action routing and enabled-state rules.
use eframe::egui;

pub(super) fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let text_color = if color == tokens.primary {
        tokens.primary_text
    } else {
        color
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(crate::appearance::text_size(ui.ctx(), 12.0))
                .color(text_color),
        )
        .fill(color.gamma_multiply(0.13))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.48)))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

#[derive(Clone, Copy)]
pub(super) enum Intent {
    Neutral,
    Primary,
    Pending,
    Danger,
}

pub(super) fn intent_color(ui: &egui::Ui, intent: Intent) -> egui::Color32 {
    let t = crate::appearance::Tokens::of(ui.ctx());
    match intent {
        Intent::Neutral => t.muted,
        Intent::Primary => t.primary,
        Intent::Pending => t.pending,
        Intent::Danger => ui.visuals().error_fg_color,
    }
}

pub(super) fn action(ui: &egui::Ui, label: &str, intent: Intent) -> egui::Button<'static> {
    let color = intent_color(ui, intent);
    let solid = matches!(intent, Intent::Primary);
    egui::Button::new(egui::RichText::new(label.to_owned()).color(if solid {
        crate::appearance::Tokens::of(ui.ctx()).primary_ink
    } else {
        color
    }))
    .fill(if solid {
        color
    } else {
        color.gamma_multiply(0.13)
    })
    .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.48)))
    .min_size(egui::vec2(0.0, 30.0))
}

pub(super) fn window_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    egui::Frame::window(&style)
        .fill(style.visuals.window_fill)
        .stroke(egui::Stroke::new(
            1.0,
            crate::appearance::Tokens::of(ctx).line_strong,
        ))
        .corner_radius(12)
        .inner_margin(16.0)
}

pub(super) fn notice(ui: &mut egui::Ui, icon: super::icons::Icon, intent: Intent, text: &str) {
    let color = intent_color(ui, intent);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.08))
        .inner_margin(10.0)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                super::icons::paint(ui.painter(), rect, icon, color);
                ui.add(egui::Label::new(text).wrap());
            });
        });
}

pub(super) fn footer<R>(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_space(8.0);
    egui::Frame::new()
        .fill(crate::appearance::Tokens::of(ui.ctx()).raised)
        .inner_margin(10.0)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 30.0),
                egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                contents,
            )
            .inner
        })
}
