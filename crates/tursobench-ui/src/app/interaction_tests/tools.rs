use super::{
    App, View, browse_and_wait, browsing, click, click_control, click_panel, draw_panel, egui,
    execute_and_wait, fixture, frame, texts, wait, wait_until,
};
use tursobench_core::{EventKind, Failure, Value};

/// The control that the label `name` describes.
fn field(output: &egui::FullOutput, name: &str) -> egui::Pos2 {
    let nodes = crate::app::test_ui::nodes(output);
    let named = |id| {
        nodes
            .iter()
            .any(|(i, n)| i == id && n.value() == Some(name))
    };
    let bounds = nodes
        .iter()
        .find_map(|(_, n)| n.labelled_by().iter().any(named).then(|| n.bounds())?)
        .unwrap_or_else(|| panic!("Missing field: {name}"));
    egui::pos2(
        bounds.x0 as f32 + 8.0,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}
/// Whether `output` painted `text`.
fn shows(output: &egui::FullOutput, text: &str) -> bool {
    texts(output).any(|t| t.galley.job.text == text)
}
fn click_and_settle(app: &mut App, ctx: &egui::Context, label: &str) {
    click_control(app, ctx, label);
    assert!(app.database.tools.transfers.ticket.is_some(), "{label}");
    wait_until(app, ctx, |a| a.database.tools.transfers.ticket.is_none());
}

#[test]
fn transfer_window_exports_previews_maps_imports_and_checkpoints() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE source(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO source VALUES (1, 'a'), (2, 'b')",
            "CREATE TABLE copy(ID INTEGER PRIMARY KEY, label TEXT)",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "source");
    app.open_transfers();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    // The export target is typed into the window; nothing runs until it is named.
    let export = dir.path().join("source.jsonl");
    click(&mut app, &ctx, field(&output, "New output file"));
    let path = export.to_str().unwrap().to_owned();
    frame(&mut app, &ctx, vec![egui::Event::Text(path.clone())]);
    click_and_settle(&mut app, &ctx, "Export complete table to JSONL");
    assert!(export.exists(), "{}", app.database.status);

    browse_and_wait(&mut app, &ctx, "copy");
    app.open_transfers();
    app.database.tools.transfers.input = path;
    click_and_settle(&mut app, &ctx, "Preview first 10 rows");
    assert!(shows(
        &frame(&mut app, &ctx, vec![]),
        "Input scope: complete-table"
    ));
    // The window keeps its size, so the preview below it scrolls into view.
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -2000.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(egui::pos2(300.0, 400.0)), wheel],
    );
    let output = (0..10).fold(frame(&mut app, &ctx, vec![]), |_, _| {
        frame(&mut app, &ctx, vec![])
    });
    // `id` maps onto `ID` ignoring case; `name` has no column and is skipped.
    for label in ["1 | a", "2 | b", "ID", "Skip"] {
        assert!(shows(&output, label), "{label}");
    }
    click_and_settle(&mut app, &ctx, "Import mapped rows");
    execute_and_wait(&mut app, &ctx, "SELECT ID, label FROM copy ORDER BY ID");
    assert_eq!(
        app.database.document.result.rows,
        [1, 2].map(|id| vec![Value::Integer(id), Value::Null])
    );

    click_and_settle(&mut app, &ctx, "Checkpoint WAL");
    app.transfer_event(EventKind::Failed(Failure::input("disk full")));
    assert!(shows(&frame(&mut app, &ctx, vec![]), "Input: disk full"));
    app.close();
}

#[test]
fn facets_and_chips_add_replace_and_remove_filters_of_the_browsed_rows() {
    let (_dir, ctx, mut app) = browsing(
        "t",
        "id INTEGER PRIMARY KEY, name TEXT",
        "(1, 'a'), (2, 'b'), (3, 'a')",
        false,
    );
    let rows = |app: &App| app.database.document.result.rows.len();
    click_panel(&ctx, "a  · 2", |ui| app.column_facets(ui, 1));
    wait(&mut app, &ctx);
    assert_eq!(rows(&app), 2);
    // A chip opens its editor; Apply replaces the filter and closes it.
    click_panel(&ctx, "name = a", |ui| app.filter_controls(ui));
    assert!(app.database.document.tools.filter_editor.is_some());
    click_panel(&ctx, "Apply", |ui| app.filter_controls(ui));
    wait(&mut app, &ctx);
    assert!(app.database.document.tools.filter_editor.is_none());
    assert_eq!(rows(&app), 2);
    click_panel(&ctx, "×", |ui| app.filter_controls(ui));
    wait(&mut app, &ctx);
    assert_eq!(rows(&app), 3);
    app.database.document.tools.advanced_filter = "name = 'b' OR id IN [3]".into();
    for (label, expected) in [("Apply expression", 2), ("Clear expression", 3)] {
        click_panel(&ctx, "AND / OR / IN…", |ui| app.filter_controls(ui));
        click_panel(&ctx, label, |ui| app.filter_controls(ui));
        wait(&mut app, &ctx);
        assert_eq!(rows(&app), expected, "{label}");
    }
    click_panel(&ctx, "Custom filter…", |ui| app.column_facets(ui, 1));
    assert_eq!(app.database.document.filter, "\"name\" = ");
    assert!(app.database.document.tools.filter_focus);
    click_panel(&ctx, "Hide column", |ui| app.column_facets(ui, 1));
    assert!(app.database.document.tools.columns.hidden.contains(&1));
    app.database.document.view = View::Query;
    let output = draw_panel(&ctx, |ui| app.column_facets(ui, 1));
    assert!(shows(&output, "Facets require a table browse"));
    app.close();
}

#[test]
fn profiling_and_administration_windows_submit_their_requests() {
    let (_dir, ctx, mut app) = browsing("t", "id INTEGER PRIMARY KEY", "(1), (2)", true);
    app.database.tools.profile.open = true;
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, field(&output, "Table"));
    click_control(&mut app, &ctx, "t");
    click_control(&mut app, &ctx, "Generate profile");
    assert!(!app.database.tools.profile.open);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.view, View::Diagnostics);
    click_control(&mut app, &ctx, "Refresh profile");
    wait(&mut app, &ctx);
    assert!(shows(&frame(&mut app, &ctx, vec![]), "Profile: t"));
    app.database.tools.admin.open = true;
    frame(&mut app, &ctx, vec![]);
    for label in ["Apply timeout", "Refresh dashboard"] {
        click_control(&mut app, &ctx, label);
        assert!(app.database.document.active.is_some(), "{label}");
        wait(&mut app, &ctx);
        let completion = app.database.document.result.completion.as_ref().unwrap();
        assert!(
            completion.error.is_none(),
            "{label}: {:?}",
            completion.error
        );
    }
    app.close();
}
