mod app;
mod appearance;
mod discovery;
mod export;
#[cfg(test)]
#[path = "../../tursobench-db/tests/common/local_fixtures.rs"]
mod local_project_fixtures;
mod storage;
#[cfg(test)]
#[path = "../../tursobench-db/tests/common/fixture.rs"]
mod test_support;
fn main() -> eframe::Result {
    eframe::run_native(
        "TursoBench — Turso 0.8.3 preview",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 720.0])
                .with_min_inner_size([720.0, 480.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx)))),
    )
}
