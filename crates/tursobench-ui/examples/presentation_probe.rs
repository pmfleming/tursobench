//! Opt-in desktop probe: continuous redraw demand, no database or preferences.
//! Move this window between visible and hidden workspaces to check frame pacing.
//! Reports once a second and closes after 30 seconds (use an external timeout too).
use eframe::egui;
#[path = "../src/desktop.rs"]
mod desktop;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

struct Probe(Arc<AtomicU64>);

impl eframe::App for Probe {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let frame = self.0.fetch_add(1, Ordering::Relaxed);
        ui.heading("TursoBench presentation probe");
        ui.label("Move this window to another workspace, then bring it back.");
        ui.label("No database or preferences are read. Closes after 30 seconds.");
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                ui.min_rect().min + egui::vec2((frame % 200) as f32, 100.0),
                egui::vec2(40.0, 40.0),
            ),
            4.0,
            egui::Color32::LIGHT_BLUE,
        );
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "TursoBench presentation probe",
        eframe::NativeOptions {
            event_loop_builder: Some(Box::new(desktop::configure_backend)),
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_app_id("tursobench-presentation-probe")
                .with_inner_size([640.0, 240.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            desktop::verify_backend(cc)?;
            let frames = Arc::new(AtomicU64::new(0));
            let observed = frames.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let mut previous = 0;
                for second in 1..=30 {
                    std::thread::sleep(Duration::from_secs(1));
                    let count = observed.load(Ordering::Relaxed);
                    println!(
                        "{}",
                        serde_json::json!({"second": second, "frames": count,
                            "frames_since_last_report": count - previous})
                    );
                    previous = count;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            });
            Ok(Box::new(Probe(frames)))
        }),
    )
}
