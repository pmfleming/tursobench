//! Shared visual primitives; callers retain action routing and enabled-state rules.
use eframe::egui;

pub(super) fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(crate::appearance::text_size(ui.ctx(), 12.0))
                .color(color),
        )
        .fill(color.gamma_multiply(0.13))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.48)))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, 24.0)),
    )
}
