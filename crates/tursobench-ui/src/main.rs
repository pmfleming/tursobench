mod app;
mod export;
mod storage;
fn main() -> eframe::Result {
    eframe::run_native(
        "TursoBench — Turso 0.8.3 preview",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx)))),
    )
}
