//! The assistant panel sits beside the sidebar; each can be hidden on its own.
use super::{
    App,
    icons::{self, Icon},
};
use eframe::egui;

impl App {
    pub(super) fn assistant_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.strong("Assistant");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::control(ui, Icon::Close, "Hide assistant", "Ctrl I", false, false)
                    .clicked()
                {
                    self.toggle_assistant();
                }
            });
        });
        ui.separator();
        icons::empty_state(
            ui,
            Icon::Assistant,
            "Chat with the agent CLI you are already signed in to",
        );
    }

    pub(super) fn assistant_focus(&mut self) {}
}
