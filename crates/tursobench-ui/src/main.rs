mod app;
mod appearance;
mod assistant_tools;
mod desktop;
mod discovery;
mod export;
#[cfg(test)]
#[path = "../../tursobench-db/tests/common/local_fixtures.rs"]
mod local_project_fixtures;
mod recovery;
mod resources;
mod storage;
#[cfg(test)]
#[path = "../../tursobench-db/tests/common/fixture.rs"]
mod test_support;
fn main() -> eframe::Result {
    // An agent CLI starts this mode as its MCP server; it never opens a window.
    let args: Vec<_> = std::env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|a| a == tursobench_assistant::relay::RELAY_FLAG)
    {
        let Some(socket) = args.get(2) else {
            eprintln!("usage: tursobench --mcp-relay <socket>");
            std::process::exit(2);
        };
        if let Err(error) = tursobench_assistant::relay::run_relay(std::path::Path::new(socket)) {
            eprintln!("tursobench relay: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../packaging/tursobench.png"))
            .expect("the bundled TursoBench icon must be a valid PNG");
    eframe::run_native(
        "TursoBench — Turso 0.8.3 preview",
        // eframe >= 0.37 uses nonblocking Glow swaps and compositor frame callbacks
        // on Wayland, so hidden workspaces keep dispatching desktop ping replies.
        // Retain upstream pacing; disabling VSync alone can cause uncapped rendering.
        eframe::NativeOptions {
            event_loop_builder: Some(Box::new(desktop::configure_backend)),
            viewport: eframe::egui::ViewportBuilder::default()
                .with_app_id("tursobench")
                .with_icon(icon)
                .with_inner_size([1280.0, 720.0])
                .with_min_inner_size([720.0, 480.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            desktop::verify_backend(cc)?;
            Ok(Box::new(app::App::new(&cc.egui_ctx)))
        }),
    )
}
