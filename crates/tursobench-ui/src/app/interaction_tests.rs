use super::{
    App, View,
    columns::CellStyle,
    context, egui, shell,
    test_ui::{control, control_from, inspect_selected, key_event, pointer_button, texts},
    tiling,
};
use std::time::{Duration, Instant};
use tursobench_core::{Budget, Event, EventKind, FilterOp, Ticket, TransactionState, Value};
use tursobench_db::{Operation, Request};
fn text(value: &str) -> Value {
    Value::Text(value.into())
}
fn visible_result(app: &App) -> &tursobench_core::ResultSet {
    &app.database.document.result
}
fn pane(app: &App, kind: tiling::Kind) -> egui::Rect {
    let panes = &app.database.workspace.pane_rects;
    panes.iter().find(|(k, _)| *k == kind).unwrap().1
}
fn schema_has(app: &App, name: &str) -> bool {
    app.database.schema.iter().any(|object| object.name == name)
}
fn assert_retained(app: &App, row: usize, col: usize, value: Value) {
    assert_eq!(
        visible_result(app).rows[row][col],
        value,
        "row {row}, column {col}"
    );
}
fn assert_staged(app: &App, row: usize, col: usize, value: Value) {
    let drafts = app.database.document.drafts.as_ref();
    let shown = drafts.and_then(|d| d.displayed(&visible_result(app).rows[row], col));
    assert_eq!(shown, Some(&value), "row {row}, column {col}");
}
fn wait_until(app: &mut App, ctx: &egui::Context, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    loop {
        app.poll(ctx);
        if done(app) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{}",
            app.database.status
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait(app: &mut App, ctx: &egui::Context) {
    wait_until(app, ctx, |a| {
        let d = &a.database;
        d.ready
            && !d.closing
            && d.document.active.is_none()
            && d.document.tools.pages.pending.is_none()
            && d.document.tools.context.pending.is_none()
            && d.catalog_ticket.is_none()
            && d.commit_ticket.is_none()
            && d.tools.count_request.is_none()
            && d.document.tools.inspection_ticket.is_none()
            && d.ui.diagram.ticket.is_none()
    });
}
fn wait_closed(app: &mut App, ctx: &egui::Context) {
    wait_until(app, ctx, |a| a.database.session.is_none());
}
fn browse_and_wait(app: &mut App, ctx: &egui::Context, table: &str) {
    app.browse_table(table.into());
    wait(app, ctx);
}
fn execute_and_wait(app: &mut App, ctx: &egui::Context, sql: &str) {
    app.run_text(sql.into(), None);
    wait(app, ctx);
}
fn close_and_wait(app: &mut App, ctx: &egui::Context) {
    app.close();
    wait_closed(app, ctx);
}
fn connected(path: &std::path::Path, writable: bool) -> (egui::Context, App) {
    let (ctx, mut app) = crate::app::test_ui::app();
    app.database.path = path.to_string_lossy().into_owned();

    app.database.writable = writable;
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    (ctx, app)
}
fn fixture(statements: &[&str], writable: bool) -> (tempfile::TempDir, egui::Context, App) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.db");
    crate::test_support::create(&path, statements);
    let (ctx, app) = connected(&path, writable);
    (dir, ctx, app)
}
/// A connection to the named local-project fixture database.
fn project(name: &str, writable: bool) -> (tempfile::TempDir, egui::Context, App) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, name)).unwrap();
    let (ctx, app) = connected(&path, writable);
    (dir, ctx, app)
}
/// A fixture holding one table, `CREATE TABLE table(columns)` with `rows`
/// inserted, browsed and loaded.
fn browsing(
    table: &str,
    columns: &str,
    rows: &str,
    writable: bool,
) -> (tempfile::TempDir, egui::Context, App) {
    let create = format!("CREATE TABLE {table}({columns})");
    let insert = format!("INSERT INTO {table} VALUES {rows}");
    let (dir, ctx, mut app) = fixture(&[&create, &insert], writable);
    browse_and_wait(&mut app, &ctx, table);
    (dir, ctx, app)
}
/// A writable fixture with the accessibility tree enabled, browsing `table`.
fn accessible(statements: &[&str], table: &str) -> (tempfile::TempDir, egui::Context, App) {
    let (dir, ctx, mut app) = fixture(statements, true);
    ctx.enable_accesskit();
    browse_and_wait(&mut app, &ctx, table);
    (dir, ctx, app)
}
/// Draw a frame and click the control labelled `label` in it.
fn click_control(app: &mut App, ctx: &egui::Context, label: &str) {
    let output = frame(app, ctx, vec![]);
    click(app, ctx, control(&output, label).center());
}
fn label_exists(output: &egui::FullOutput, label: &str) -> bool {
    super::test_ui::nodes(output)
        .iter()
        .any(|(_, node)| node.label() == Some(label))
}
/// The bounds of the button labelled `label`, if `output` drew one.
fn button(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    super::test_ui::nodes(output).iter().find_map(|(_, node)| {
        let button = node.label() == Some(label) && node.role() == egui::Role::Button;
        let bounds = node.bounds().filter(|_| button)?;
        let corner = |x: f64, y: f64| egui::pos2(x as f32, y as f32);
        Some(egui::Rect::from_min_max(
            corner(bounds.x0, bounds.y0),
            corner(bounds.x1, bounds.y1),
        ))
    })
}
/// The inspector-sized surface a record or structure pane is drawn on alone.
const PANEL: egui::Vec2 = egui::vec2(396.0, 244.0);
fn draw_panel(ctx: &egui::Context, draw: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
    super::test_ui::render(ctx, PANEL, vec![], draw)
}
/// Draw `draw` alone on the panel and click its control named `name`.
fn click_panel(ctx: &egui::Context, name: &str, mut draw: impl FnMut(&mut egui::Ui)) {
    let pos = control(&draw_panel(ctx, &mut draw), name).center();
    for pressed in [true, false] {
        super::test_ui::render(ctx, PANEL, super::test_ui::pointer(pos, pressed), &mut draw);
    }
}
/// Draw a frame at `size`, click the control labelled `label` in it with
/// frames of the same size and return where that control was.
fn click_control_at(
    app: &mut App,
    ctx: &egui::Context,
    size: egui::Vec2,
    label: &str,
) -> egui::Rect {
    let rect = control(&frame_at(app, ctx, size, vec![]), label);
    for pressed in [true, false] {
        frame_at(
            app,
            ctx,
            size,
            super::test_ui::pointer(rect.center(), pressed),
        );
    }
    rect
}
fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    for pressed in [true, false] {
        frame(app, ctx, super::test_ui::pointer(pos, pressed));
    }
}
fn press_key(app: &mut App, ctx: &egui::Context, key: egui::Key, modifiers: egui::Modifiers) {
    frame(app, ctx, vec![key_event(key, true, modifiers)]);
}
fn frame_at(
    app: &mut App,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    super::test_ui::render(ctx, size, events, |ui| app.render(ui))
}
/// The default frame size, and the laptop and ultrawide ones tests also draw at.
const DESKTOP: egui::Vec2 = egui::vec2(1400.0, 900.0);
const LAPTOP: egui::Vec2 = egui::vec2(1280.0, 720.0);
const WIDE: egui::Vec2 = egui::vec2(3440.0, 1440.0);
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    frame_at(app, ctx, DESKTOP, events)
}
/// Draw a frame with no input.
fn draw(app: &mut App, ctx: &egui::Context) -> egui::FullOutput {
    frame(app, ctx, vec![])
}
/// Draw two frames at `size` and return the settled second one.
fn settle(app: &mut App, ctx: &egui::Context, size: egui::Vec2) -> egui::FullOutput {
    frame_at(app, ctx, size, vec![]);
    frame_at(app, ctx, size, vec![])
}
/// Press and release `key` with no modifiers.
fn tap(app: &mut App, ctx: &egui::Context, key: egui::Key) {
    press_key(app, ctx, key, egui::Modifiers::NONE);
}
fn table_name(app: &App) -> &str {
    &app.database.document.table.as_ref().unwrap().name
}
/// The table the relationship map has selected.
fn selected(app: &App) -> Option<&str> {
    app.database.ui.diagram.selected.as_deref()
}
fn offset(app: &App) -> usize {
    app.database.document.browse.as_ref().unwrap().offset
}
/// Select from cell `from` to cell `to` and return the selected cells.
fn select_range(app: &mut App, from: (usize, usize), to: (usize, usize)) -> Vec<(usize, usize)> {
    app.select_cell(from.0, from.1, false);
    app.select_cell(to.0, to.1, true);
    app.selected_cells().collect()
}
/// Request the next page, or the previous one when `back`, and wait for it to
/// arrive without a paging error.
fn turn_page(app: &mut App, ctx: &egui::Context, back: bool) {
    app.request_page(back);
    wait(app, ctx);
    let error = &app.database.document.tools.pages.error;
    assert!(error.is_empty(), "{error}");
}
/// Whether a finished result completed with an error.
fn errored(result: &tursobench_core::ResultSet) -> bool {
    result.completion.as_ref().unwrap().error.is_some()
}
/// A result's ticket and rows, to compare after actions that must keep them.
fn held(result: &tursobench_core::ResultSet) -> (Option<Ticket>, Vec<Vec<Value>>) {
    (result.current, result.rows.clone())
}
fn drafts(app: &mut App) -> &mut tursobench_core::DraftBatch {
    app.database.document.drafts.as_mut().unwrap()
}
/// The next event a session reports, waiting up to five seconds.
fn next_event(session: &tursobench_db::Session) -> EventKind {
    let event = session.events.recv_timeout(Duration::from_secs(5));
    event.unwrap().kind
}
/// A filter comparing the first (id) column with `id`.
fn id_filter(op: FilterOp, id: i64) -> tursobench_core::Filter {
    tursobench_core::Filter {
        column: 0,
        op,
        value: Value::Integer(id),
    }
}
/// Open `path` in a new database workspace and wait until it is ready.
fn open_database(app: &mut App, ctx: &egui::Context, path: std::path::PathBuf, writable: bool) {
    app.request_database(ctx, path);
    app.database.writable = writable;
    inspect_selected(app, ctx);
    app.open(ctx);
    wait(app, ctx);
}
/// Tables `a` and `b`, each `(id INTEGER PRIMARY KEY, value TEXT)`, holding
/// the given `VALUES` rows.
fn two_tables(a: &str, b: &str) -> (tempfile::TempDir, egui::Context, App) {
    let create = |table: &str| format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY, value TEXT)");
    let (insert_a, insert_b) = (
        format!("INSERT INTO a VALUES {a}"),
        format!("INSERT INTO b VALUES {b}"),
    );
    fixture(&[&create("a"), &create("b"), &insert_a, &insert_b], true)
}
/// Small, laptop, desktop and ultrawide viewports, and one at 1.5 pixels per point.
const VIEWPORTS: [(f32, f32, f32); 5] = [
    (720.0, 480.0, 1.0),
    (1366.0, 768.0, 1.0),
    (1920.0, 1080.0, 1.0),
    (3440.0, 1440.0, 1.0),
    (1440.0, 960.0, 1.5),
];
/// Run `check` with each `(width, height, pixels per point)` viewport in the
/// dark and then the light theme, returning to one pixel per point.
fn across_themes(
    app: &mut App,
    ctx: &egui::Context,
    viewports: &[(f32, f32, f32)],
    mut check: impl FnMut(&mut App, f32, f32),
) {
    for dark in [true, false] {
        app.set_dark(ctx, dark);
        for &(width, height, scale) in viewports {
            ctx.set_pixels_per_point(scale);
            check(app, width, height);
        }
    }
    ctx.set_pixels_per_point(1.0);
}

#[test]
fn committed_sql_invalidates_other_tiles_and_refreshes_schema_after_loads_in_flight() {
    let (_dir, ctx, mut app) =
        browsing("t", "id INTEGER PRIMARY KEY, value TEXT", "(1,'old')", true);
    let table_view = app.database.document.id;
    for (sql, stale) in [
        ("BEGIN", false),
        ("UPDATE t SET value='new'", false),
        ("ROLLBACK", false),
        ("UPDATE t SET value='new'", true),
    ] {
        execute_and_wait(&mut app, &ctx, sql);
        let table = app.document_ref(table_view).unwrap();
        assert_eq!(table.stale, stale, "after {sql}");
        assert_eq!(table.result.rows[0][1], text("old"));
    }
    // A refresh asked for while the catalog or the map is still loading runs
    // once that load lands, so neither misses the new table.
    app.refresh_catalog();
    app.refresh_catalog();
    assert!(app.database.catalog_refresh_pending);
    app.run_text("CREATE TABLE added(id INTEGER PRIMARY KEY)".into(), None);
    app.load_diagram();
    wait(&mut app, &ctx);
    let database = &app.database;
    assert!(!database.catalog_refresh_pending && !database.diagram_refresh_pending);
    assert!(schema_has(&app, "added"));
    let diagram = database.ui.diagram.data.as_ref().unwrap();
    assert!(diagram.tables.iter().any(|table| table.name == "added"));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn external_changes_refresh_clean_tables_keep_sql_results_and_preserve_drafts() {
    let (_dir, ctx, mut app) = browsing(
        "t",
        "id INTEGER PRIMARY KEY, value TEXT",
        "(1,'initial')",
        true,
    );
    let table = app.database.document.id;
    execute_and_wait(&mut app, &ctx, "SELECT value FROM t");
    let query = app.database.document.id;
    let (db, c) = crate::test_support::run(async {
        let db = turso::Builder::new_local(&app.database.path)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        (db, c)
    });
    let external = |sql: &str| crate::test_support::run(c.execute(sql, ())).unwrap();
    external("UPDATE t SET value='external'");
    wait_until(&mut app, &ctx, |a| {
        let rows = &a.document_ref(table).unwrap().result.rows;
        rows.first().is_some_and(|r| r[1] == text("external"))
    });
    let result = app.document_ref(query).unwrap();
    assert!(result.stale);
    assert_eq!(result.result.rows, [[text("initial")]]);
    assert_eq!(
        app.database.document.id, query,
        "automatic refresh keeps keyboard focus"
    );
    app.switch_document(table);
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, text("draft"));
    external("UPDATE t SET value='changed again'");
    wait_until(&mut app, &ctx, |a| a.database.document.external_refresh);
    assert_staged(&app, 0, 1, text("draft"));
    assert_retained(&app, 0, 1, text("external"));
    external("CREATE TABLE added(id INTEGER)");
    wait_until(&mut app, &ctx, |a| schema_has(a, "added"));
    assert_staged(&app, 0, 1, text("draft"));
    app.database.document.drafts = None;
    drop((c, db));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn bulk_deletion_stays_staged_and_worker_termination_preserves_drafts_reporting_unknown_cleanup() {
    let (_dir, ctx, mut app) = browsing(
        "t",
        "id INTEGER PRIMARY KEY,value TEXT DEFAULT 'default'",
        "(1,'one'),(2,'two'),(3,'three')",
        true,
    );
    draw(&mut app, &ctx);
    app.database.document.tools.grid_selection = Some(super::selection::Selection {
        ticket: visible_result(&app).current,
        anchor: (0, 0),
        cursor: (1, 1),
    });
    app.stage_selected_deletions();
    let drafts = app.database.document.drafts.take().unwrap();
    assert_eq!(drafts.edits.len(), 2);
    assert_eq!(visible_result(&app).rows.len(), 3);
    app.begin_insert();
    assert_eq!(app.database.document.insert_fields.len(), 2);
    assert!(!app.database.document.insert_use_json);
    app.database.document.show_insert = false;
    app.stage_cell(0, 1, text("draft"));
    let old = app.database.session.take().unwrap();
    old.close();
    assert!(matches!(
        next_event(&old),
        EventKind::Closed { rollback: Ok(()) }
    ));
    drop(old);
    // An actual worker unwind drops the event sender without a Closed ack.
    let session = tursobench_db::Session::open(
        (&app.database.path).into(),
        tursobench_core::Access::ReadWrite,
        || panic!("injected worker termination"),
    )
    .unwrap();
    assert!(matches!(next_event(&session), EventKind::Opened(_)));
    app.database.commit_ticket = Some(session.ticket(1, 0));
    app.database.commit_owner = Some(app.database.document.id);
    app.database.session = Some(session);
    app.database.closing = true;
    app.quit = true;
    wait_closed(&mut app, &ctx);
    assert!(!app.database.ready && !app.quit && !app.database.closing);
    assert_eq!(app.database.transaction, TransactionState::Unknown);
    assert!(app.database.uncertain_edit && app.has_drafts());
    assert!(app.database.status.contains("UNKNOWN"));
    assert!(!app.can_edit());
    assert_staged(&app, 0, 1, text("draft"));
}

#[test]
#[ignore = "manual frame profile; run with --ignored --exact and --nocapture"]
fn profile_one_thousand_staged_edits() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO t SELECT x,'original' FROM n",
        ],
        true,
    );
    app.preferences.row_limit = 2000;
    browse_and_wait(&mut app, &ctx, "t");
    while visible_result(&app).rows.len() < 1000 {
        turn_page(&mut app, &ctx, false);
    }
    app.database.document.drafts = Some(tursobench_core::DraftBatch {
        table: app.database.document.table.clone().unwrap(),
        edits: visible_result(&app)
            .rows
            .iter()
            .map(|row| tursobench_core::Edit::Update {
                original: row.clone(),
                changes: vec![(1, text("changed"))],
            })
            .collect(),
    });
    for (label, review, frames) in [("grid", false, 20), ("review", true, 3)] {
        app.database.document.show_preview = review;
        draw(&mut app, &ctx);
        let start = Instant::now();
        for _ in 0..frames {
            draw(&mut app, &ctx);
        }
        println!(
            "{}",
            serde_json::json!({"profile":label,"drafts":1000,"frames":frames,"mean_ms":start.elapsed().as_secs_f64()*1000.0/f64::from(frames)})
        );
    }
    let document = &app.database.document;
    let drafts = document.drafts.as_ref();
    // The last twenty rows' two cells, looked up a thousand times.
    let tail = || (0..1000).flat_map(|_| (980..1000).flat_map(|row| [(row, 0), (row, 1)]));
    for cached in [false, true] {
        let lookup = |row: usize, col: usize| {
            if cached {
                document.tools.draft_ui.value(drafts, row, col)
            } else {
                drafts.unwrap().displayed(&document.result.rows[row], col)
            }
        };
        let start = Instant::now();
        for (row, col) in tail() {
            std::hint::black_box(lookup(row, col));
        }
        println!(
            "{}",
            serde_json::json!({"profile":"tail_draft_lookups","cached":cached,"elapsed_ms":start.elapsed().as_secs_f64()*1000.0})
        );
    }
    app.database.document.drafts = None;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn counts_keep_rows_and_drafts_and_filtered_counts_belong_to_their_view() {
    let (_dir, ctx, mut app) = browsing(
        "t",
        "id INTEGER PRIMARY KEY,value TEXT",
        "(1,'original'),(2,'second'),(3,'c'),(4,'d'),(5,'e')",
        true,
    );
    // An explicit count leaves the table's rows and drafts with their ticket.
    app.stage_cell(0, 1, text("draft"));
    let before = held(visible_result(&app));
    let unchanged = |app: &App| {
        assert_eq!(held(visible_result(app)), before);
        assert!(app.has_drafts());
    };
    let count = |app: &mut App, table: &str| {
        app.count_table(table.into());
        wait(app, &ctx);
    };
    count(&mut app, "t");
    assert_eq!(app.database.tools.counts.get("t"), Some(&5));
    unchanged(&app);
    count(&mut app, "missing table");
    assert!(app.database.status.contains("Count failed"));
    assert!(!app.database.tools.counts.contains_key("missing table"));
    unchanged(&app);
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.database.tools.counts.is_empty());
    // A filtered count belongs to its view, not to the table's total.
    let owner = app.database.document.id;
    app.add_filter(id_filter(FilterOp::Less, 3));
    wait(&mut app, &ctx);
    count(&mut app, "t");
    assert_eq!(app.database.document.tools.filtered_count, Some(2));
    assert!(!app.database.tools.counts.contains_key("t"));
    app.open_sql_window(false);
    count(&mut app, "t");
    assert_eq!(app.database.tools.counts.get("t"), Some(&5));
    let filtered = app.document_ref(owner).unwrap().tools.filtered_count;
    assert_eq!(filtered, Some(2));
    // A count replaced by a new filter is ignored.
    app.focus_window(owner);
    app.count_table("t".into());
    app.add_filter(id_filter(FilterOp::Greater, 1));
    wait(&mut app, &ctx);
    assert!(app.database.document.tools.filtered_count.is_none());
    close_and_wait(&mut app, &ctx);
}

#[test]
fn byte_limited_pages_continue_without_a_false_end_and_scroll_back_with_any_key_or_sort() {
    for (key, size, sort) in [
        ("PRIMARY KEY", 25000, None),
        ("", 25000, None),
        ("PRIMARY KEY", 50000, Some(1)),
    ] {
        let create = format!("CREATE TABLE t(id INTEGER {key},value TEXT)");
        let insert = format!(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<301) INSERT INTO t SELECT x,hex(zeroblob({size})) FROM n"
        );
        let (_dir, ctx, mut app) = fixture(&[&create, &insert], false);
        app.preferences.row_limit = 200;
        browse_and_wait(&mut app, &ctx, "t");
        let first = visible_result(&app).rows.len();
        assert!(first < 100);
        assert!(app.database.document.tools.pages.end.is_none());
        if sort.is_some() {
            app.database.document.browse.as_mut().unwrap().sort = sort;
            app.reload();
            wait(&mut app, &ctx);
        } else {
            turn_page(&mut app, &ctx, false);
            let next = Value::Integer(first as i64 + 1);
            assert!(visible_result(&app).rows.iter().any(|r| r[0] == next));
        }
        while app.database.document.tools.pages.end.is_none() {
            turn_page(&mut app, &ctx, false);
        }
        assert!(offset(&app) > 100);
        while offset(&app) > 0 {
            let before = offset(&app);
            turn_page(&mut app, &ctx, true);
            assert!(offset(&app) < before);
            assert_retained(&app, 0, 0, Value::Integer(offset(&app) as i64 + 1));
        }
        close_and_wait(&mut app, &ctx);
    }
}

#[test]
fn sql_selection_shortcut_and_window_expansion_have_distinct_targets() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE sample(id INTEGER PRIMARY KEY)"], false);
    app.open_sql_window(false);
    let editor = app.database.document.id;
    app.database.document.sql = "SELECT 73 AS answer".into();
    draw(&mut app, &ctx);
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows, [[Value::Integer(73)]]);
    assert_eq!(app.database.document.kind, tiling::Kind::Result);
    assert_eq!(app.database.workspace.expanded, None);
    assert_eq!(app.document_ref(editor).unwrap().sql, "SELECT 73 AS answer");
    let ticket = visible_result(&app).current;
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::CTRL);
    wait(&mut app, &ctx);
    assert_ne!(visible_result(&app).current, ticket);
    assert_eq!(visible_result(&app).rows, [[Value::Integer(73)]]);
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
    );
    assert_eq!(app.database.workspace.expanded, Some(tiling::Kind::Sql));
    assert!(app.database.ui.sidebar_rect.right() <= app.database.workspace.area.left());
    app.focus_window(editor);
    app.minimize_pane(tiling::Kind::Sql);
    let minimized = &app.database.workspace.minimized;
    assert!(minimized.contains(&tiling::Kind::Sql));
    app.focus_pane(tiling::Kind::Sql);
    assert_eq!(app.database.document.sql, "SELECT 73 AS answer");
    app.close();
}

#[test]
fn independent_documents_route_results_and_keep_filters_drafts_and_sql() {
    let (_dir, ctx, mut app) = two_tables("(1, 'alpha')", "(2, 'beta')");
    app.browse_table("a".into());
    let a_id = app.database.document.id;
    let a_ticket = visible_result(&app).current.unwrap();
    app.database.document.sql = "SELECT 'draft a'".into();
    app.database.document.filter = "value = alpha".into();
    app.browse_table("b".into());
    let b_id = app.database.document.id;
    app.database.document.sql = "SELECT 'draft b'".into();
    wait(&mut app, &ctx);
    assert_eq!(
        app.database.document.id, b_id,
        "background completions must not change focus"
    );
    assert_retained(&app, 0, 1, text("beta"));
    app.switch_document(a_id);
    assert_eq!(visible_result(&app).current, Some(a_ticket));
    assert_retained(&app, 0, 1, text("alpha"));
    assert_eq!(app.database.document.sql, "SELECT 'draft a'");
    assert_eq!(app.database.document.filter, "value = alpha");
    app.stage_cell(0, 1, text("staged alpha"));
    assert!(app.database.document.has_drafts());
    app.switch_document(b_id);
    assert!(app.has_drafts(), "draft protection is session wide");
    assert!(!app.can_edit(), "another view owns the pending edit batch");
    assert!(app.database.document.drafts.is_none());
    assert_eq!(app.database.document.sql, "SELECT 'draft b'");
    // Discarded and stale deliveries cannot change either view or steal focus.
    let b = visible_result(&app).current.unwrap();
    let original = visible_result(&app).rows.clone();
    let a = app.database.documents.get_mut(&a_id).unwrap();
    a.discard_ticket = Some(a_ticket);
    for ticket in [
        a_ticket,
        Ticket {
            generation: b.generation + 1,
            ..b
        },
        Ticket {
            database: b.database + 1,
            ..b
        },
        Ticket {
            session: b.session + 1,
            ..b
        },
    ] {
        for kind in [
            EventKind::Rows(vec![vec![Value::Integer(99)]]),
            EventKind::Columns(vec![]),
        ] {
            app.handle_event(&ctx, Event { ticket, kind });
            assert_eq!(app.database.document.id, b_id);
            assert_eq!(visible_result(&app).rows, original);
            assert_eq!(visible_result(&app).columns.len(), 2);
            assert_eq!(app.database.documents[&a_id].result.rows.len(), 1);
            assert_eq!(app.database.documents[&a_id].result.columns.len(), 2);
        }
    }
    // A closed view loses its rows, but its completed write still updates the session.
    app.database.documents.remove(&a_id);
    let completion = tursobench_core::Completion {
        elapsed_ms: 1,
        delivery: tursobench_core::Delivery::Complete,
        rows_seen: 1,
        page_has_more: None,
        changed: 1,
        transaction: TransactionState::Active,
        error: None,
    };
    let activity = app.database.activity.len();
    app.handle_event(
        &ctx,
        Event {
            ticket: Ticket {
                session: a_ticket.session + 1,
                ..a_ticket
            },
            kind: EventKind::Finished(completion.clone()),
        },
    );
    assert_eq!(app.database.activity.len(), activity);
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    for kind in [
        EventKind::Rows(vec![vec![Value::Integer(99)]]),
        EventKind::Columns(vec![]),
        EventKind::Finished(completion),
    ] {
        app.handle_event(
            &ctx,
            Event {
                ticket: a_ticket,
                kind,
            },
        );
    }
    assert_eq!(app.database.transaction, TransactionState::Active);
    assert_eq!(app.database.activity.len(), activity + 1);
    assert_eq!(visible_result(&app).rows, original);
    assert_eq!(visible_result(&app).columns.len(), 2);
    app.close();
}

/// Select a database that passes the advisory checks, then vanishes before it opens.
fn select_vanishing_database(
    app: &mut App,
    ctx: &egui::Context,
    dir: &std::path::Path,
    writable: bool,
) {
    let disappearing = dir.join("missing.db");
    crate::test_support::create(&disappearing, &["CREATE TABLE example(id INTEGER)"]);
    app.set_database_path(disappearing.to_str().unwrap().into());
    app.database.writable = writable;
    inspect_selected(app, ctx);
    std::fs::remove_file(&disappearing).unwrap();
}
#[test]
fn failed_open_after_close_can_retry_a_valid_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("valid.db");
    crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);
    let (ctx, mut app) = crate::app::test_ui::app();
    // The file can disappear after advisory checks; actual admission must still
    // report its asynchronous failure and complete a concurrent close request.
    select_vanishing_database(&mut app, &ctx, dir.path(), false);
    app.open(&ctx);
    assert!(app.database.session.is_some());
    app.request_close();
    assert!(app.database.closing);
    wait_closed(&mut app, &ctx);
    assert!(!app.database.ready && !app.database.closing && !app.database.confirm_close);
    assert!(app.database.document.active.is_none());
    assert!(
        app.preferences.discovery.recent.is_empty(),
        "failed opens must not enter history"
    );

    app.set_database_path(path.to_str().unwrap().into());

    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    assert!(schema_has(&app, "example"));
    assert_eq!(app.preferences.discovery.recent.len(), 1);
    assert_eq!(app.preferences.discovery.recent[0].path, path);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn failed_open_completes_pending_exit_after_any_export() {
    fn poll_and_check_close(app: &mut App, ctx: &egui::Context) -> bool {
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| app.poll(ctx));
        output.textures_delta.clear();
        output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Close))
        })
    }
    for pending_export in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut app) = crate::app::test_ui::app();
        select_vanishing_database(&mut app, &ctx, dir.path(), true);
        app.open(&ctx);
        app.quit = true;
        app.request_close();
        assert!(!app.database.confirm_close);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if pending_export {
            app.database.export_job = Some(rx);
        }
        let mut close_sent = false;
        let started = Instant::now();
        while app.database.session.is_some() {
            close_sent |= poll_and_check_close(&mut app, &ctx);
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.database.closing && !app.database.confirm_close);
        assert_eq!(close_sent, !pending_export);
        if pending_export {
            tx.send(Ok("Export completed".into())).unwrap();
            assert!(poll_and_check_close(&mut app, &ctx));
        }
    }
}

#[test]
fn diagram_preserves_drafts_and_result_ownership_and_chrome_fits_both_themes() {
    let (_dir, ctx, mut app) = project("scratchpad", true);
    browse_and_wait(&mut app, &ctx, "buffer_ids");
    let data = app.database.document.id;
    let before = held(visible_result(&app));
    app.stage_cell(0, 0, text("pending diagram edit"));
    app.show_structure();
    wait(&mut app, &ctx);
    assert_eq!(app.database.workspace.active, tiling::Kind::Structure);
    assert_eq!(held(&app.document_ref(data).unwrap().result), before);
    app.overview();
    let map = app.database.document.id;
    assert!(app.has_drafts());
    assert_eq!(app.database.document.kind, tiling::Kind::Map);
    across_themes(&mut app, &ctx, &VIEWPORTS, |app, width, height| {
        settle(app, &ctx, egui::vec2(width, height));
        let ui = &app.database.ui;
        let workspace = &app.database.workspace;
        assert!(ui.sidebar_rect.right() <= workspace.area.left());
        let viewport = ctx.content_rect();
        assert_eq!(ui.sidebar_rect.left(), viewport.left());
        assert_eq!(ui.sidebar_rect.top(), viewport.top());
        assert_eq!(ui.sidebar_rect.bottom(), viewport.bottom());
        let tile_of = |rect: egui::Rect| {
            let mut tiles = workspace.pane_rects.iter().map(|(_, tile)| *tile);
            tiles.find(|tile| tile.contains_rect(rect))
        };
        for rect in &ui.ribbon_actions {
            assert!(
                viewport.contains_rect(*rect),
                "ribbon action outside viewport: {rect:?}"
            );
            assert!(rect.left() >= ui.sidebar_rect.right());
            assert!(rect.height() >= 24.0);
            assert!(tile_of(*rect).is_some());
        }
        for (_, minimize, expand) in &workspace.controls {
            assert!((minimize.center().y - expand.center().y).abs() < 1.0);
            let tile = tile_of(*minimize).filter(|tile| tile.contains_rect(*expand));
            let tile = tile.unwrap_or_else(|| {
                panic!(
                    "controls outside tile at {width}x{height}: {minimize:?}, {expand:?}; {:?}",
                    workspace.pane_rects
                )
            });
            for action in ui.ribbon_actions.iter().filter(|a| tile.contains_rect(**a)) {
                assert!(
                    (action.center().y - minimize.center().y).abs() < 1.0,
                    "header controls must share one horizontal line: {action:?}, {minimize:?}"
                );
            }
        }
        for (_, rect) in &workspace.tiles {
            assert!(workspace.area.contains_rect(*rect));
        }
        let saved = workspace.minimized.clone();
        app.expand_window(map);
        frame_at(app, &ctx, egui::vec2(width, height), vec![]);
        let tiles = &app.database.workspace.tiles;
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].0, map);
        assert!(app.database.ui.sidebar_rect.right() <= tiles[0].1.left());
        app.expand_window(map);
        assert_eq!(app.database.workspace.minimized, saved);
    });
    let table = app.document_ref(data).unwrap();
    assert!(table.has_drafts());
    assert_eq!(table.result.rows, before.1);
    app.activate_table("buffer_ids".into());
    assert_eq!(app.database.document.id, data);
    app.close();
}

#[test]
fn reverted_empty_batch_cannot_carry_identity_to_another_table() {
    let (_dir, ctx, mut app) = two_tables("(1,'same')", "(1,'same')");
    browse_and_wait(&mut app, &ctx, "a");
    app.stage_cell(0, 1, text("temporary"));
    let row = visible_result(&app).rows[0].clone();
    drafts(&mut app).revert_row(&row);
    assert!(!app.has_drafts());
    browse_and_wait(&mut app, &ctx, "b");
    app.stage_cell(0, 1, text("only b"));
    assert_eq!(drafts(&mut app).table.name, "b");
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert_retained(&app, 0, 1, text("only b"));
    browse_and_wait(&mut app, &ctx, "a");
    assert_retained(&app, 0, 1, text("same"));
    let run_sql = |app: &mut App, sql: &str| {
        app.database.document.sql = sql.into();
        app.run_sql();
        wait(app, &ctx);
    };
    run_sql(&mut app, "BEGIN");
    browse_and_wait(&mut app, &ctx, "a");
    assert_eq!(app.database.transaction, TransactionState::Active);
    assert!(!app.can_edit());
    app.stage_cell(0, 1, text("only b"));
    assert!(!app.has_drafts());
    run_sql(&mut app, "ROLLBACK");
    browse_and_wait(&mut app, &ctx, "b");
    let retained = visible_result(&app).rows.clone();
    close_and_wait(&mut app, &ctx);
    assert!(app.database.status.contains("cleanup confirmed"));
    app.database.document.ui.nav = shell::Nav::Table;
    app.first_cell_rect = None;
    draw(&mut app, &ctx);
    assert!(app.first_cell_rect.is_some());
    assert_eq!(visible_result(&app).rows, retained);
    app.copy_results(&ctx, crate::export::Format::Json);
    assert!(app.database.status.contains("Copied retained"));
    app.browse_table("a".into());
    assert_eq!(table_name(&app), "a");
    assert_retained(&app, 0, 1, text("same"));
    app.database.document.ui.nav = shell::Nav::Result;
    draw(&mut app, &ctx);
    assert!(app.database.session.is_none());
}
#[test]
fn local_project_pages_and_fts_results_render_and_export() {
    let export = |app: &App, path: &std::path::Path, scope: &str| {
        let result = visible_result(app);
        let format = crate::export::Format::JsonLines;
        crate::export::write_result_as(path, format, scope, &result.columns, &result.rows, "t")
            .unwrap();
    };
    for name in crate::local_project_fixtures::NAMES {
        let (dir, ctx, mut app) = project(name, false);
        let (table, typed) = match name {
            "scratchpad" => ("buffer_ids", None),
            "agenda" => ("items", Some((5, "blob"))),
            "notifications" => ("notifications", Some((8, "null"))),
            _ => ("search_content", None),
        };
        browse_and_wait(&mut app, &ctx, table);
        assert_eq!(visible_result(&app).rows.len(), 100);
        assert!(!app.can_edit());
        draw(&mut app, &ctx);
        let path = dir.path().join("page.jsonl");
        export(&app, &path, "table-page");
        let contents = std::fs::read_to_string(path).unwrap();
        let records: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(records.len(), 102);
        assert_eq!(records[0]["scope"], "table-page");
        assert_eq!(records.last().unwrap()["rows"], 100);
        if let Some((column, kind)) = typed {
            assert_eq!(records[1]["values"][column]["type"], kind);
        }
        if name == "clipboard" {
            app.database.document.sql =
                "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)".into();
            app.database.document.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
            app.run_sql();
            wait(&mut app, &ctx);
            assert_eq!(visible_result(&app).rows, [[text("entry-0000")]]);
            draw(&mut app, &ctx);
            export(&app, &dir.path().join("fts.jsonl"), "complete-query-result");
        }
        close_and_wait(&mut app, &ctx);
        assert!(app.database.status.contains("cleanup confirmed"));
    }
}

#[test]
fn click_inspect_stage_no_write_failed_commit_retains_drafts() {
    let (_dir, ctx, mut app) = browsing(
        "items",
        "id INTEGER PRIMARY KEY, name TEXT",
        "(1, 'original'), (2, 'other')",
        true,
    );
    settle(&mut app, &ctx, DESKTOP);
    let pos = app.first_cell_rect.unwrap().center();
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)],
    );
    frame(&mut app, &ctx, vec![pointer_button(pos, false)]);
    assert_eq!(app.database.document.inspect, Some((0, 0)));
    app.clone_row(0);
    let clone: Vec<Option<Value>> =
        serde_json::from_str(&app.database.document.insert_json).unwrap();
    assert_eq!(clone, vec![None, Some(text("original"))]);
    assert!(!app.has_drafts());
    draw(&mut app, &ctx);
    click_control(&mut app, &ctx, "Stage insert (no database write)");
    let insert = app.database.document.drafts.take().unwrap();
    assert_eq!(
        insert.edits,
        vec![tursobench_core::Edit::Insert { values: clone }]
    );
    assert_eq!(visible_result(&app).rows.len(), 2);
    app.database.document.show_insert = false;
    app.stage_cell(0, 1, text("draft"));
    app.update_draft_presentation();
    let document = &app.database.document;
    let draft = document.drafts.as_ref().unwrap();
    let cached = document.tools.draft_ui.value(Some(draft), 0, 1).unwrap();
    let displayed = draft.displayed(&document.result.rows[0], 1).unwrap();
    assert!(std::ptr::eq(cached, displayed));
    app.database.document.show_preview = true;
    draw(&mut app, &ctx);
    click_control(&mut app, &ctx, "Remove operation");
    assert!(
        !app.has_drafts(),
        "review removes the selected local operation"
    );
    assert_retained(&app, 0, 1, text("original"));
    app.database.document.show_preview = false;
    app.stage_cell(0, 1, text("draft"));
    let original = visible_result(&app).rows[0].clone();
    assert_staged(&app, 0, 1, text("draft"));
    drafts(&mut app).revert_cell(&original, 1);
    assert!(!app.has_drafts());
    app.stage_delete(0);
    assert!(drafts(&mut app).deleted(&original));
    drafts(&mut app).revert_row(&original);
    assert!(!app.has_drafts());
    app.stage_cell(0, 0, Value::Integer(2));
    assert!(app.has_drafts());
    assert!(app.database.commit_ticket.is_none());
    // Inspecting, rendering and staging produced no SQL request or row mutation.
    assert_retained(&app, 0, 0, Value::Integer(1));
    draw(&mut app, &ctx);
    let s = app.database.session.as_ref().unwrap();
    app.database.sequence += 1;
    s.submit(Request {
        ticket: s.ticket(app.database.sequence, 0),
        operation: Operation::Execute {
            sql: "SELECT id FROM items ORDER BY id".into(),
            parameters: vec![],
            budget: Budget::default(),
        },
    })
    .unwrap();
    let mut rows = vec![];
    loop {
        match next_event(s) {
            EventKind::Rows(r) => rows.extend(r),
            EventKind::Finished(c) => {
                assert!(c.error.is_none());
                break;
            }
            _ => {}
        }
    }
    assert_eq!(rows, [[Value::Integer(1)], [Value::Integer(2)]]);
    // Deliberate key collision fails and retains local drafts.
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.has_drafts());
    assert!(app.database.status.contains("drafts retained"));
    app.stage_cell(0, 0, Value::Integer(3));
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_retained(&app, 1, 0, Value::Integer(3));
    // An ambiguous acknowledgement must retain the owning view's draft and
    // prevent both another commit and further staging, even in the background.
    app.stage_cell(0, 1, text("uncertain draft"));
    let owner = app.database.document.id;
    let other = *app.database.documents.keys().next().unwrap();
    app.database.sequence += 1;
    let database = &app.database;
    let session = database.session.as_ref().unwrap();
    let ticket = session.ticket(database.sequence, database.generation);
    app.database.commit_ticket = Some(ticket);
    app.database.commit_owner = Some(owner);
    app.switch_document(other);
    app.handle_event(
        &ctx,
        Event {
            ticket,
            kind: EventKind::EditsCommitted(tursobench_core::CommitOutcome::Unknown(
                tursobench_core::Failure::input("lost commit acknowledgement"),
            )),
        },
    );
    assert_eq!(app.database.document.id, other);
    assert!(app.database.uncertain_edit && app.has_drafts());
    let status = &app.database.status;
    assert!(status.contains("UNKNOWN") && status.contains("Do not retry"));
    assert!(app.database.commit_ticket.is_none() && app.database.commit_owner.is_none());
    app.switch_document(owner);
    assert!(!app.can_edit());
    let sequence = app.database.sequence;
    app.commit_drafts();
    app.stage_cell(0, 1, text("blocked edit"));
    assert_eq!(app.database.sequence, sequence);
    assert_staged(&app, 0, 1, text("uncertain draft"));
    app.close();
}

#[test]
fn selection_named_history_metadata_reference_back_and_tools_render() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT,payload TEXT,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'001','{\"x\":1}')",
            "INSERT INTO child VALUES(1,7,'001'),(2,NULL,NULL)",
        ],
        false,
    );
    let mut schema = app.database.schema.iter();
    assert!(schema.any(|o| o.name == "child" && o.ddl.as_ref().unwrap().contains("FOREIGN KEY")));
    browse_and_wait(&mut app, &ctx, "child");
    assert!(app.database.document.tools.inspection_error.is_empty());
    let inspection = app.database.document.tools.inspection.as_ref().unwrap();
    let key = inspection.foreign_keys[0].clone();
    assert!(app.cell_references(1, 1).is_empty());
    app.database.document.ui.nav = shell::Nav::Table;
    draw(&mut app, &ctx);
    let link = app.fk_cell_rect.expect("composite FK rendered as a link");
    click(&mut app, &ctx, link.center());
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.ui.nav, shell::Nav::Table);
    assert_eq!(selected(&app), Some("parent"));
    assert_eq!(table_name(&app), "parent");
    assert_eq!(visible_result(&app).rows.len(), 1);
    assert_eq!(app.export_scope(), "related-row-lookup");
    app.follow_incoming(0, "child".into(), key.clone());
    wait(&mut app, &ctx);
    assert_eq!(table_name(&app), "child");
    assert_eq!(visible_result(&app).rows.len(), 1);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(selected(&app), Some("parent"));
    app.database.document.tools.detail = Some(0);
    app.show_structure();
    app.database.tools.help = true;
    let parent = app.database.schema.iter().find(|o| o.name == "parent");
    app.database.document.tools.ddl = parent.cloned();
    draw(&mut app, &ctx);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(table_name(&app), "child");
    assert_eq!(selected(&app), Some("child"));
    assert_eq!(visible_result(&app).rows.len(), 2);
    app.follow_key(1, key);
    assert!(app.database.status.contains("NULL foreign key"));
    assert!(app.database.document.tools.related.is_none());
    app.database.document.sql = "SELECT 'λ;'; SELECT :name AS value;".into();
    let byte = app.database.document.sql.find("SELECT :name").unwrap();
    let cursor = app.database.document.sql[..byte].chars().count();
    app.database.document.tools.selection = cursor..cursor;
    app.database.document.params =
        r#"{":name":{"type":"Text","value":"bound-only-secret"}}"#.into();
    app.run_current(None);
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows, [[text("bound-only-secret")]]);
    assert!(app.database.document.sql.contains("λ;"));
    let history = &app.preferences.query_history[0];
    assert_eq!(history.database, app.database.path);
    assert!(history.elapsed_ms.is_some());
    assert!(!history.sql.contains("λ"));
    assert!(!history.failed);
    let saved = serde_json::to_string(&app.preferences).unwrap();
    assert!(!saved.contains("bound-only-secret"));
    app.run_text("SELECT * FROM parent WHERE a=?1".into(), Some(false));
    wait(&mut app, &ctx);
    assert!(app.preferences.query_history[0].failed);
    let history = &app.preferences.query_history[0].sql;
    assert!(history.starts_with("EXPLAIN QUERY PLAN"));
    let explained = "SELECT * FROM parent";
    app.database.document.sql = explained.into();
    app.database.document.params = "[]".into();
    app.run_text(explained.into(), Some(false));
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.view, View::Explain);
    assert!(!visible_result(&app).rows.is_empty());
    app.database.document.tools.complete = true;
    draw(&mut app, &ctx);
    app.close();
}

#[test]
fn overview_and_table_views_preserve_camera_and_rejects_navigation_with_drafts() {
    let (_dir, ctx, mut app) = two_tables("(1, 'original')", "(2, 'other')");
    assert_eq!(app.database.document.ui.nav, shell::Nav::Overview);
    let diagram = &app.database.ui.diagram;
    assert_eq!(diagram.data.as_ref().unwrap().tables.len(), 2);
    draw(&mut app, &ctx);
    // Explicit user camera choices survive opening/resizing other views.
    app.database.ui.diagram.freeze_camera();
    let camera = |app: &App| {
        let camera = &app.database.ui.diagram.camera;
        (camera.zoom, camera.pan)
    };
    let frozen = camera(&app);
    browse_and_wait(&mut app, &ctx, "a");
    assert_eq!(app.database.document.ui.nav, shell::Nav::Table);
    assert_retained(&app, 0, 1, text("original"));
    let retained_ticket = visible_result(&app).current;
    app.refresh_catalog();
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).current, retained_ticket);
    assert_retained(&app, 0, 1, text("original"));
    for view in [
        shell::TableView::Data,
        shell::TableView::Structure,
        shell::TableView::Data,
    ] {
        app.database.document.ui.table_view = view;
        draw(&mut app, &ctx);
        assert_eq!(camera(&app), frozen);
    }
    app.stage_cell(0, 1, text("draft"));
    let ticket = visible_result(&app).current;
    app.browse_table("b".into());
    assert_eq!(visible_result(&app).current, ticket);
    assert_eq!(selected(&app), Some("a"));
    assert_eq!(table_name(&app), "a");
    app.overview();
    app.activate_table("a".into());
    assert_eq!(app.database.document.ui.nav, shell::Nav::Table);
    assert_eq!(visible_result(&app).current, ticket);
    assert!(app.has_drafts());
    app.database.document.drafts = None;
    app.close();
}

#[test]
fn rapid_map_selection_keeps_latest_rows_and_structure_including_views() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "INSERT INTO b VALUES(2)",
            "CREATE VIEW v AS SELECT * FROM b",
            "CREATE TRIGGER b_insert AFTER INSERT ON b BEGIN SELECT 1; END",
        ],
        false,
    );
    app.browse_table("a".into());
    browse_and_wait(&mut app, &ctx, "b");
    assert_eq!(visible_result(&app).rows, [[Value::Integer(2)]]);
    assert_eq!(table_name(&app), "b");
    assert_eq!(selected(&app), Some("b"));
    let mut schema = app.database.schema.iter();
    assert!(schema.any(|o| o.kind == "trigger" && o.table == "b"));
    app.database.ui.sidebar = shell::ToolPanel::Schema;
    browse_and_wait(&mut app, &ctx, "v");
    let inspection = app.database.document.tools.inspection.as_ref().unwrap();
    assert_eq!(inspection.table.name, "v");
    assert!(!app.can_edit());
    app.database.document.ui.table_view = shell::TableView::Structure;
    draw(&mut app, &ctx);
    assert_eq!(selected(&app), Some("v"));
    app.close();
}

#[test]
fn sql_tile_keeps_result_identity_and_read_transactions_visible() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
        ],
        false,
    );
    draw(&mut app, &ctx);
    assert!(app.database.ui.transaction_rects.is_empty());
    app.toggle_sql();
    execute_and_wait(&mut app, &ctx, "SELECT 42 AS answer");
    assert_eq!(app.database.document.ui.nav, shell::Nav::Result);
    assert_eq!(app.database.document.view, View::Query);
    assert!(app.database.document.table.is_none());
    assert_eq!(visible_result(&app).rows, [[Value::Integer(42)]]);
    let ticket = visible_result(&app).current;
    let result_id = app.database.document.id;
    app.toggle_sql();
    assert_eq!(app.database.document.kind, tiling::Kind::Sql);
    draw(&mut app, &ctx);
    assert_eq!(app.document_ref(result_id).unwrap().result.current, ticket);
    app.toggle_sql();
    execute_and_wait(&mut app, &ctx, "BEGIN");
    assert_eq!(app.database.transaction, TransactionState::Active);
    app.toggle_sql();
    app.overview();
    draw(&mut app, &ctx);
    assert_eq!(app.database.ui.transaction_rects.len(), 2);
    let rollback = app.database.ui.transaction_rects[1].center();
    click(&mut app, &ctx, rollback);
    wait(&mut app, &ctx);
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    app.sidebar_search.focus = true;
    app.sidebar_search.query = "a.id".into();
    draw(&mut app, &ctx);
    wait(&mut app, &ctx);
    let output = frame_at(&mut app, &ctx, LAPTOP, vec![]);
    control(&output, "a.id · column");
    tap(&mut app, &ctx, egui::Key::Enter);
    wait(&mut app, &ctx);
    assert_eq!(table_name(&app), "a");
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("a".into(), "id".into()))
    );
    assert!(!app.database.ui.palette_open);
    app.close();
}

#[test]
fn access_switch_waits_for_closed_then_opens_a_new_session() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE a(id INTEGER PRIMARY KEY)"], false);
    let old_session = app.database.session.as_ref().unwrap().id;
    let output = draw(&mut app, &ctx);
    app.database.ui.diagram.freeze_camera();
    app.database.ui.diagram.camera.zoom = 0.7;
    app.database.ui.diagram.camera.pan = egui::vec2(30.0, 42.0);
    let access = button(&output, "Read only").expect("status-bar access icon");
    click(&mut app, &ctx, access.center());
    assert!(app.database.ui.access_dialog);
    assert_eq!(app.database.session.as_ref().unwrap().id, old_session);
    click_control(&mut app, &ctx, "Close cleanly and reopen");
    assert!(app.database.closing);
    assert!(!app.database.writable);
    wait(&mut app, &ctx);
    assert!(app.database.writable);
    assert!(button(&draw(&mut app, &ctx), "Read/write").is_some());
    assert_ne!(app.database.session.as_ref().unwrap().id, old_session);
    assert_eq!(app.database.document.ui.nav, shell::Nav::Overview);
    assert!(app.database.pending_access.is_none());
    assert_eq!(app.database.ui.diagram.camera.zoom, 0.7);
    assert_eq!(app.database.ui.diagram.camera.pan, egui::vec2(30.0, 42.0));
    app.close();
}

#[test]
fn navigation_restores_typed_browse_context_and_query_return_without_dropping_drafts() {
    let (_dir, ctx, mut app) = two_tables("(1,'first'),(2,'second')", "(3,'other')");
    browse_and_wait(&mut app, &ctx, "a");
    let browse = app.database.document.browse.as_mut().unwrap();
    browse.sort = Some(1);
    browse.descending = true;
    browse.filters.push(id_filter(FilterOp::Equal, 2));
    app.reload();
    wait(&mut app, &ctx);
    app.database.document.inspect = Some((0, 1));
    app.database.document.tools.detail = Some(0);
    app.database.document.tools.grid_scroll = egui::vec2(40.0, 20.0);
    browse_and_wait(&mut app, &ctx, "b");
    app.navigate_back();
    wait(&mut app, &ctx);
    let browse = app.database.document.browse.as_ref().unwrap();
    assert_eq!((browse.sort, browse.descending), (Some(1), true));
    assert_eq!(
        visible_result(&app).rows,
        [[Value::Integer(2), text("second")]]
    );
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    assert_eq!(app.database.document.tools.detail, Some(0));
    assert_eq!(
        app.database.document.tools.restore_scroll,
        Some(egui::vec2(40.0, 20.0))
    );
    execute_and_wait(&mut app, &ctx, "SELECT 42");
    assert!(!app.can_edit());
    assert!(app.database.tools.result_return.is_some());
    app.return_to_table();
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    app.stage_cell(0, 1, text("draft"));
    let ticket = visible_result(&app).current;
    app.browse_table("b".into());
    app.run_text("SELECT 99".into(), None);
    app.navigate_back();
    assert_eq!(visible_result(&app).current, ticket);
    assert!(app.has_drafts());
    app.close();
}

#[test]
fn settings_shortcut_opens_a_window_without_replacing_the_database_tool() {
    let (ctx, mut app) = crate::app::test_ui::app();
    draw(&mut app, &ctx);
    press_key(&mut app, &ctx, egui::Key::Comma, egui::Modifiers::CTRL);
    let output = frame_at(&mut app, &ctx, LAPTOP, vec![]);
    assert!(app.settings.open);
    assert_eq!(app.database.ui.sidebar, shell::ToolPanel::Schema);
    assert_eq!(app.database.document.ui.nav, shell::Nav::Databases);
    for name in [
        "Appearance",
        "Discovery",
        "Database resources",
        "Queries and SQL recovery",
    ] {
        control(&output, name);
    }
    let initial = app.settings.page;
    click_control(&mut app, &ctx, "Queries and SQL recovery");
    let chosen = app.settings.page;
    assert_ne!(chosen, initial);
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!app.settings.open);
    press_key(&mut app, &ctx, egui::Key::Comma, egui::Modifiers::CTRL);
    assert_eq!(app.settings.page, chosen);
    assert!(app.settings.open);
    app.open_palette();
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!app.database.ui.palette_open);
    assert!(app.settings.open);
    assert!(app.database.session.is_none());
}

#[test]
fn sidebar_catalog_includes_views_when_map_is_incomplete_and_tools_preserve_rows() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE example(id INTEGER PRIMARY KEY)",
            "INSERT INTO example VALUES(1)",
            "CREATE VIEW example_view AS SELECT * FROM example",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "example");
    app.database.ui.diagram.data = Some(tursobench_core::SchemaDiagram {
        tables: vec![],
        limited: true,
    });
    let before = held(visible_result(&app));
    for panel in [
        shell::ToolPanel::History,
        shell::ToolPanel::Saved,
        shell::ToolPanel::Diagnostics,
    ] {
        let nav = app.database.document.ui.nav;
        app.select_tool(panel);
        assert_eq!(app.database.ui.sidebar, panel);
        assert_eq!(app.database.document.ui.nav, nav);
        let output = frame_at(&mut app, &ctx, LAPTOP, vec![]);
        assert!(output.shapes.len() > 10);
        app.select_tool(shell::ToolPanel::Schema);
        assert_eq!(held(visible_result(&app)), before);
    }
    browse_and_wait(&mut app, &ctx, "example_view");
    assert_eq!(visible_result(&app).rows, before.1);
    assert!(!app.can_edit());
    app.close();
}

#[test]
fn table_layout_aligns_retained_data_and_keeps_status_navigation_accessible() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE alignment_table(label TEXT PRIMARY KEY,value INTEGER)",
            "INSERT INTO alignment_table VALUES('aligned-cell',42)",
            "CREATE TABLE status_test(id INTEGER PRIMARY KEY)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<205) INSERT INTO status_test SELECT x FROM n",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "alignment_table");
    ctx.enable_accesskit();
    let viewports = [
        (900.0, 600.0, 1.0),
        (1400.0, 900.0, 1.0),
        (1440.0, 960.0, 1.5),
    ];
    across_themes(&mut app, &ctx, &viewports, |app, width, height| {
        let size = egui::vec2(width, height);
        app.database.workspace.expanded = (width > 900.0).then_some(tiling::Kind::Data);
        let output = settle(app, &ctx, size);
        let pane = pane(app, tiling::Kind::Data);
        let text_position = |value: &str| {
            texts(&output)
                .find(|text| text.galley.job.text == value && pane.contains(text.pos))
                .map(|text| text.pos)
                .unwrap_or_else(|| panic!("Missing tile text {value}"))
        };
        let bar = control(&output, "Table data status");
        assert!(
            (bar.height() - super::grid::STATUS_HEIGHT).abs() < 0.1,
            "{bar:?}"
        );
        assert!(
            (bar.bottom() - (pane.bottom() - 4.0)).abs() < 1.1,
            "bar {bar:?}, pane {pane:?}"
        );
        for name in ["Row number", "Go to row", "Count matching rows"] {
            assert!(bar.contains_rect(control(&output, name)), "{name}: {bar:?}");
        }
        let title = text_position("alignment_table");
        let cell = text_position("aligned-cell");
        let status = text_position("1–1 · 1 retained");
        assert!(
            (title.x - cell.x).abs() < 1.1,
            "title {title:?}, cell {cell:?}, pane {pane:?}"
        );
        assert!(
            (status.x - cell.x).abs() < 1.1,
            "status {status:?}, cell {cell:?}"
        );
        let mut controls = app.database.workspace.controls.iter();
        let expand = controls.find(|c| c.0 == tiling::Kind::Data).unwrap().2;
        let count = control(&output, "Count matching rows");
        assert!(
            (expand.right() - count.right()).abs() < 1.1,
            "expand {expand:?}, count {count:?}"
        );
        let header = |rect: egui::Rect| {
            (rect.height() - super::grid::HEADER_HEIGHT).abs() < 1.1
                && rect.width() >= pane.width() - 9.0
                && pane.contains_rect(rect)
        };
        let mut shapes = output.shapes.iter().map(|shape| &shape.shape);
        assert!(
            shapes.any(|shape| matches!(shape, egui::Shape::Rect(r) if header(r.rect))),
            "full-width column header"
        );
    });
    browse_and_wait(&mut app, &ctx, "status_test");
    click_control(&mut app, &ctx, "Count matching rows");
    wait(&mut app, &ctx);
    assert_eq!(app.database.tools.counts.get("status_test"), Some(&205));
    app.database.document.tools.jump_row = 151;
    click_control(&mut app, &ctx, "Go to row");
    wait(&mut app, &ctx);
    assert_eq!(offset(&app), 150);
    assert_eq!(visible_result(&app).rows[0], vec![Value::Integer(151)]);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn table_views_sidebar_and_transactions_fit_themes_and_scaled_viewports() {
    let name = "a_table_with_a_deliberately_long_name_to_exercise_header_truncation";
    let (_dir, ctx, mut app) = fixture(
        &[
            &format!("CREATE TABLE {name}(id INTEGER PRIMARY KEY,value TEXT)"),
            &format!(
                "INSERT INTO {name} WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200) SELECT x,'value' FROM n"
            ),
        ],
        false,
    );
    ctx.enable_accesskit();
    browse_and_wait(&mut app, &ctx, name);
    let mut viewports = vec![(1280.0, 720.0, 1.0), (960.0, 640.0, 1.0)];
    viewports.extend(VIEWPORTS);
    across_themes(&mut app, &ctx, &viewports, |app, width, height| {
        use shell::TableView::{Data, Structure};
        for (sql, view, details) in [
            (false, Data, false),
            (false, Data, true),
            (false, Structure, false),
            (true, Data, false),
            (true, Structure, false),
        ] {
            let document = &mut app.database.document;
            document.ui.sql_open = sql;
            document.ui.table_view = view;
            document.ui.inspector_open = details;
            document.inspect = Some((0, 0));
            for _ in 0..2 {
                let output = frame_at(app, &ctx, egui::vec2(width, height), vec![]);
                assert!(!label_exists(&output, "Insert row"));
            }
            let ui = &app.database.ui;
            assert!(ui.sidebar_rect.width() >= 180.0);
            assert_eq!(app.database.workspace.pane_rects.len(), 5);
            if view == Data {
                assert!(
                    ui.workspace_rect.contains_rect(ui.grid_rect),
                    "{width}x{height}: workspace {:?}, grid {:?}, tiles {:?}",
                    ui.workspace_rect,
                    ui.grid_rect,
                    app.database.workspace.tiles
                );
            }
            if width == 1280.0 && (sql, view, details) == (false, Data, false) {
                let rows = ui.visible_rows;
                assert!(rows >= 9, "{rows} fully visible rows");
            }
        }
    });
    execute_and_wait(&mut app, &ctx, "BEGIN");
    app.database.document.ui.sql_open = false;
    app.overview();
    settle(&mut app, &ctx, egui::vec2(720.0, 480.0));
    assert_eq!(app.database.ui.transaction_rects.len(), 2);
    for r in &app.database.ui.transaction_rects {
        assert!(ctx.content_rect().contains_rect(*r), "{r:?}");
    }
    app.close();
}

#[test]
fn escape_and_search_close_popups_without_replacing_the_retained_result() {
    let (_dir, ctx, mut app) = browsing("a", "id INTEGER PRIMARY KEY", "(1)", false);
    let ticket = visible_result(&app).current;
    let popup = egui::Id::unique("test-menu");
    egui::Popup::open_id(&ctx, popup);
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!egui::Popup::is_any_open(&ctx));
    egui::Popup::open_id(&ctx, popup);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(app.database.ui.palette_open);
    assert!(!egui::Popup::is_any_open(&ctx));
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!app.database.ui.palette_open);
    app.database.document.ui.sql_open = true;
    app.database.document.ui.filter_open = true;
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!app.database.document.ui.filter_open);
    assert!(app.database.document.ui.sql_open);
    for (key, modifiers, opened) in [
        (
            egui::Key::E,
            egui::Modifiers::CTRL,
            (|a: &App| a.database.ui.export_open) as fn(&App) -> bool,
        ),
        (egui::Key::F, egui::Modifiers::CTRL, |a: &App| {
            a.database.document.ui.filter_open
        }),
        (egui::Key::F1, egui::Modifiers::NONE, |a: &App| {
            a.database.tools.help
        }),
    ] {
        press_key(&mut app, &ctx, key, modifiers);
        assert!(opened(&app), "{key:?}");
        tap(&mut app, &ctx, egui::Key::Escape);
        assert!(!opened(&app), "Escape must close {key:?}'s surface");
    }
    app.database.document.tools.columns.hidden.insert(0);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Text("Show hidden columns".into())],
    );
    tap(&mut app, &ctx, egui::Key::Enter);
    assert!(!app.database.ui.palette_open);
    assert!(app.database.document.tools.columns.hidden.is_empty());
    assert_eq!(visible_result(&app).current, ticket);
    app.close();
}

#[test]
fn column_presentation_preserves_schema_indices_and_manual_widths() {
    let (_dir, ctx, mut app) = browsing(
        "sample",
        "id INTEGER PRIMARY KEY, label TEXT, number INTEGER",
        "(1,'a long string',42)",
        true,
    );
    draw(&mut app, &ctx);
    ctx.enable_accesskit();
    for (label, sorted, descending) in [
        ("Column number", Some(2), false),
        ("Column number ascending", Some(2), true),
        ("Column number descending", None, false),
    ] {
        click_control(&mut app, &ctx, label);
        wait(&mut app, &ctx);
        let browse = app.database.document.browse.as_ref().unwrap();
        assert_eq!((browse.sort, browse.descending), (sorted, descending));
    }
    let columns = &mut app.database.document.tools.columns;
    columns.visuals.insert(1, CellStyle::Status);
    draw(&mut app, &ctx);
    assert_retained(&app, 0, 1, text("a long string"));
    let columns = &mut app.database.document.tools.columns;
    columns.move_before(2, 0);
    for (from, target, expected) in [
        (2, 2, vec![2, 0, 1]),
        (9, 0, vec![2, 0, 1]),
        (0, 9, vec![2, 0, 1]),
        (2, 1, vec![0, 2, 1]),
        (2, 0, vec![2, 0, 1]),
    ] {
        columns.move_before(from, target);
        assert_eq!(columns.order, expected);
    }
    columns.hidden.insert(1);
    columns.manual.insert(2);
    columns.widths[2] = 217.0;
    draw(&mut app, &ctx);
    assert_eq!(app.visible_columns(), vec![2, 0]);
    assert_eq!(app.database.document.tools.columns.widths[2], 217.0);
    app.open_cell(0, 2);
    assert_eq!(app.database.document.tools.cell_editor.input, "42");
    app.stage_cell(0, 2, Value::Integer(55));
    assert_staged(&app, 0, 2, Value::Integer(55));
    assert_retained(&app, 0, 0, Value::Integer(1));
    super::test_ui::render(&ctx, egui::vec2(300.0, 240.0), vec![], |ui| {
        let rect = ui.available_rect_before_wrap();
        for (style, name, value, painted) in [
            (CellStyle::Auto, "status", text("blocked"), true),
            (CellStyle::Status, "state", text("pending"), true),
            (CellStyle::Percent, "progress", Value::Real(50.0), false),
            (CellStyle::Priority, "priority", Value::Integer(3), false),
            (CellStyle::Priority, "priority", Value::Null, false),
            (CellStyle::Percent, "progress", Value::Real(f64::NAN), false),
        ] {
            assert_eq!(style.paint(ui, rect, name, &value), painted);
        }
    });
    let menu_ctx = egui::Context::default();
    menu_ctx.enable_accesskit();
    let columns = &mut app.database.document.tools.columns;
    for label in ["Fit contents", "Move left", "Move right", "Hide column"] {
        click_panel(&menu_ctx, label, |ui| columns.header_menu(ui, 2));
    }
    assert!(!columns.manual.contains(&2));
    assert!(columns.refit);
    assert_eq!(columns.order, vec![0, 2, 1]);
    assert!(columns.hidden.contains(&2));
    app.close();
}

#[test]
fn scroll_pages_append_evict_reload_and_reject_stale_or_draft_owned_fetches() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER)",
            "INSERT INTO sample WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<450) SELECT x,x FROM n",
        ],
        true,
    );
    app.preferences.row_limit = 400;
    browse_and_wait(&mut app, &ctx, "sample");
    let (owner, first) = held(visible_result(&app));
    app.request_page(false);
    assert_eq!(visible_result(&app).rows, first);
    assert!(app.database.document.tools.pages.pending.is_some());
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows.len(), 200);
    assert_eq!(visible_result(&app).current, owner);
    turn_page(&mut app, &ctx, false);
    draw(&mut app, &ctx);
    app.select_cell(150, 0, false);
    app.database.document.tools.grid_scroll.y = 3000.0;
    execute_and_wait(&mut app, &ctx, "SELECT 765");
    app.return_to_table();
    wait(&mut app, &ctx);
    for _ in 0..2 {
        draw(&mut app, &ctx);
        wait(&mut app, &ctx);
    }
    assert_eq!(visible_result(&app).rows.len(), 300);
    assert_eq!(app.database.document.inspect, Some((150, 0)));
    assert_eq!(
        app.database.document.tools.grid_selection.unwrap().cursor,
        (150, 0)
    );
    assert!(app.database.document.tools.restore_navigation.is_none());
    app.select_cell(200, 1, false);
    turn_page(&mut app, &ctx, false);
    assert_eq!(offset(&app), 100);
    assert_eq!(visible_result(&app).rows.len(), 300);
    assert_eq!(app.database.document.inspect, Some((100, 1)));
    turn_page(&mut app, &ctx, true);
    assert_eq!(offset(&app), 0);
    assert_retained(&app, 0, 0, Value::Integer(1));
    app.stage_cell(0, 1, Value::Integer(999));
    app.request_page(false);
    assert!(app.database.document.tools.pages.pending.is_none());
    assert!(app.has_drafts());
    app.database.document.drafts = None;
    app.request_page(false);
    execute_and_wait(&mut app, &ctx, "SELECT 987 AS answer");
    assert_eq!(visible_result(&app).rows, [[Value::Integer(987)]]);
    browse_and_wait(&mut app, &ctx, "sample");
    for _ in 0..5 {
        turn_page(&mut app, &ctx, false);
    }
    assert_eq!(app.database.document.tools.pages.end, Some(450));
    assert_eq!(
        visible_result(&app).rows.last().unwrap()[0],
        Value::Integer(450)
    );
    let rows = visible_result(&app).rows.clone();
    app.request_page(false);
    assert!(app.database.document.tools.pages.pending.is_none());
    assert_eq!(visible_result(&app).rows, rows);
    assert!(app.add_filter(id_filter(FilterOp::Equal, 1)));
    wait(&mut app, &ctx);
    assert_eq!(
        visible_result(&app).rows,
        [[Value::Integer(1), Value::Integer(1)]],
        "filter queries the entire table, including evicted rows"
    );
    assert_eq!(offset(&app), 0);
    app.close();
}

#[test]
fn blob_cells_paint_cached_summaries_within_the_frame_budget() {
    let fill = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100) \
                INSERT INTO b SELECT x, randomblob(200), zeroblob(x) FROM n";
    let create = "CREATE TABLE b(id INTEGER PRIMARY KEY, data BLOB, zeros BLOB)";
    let (_dir, ctx, mut app) = fixture(&[create, fill], false);
    browse_and_wait(&mut app, &ctx, "b");
    let output = settle(&mut app, &ctx, DESKTOP);
    let shown: Vec<_> = texts(&output).map(|t| t.galley.job.text.clone()).collect();
    assert!(
        shown.iter().any(|t| t.starts_with("200 bytes")),
        "{shown:?}"
    );
    assert!(!shown.iter().any(|t| t.contains("[blob:")), "{shown:?}");
    let made = || super::value::SUMMARIES_MADE.with(std::cell::Cell::get);
    let before = made();
    let start = Instant::now();
    for _ in 0..10 {
        draw(&mut app, &ctx);
    }
    assert_eq!(
        made(),
        before,
        "painting must reuse summaries, not sniff values"
    );
    let mean = start.elapsed() / 10;
    assert!(mean < Duration::from_millis(500), "{mean:?} per frame");
    app.close();
}

#[test]
fn record_prioritizes_values_and_reads_related_rows_only_on_request() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT COLLATE NOCASE,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a TEXT,b TEXT,payload TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'Alpha')",
            "INSERT INTO child VALUES(1,'007','alpha','child-only')",
        ],
        false,
    );
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    browse_and_wait(&mut app, &ctx, "child");
    let inspection = app.database.document.tools.inspection.as_ref().unwrap();
    let child = inspection.foreign_keys[0].clone();
    let before = held(visible_result(&app));
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    wait(&mut app, &ctx);
    let submitted = app.database.sequence;
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    assert_eq!(
        app.database.sequence, submitted,
        "cache hits must not submit another request"
    );
    assert_eq!(held(visible_result(&app)), before);
    browse_and_wait(&mut app, &ctx, "parent");
    draw(&mut app, &ctx);
    app.select_cell(0, 0, false);
    let before = held(visible_result(&app));
    ctx.enable_accesskit();
    let sequence = app.database.sequence;
    let line = super::test_ui::render(&ctx, egui::vec2(396.0, 30.0), vec![], |ui| {
        app.related_list(ui, 0)
    });
    assert!(control(&line, "Count related child (FK 0)").height() <= 28.0);
    assert_eq!(
        app.database.sequence, sequence,
        "rendering related counts must not read the database"
    );
    app.request_context(0, "child", &child, context::ReadKind::Count);
    wait(&mut app, &ctx);
    app.open_record_window(0, 0);
    app.expand_pane(tiling::Kind::Record);
    let sequence = app.database.sequence;
    for size in [LAPTOP, WIDE] {
        let output = settle(&mut app, &ctx, size);
        // No heading: the first field starts the tile body, and
        // Relationships is one line at its foot.
        assert!(!texts(&output).any(|text| text.galley.job.text == "Fields & values"));
        let relationships = control(&output, "Relationships");
        let value = control(&output, "a · INTEGER · 7");
        assert!(value.bottom() < relationships.top());
        assert_eq!(relationships.height(), 24.0);
        let shown: Vec<_> = texts(&output).map(|t| t.galley.job.text.as_str()).collect();
        assert!(shown.contains(&"0 parents · 1 referring"), "{shown:?}");
        assert!(value.width() > pane(&app, tiling::Kind::Record).width() * 0.9);
        assert_eq!(
            app.database.sequence, sequence,
            "width must not trigger related reads"
        );
    }
    draw(&mut app, &ctx);
    for (label, deferred) in [("Relationships", true), ("Preview related rows", false)] {
        click_control(&mut app, &ctx, label);
        for _ in 0..3 {
            draw(&mut app, &ctx);
        }
        // Opening key navigation still leaves row reads deferred.
        assert_eq!(app.database.sequence == sequence, deferred, "{label}");
    }
    wait(&mut app, &ctx);
    let output = draw(&mut app, &ctx);
    let shown: Vec<_> = texts(&output).map(|t| t.galley.job.text.as_str()).collect();
    assert!(shown.iter().any(|t| t.contains("child-only")), "{shown:?}");
    assert_eq!(held(visible_result(&app)), before);
    assert_eq!(app.database.document.inspect, Some((0, 0)));
    app.request_context(0, "child", &child, context::ReadKind::Count);
    execute_and_wait(&mut app, &ctx, "SELECT 654");
    assert_eq!(visible_result(&app).rows, [[Value::Integer(654)]]);
    app.close();
}

#[test]
fn grid_keys_extend_copy_stage_inline_and_enter_filter_tokens_without_writing() {
    let (_dir, ctx, mut app) = browsing(
        "sample",
        "id INTEGER PRIMARY KEY,n INTEGER,text TEXT",
        "(1,9007199254740993,''),(2,9007199254740995,'two')",
        true,
    );
    settle(&mut app, &ctx, DESKTOP);
    let pos = app.first_cell_rect.unwrap().center();
    click(&mut app, &ctx, pos);
    let send = |app: &mut App, key, mods, typed: Option<&str>| {
        let mut events = vec![egui::Event::ModifiersChanged(mods)];
        events.extend(typed.map(|text| egui::Event::Text(text.into())));
        events.push(key_event(key, true, mods));
        frame_at(app, &ctx, LAPTOP, events)
    };
    use egui::{Key, Modifiers};
    send(&mut app, Key::ArrowRight, Modifiers::SHIFT, None);
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 0), (0, 1)]
    );
    let output = send(&mut app, Key::C, Modifiers::CTRL, None);
    let copied = egui::OutputCommand::CopyText("1\t9007199254740993".into());
    assert!(output.platform_output.commands.contains(&copied));
    send(&mut app, Key::ArrowRight, Modifiers::NONE, None);
    send(&mut app, Key::Enter, Modifiers::NONE, None);
    assert!(app.database.document.tools.inline.is_some());
    send(&mut app, Key::Enter, Modifiers::NONE, Some("draft"));
    assert!(app.database.document.tools.inline.is_none());
    assert!(app.has_drafts());
    assert_eq!(visible_result(&app).rows[0][2], Value::Text(String::new()));
    // The editable record inspector uses the same focus/submission policy as the grid.
    app.focus_pane(tiling::Kind::Record);
    app.start_inline(0, 1);
    for (events, input, accepted) in [
        (vec![], None, false),
        (
            vec![key_event(Key::Enter, true, Modifiers::NONE)],
            Some("invalid"),
            false,
        ),
        (
            vec![key_event(Key::Enter, true, Modifiers::NONE)],
            Some("123"),
            true,
        ),
    ] {
        if let Some(input) = input {
            let inline = app.database.document.tools.inline.as_mut().unwrap();
            inline.editor.input = input.into();
        }
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.record_inspector(ui, 0));
        output.textures_delta.clear();
        assert_eq!(app.database.document.tools.inline.is_none(), accepted);
    }
    assert_staged(&app, 0, 1, Value::Integer(123));
    assert_retained(&app, 0, 1, Value::Integer(9007199254740993));
    app.start_inline(0, 1);
    tap(&mut app, &ctx, Key::Escape);
    assert!(app.database.document.tools.inline.is_none());
    assert_eq!(select_range(&mut app, (0, 1), (1, 1)), vec![(0, 1), (1, 1)]);
    let summary = app.selection_summary();
    assert!(summary.contains("18014398509481988"), "{summary}");
    app.database.document.tools.columns.move_before(2, 1);
    assert_eq!(
        select_range(&mut app, (0, 2), (1, 1)),
        vec![(0, 2), (0, 1), (1, 2), (1, 1)]
    );
    // Enter keeps a typed filter token, and Backspace removes it again.
    app.database.document.drafts = None;
    app.database.document.tools.filter_focus = true;
    draw(&mut app, &ctx);
    let typed = egui::Event::Text("n>=9007199254740994".into());
    frame(
        &mut app,
        &ctx,
        vec![typed, key_event(Key::Enter, true, Modifiers::NONE)],
    );
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows.len(), 1);
    assert_retained(&app, 0, 0, Value::Integer(2));
    let browse = app.database.document.browse.as_ref().unwrap();
    assert_eq!(browse.filters[0].op, FilterOp::GreaterOrEqual);
    app.database.document.tools.filter_focus = true;
    draw(&mut app, &ctx);
    tap(&mut app, &ctx, Key::Backspace);
    wait(&mut app, &ctx);
    draw(&mut app, &ctx);
    let browse = app.database.document.browse.as_ref().unwrap();
    assert!(browse.filters.is_empty());
    assert_eq!(visible_result(&app).rows.len(), 2);
    // Copying keeps empty text fields distinct from missing ones.
    app.database.document.tools.columns.move_before(2, 0);
    select_range(&mut app, (0, 2), (0, 0));
    assert_eq!(app.selection_tsv().unwrap(), "\"\"\t1");
    app.close();
}

#[test]
fn clipboard_grid_and_record_share_live_rows_without_overlays() {
    let (_dir, ctx, mut app) = project("clipboard", false);
    browse_and_wait(&mut app, &ctx, "entries");
    let full_hd = egui::vec2(1920.0, 1080.0);
    for size in [LAPTOP, full_hd] {
        settle(&mut app, &ctx, size);
        assert!(!app.database.document.ui.inspector_open);
    }
    let columns = visible_result(&app).columns.len();
    assert_eq!(columns, 11);
    let data_id = app.database.document.id;
    app.expand_window(data_id);
    settle(&mut app, &ctx, full_hd);
    let spacing = ctx.style_of(egui::Theme::Dark).spacing.item_spacing.x;
    let widths = &app.database.document.tools.columns.widths;
    let width = widths.iter().sum::<f32>() + 44.0 + columns as f32 * spacing;
    let ui = &app.database.ui;
    let grid = ui.grid_rect.width();
    assert!(width <= grid + 1.0, "columns {width} > grid {grid}");
    assert!(ui.visible_rows >= 30, "{} rows", ui.visible_rows);
    app.open_record_window(0, 0);
    assert_eq!(app.database.workspace.active, tiling::Kind::Record);
    assert_eq!(app.database.document.id, data_id);
    settle(&mut app, &ctx, egui::vec2(2560.0, 1440.0));
    assert!(!app.database.document.ui.inspector_open);
    app.close();
}

#[test]
fn filtered_index_tree_opens_original_ddl_without_replacing_primary_rows() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE INDEX sample_value ON sample(value)",
            "INSERT INTO sample VALUES(1,'one')",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "sample");
    app.sidebar_search.query = "sample_value".into();
    let output = settle(&mut app, &ctx, LAPTOP);
    let sidebar = app.database.ui.sidebar_rect;
    let pos = texts(&output)
        .rev()
        .find(|t| t.galley.job.text == "sample_value · index" && sidebar.contains(t.pos))
        .map(|t| t.pos + egui::vec2(8.0, 6.0))
        .expect("filtered index row");
    let data_id = app.database.document.id;
    let before = held(visible_result(&app));
    click(&mut app, &ctx, pos);
    let ddl = app.database.document.tools.ddl.as_ref().unwrap();
    assert_eq!(ddl.name, "sample_value");
    let structure = shell::TableView::Structure;
    assert_eq!(app.database.document.ui.table_view, structure);
    app.refresh_table();
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.ui.table_view, structure);
    assert_eq!(held(&app.document_ref(data_id).unwrap().result), before);
    app.close();
}

/// Press at `start`, move by `delta` and release there, with settling frames.
fn drag_by(app: &mut App, ctx: &egui::Context, start: egui::Pos2, delta: egui::Vec2) {
    let press = vec![
        egui::Event::PointerMoved(start),
        pointer_button(start, true),
    ];
    frame(app, ctx, press);
    frame(app, ctx, vec![egui::Event::PointerMoved(start + delta)]);
    draw(app, ctx);
    frame(app, ctx, vec![pointer_button(start + delta, false)]);
    draw(app, ctx);
}
fn handles(app: &App) -> Vec<(tiling::Divider, egui::Rect)> {
    let workspace = &app.database.workspace;
    let frame = app.workspace_frame();
    tiling::dividers(
        &frame,
        &workspace.layout,
        &workspace.minimized,
        workspace.area,
    )
}
/// Dragging `divider` by `distance` moved only the edges beside it: a column
/// divider widens the first column's two tiles and narrows the rest; a row
/// divider moves the bottom of the tile above and the top of the one below.
fn assert_resized(
    divider: tiling::Divider,
    before: &[(tiling::Kind, egui::Rect)],
    after: &[(tiling::Kind, egui::Rect)],
    distance: f32,
) {
    let near = |a: f32, b: f32| (a - b).abs() < 0.1;
    for (i, ((kind, old), (_, new))) in before.iter().zip(after).enumerate() {
        match divider {
            tiling::Divider::Columns(_) => {
                let expected = if i < 2 { distance } else { -distance };
                assert!(near(new.width() - old.width(), expected));
                assert!(near(new.height(), old.height()));
            }
            tiling::Divider::Rows(above, _) if *kind == above => {
                assert!(near(new.bottom() - old.bottom(), distance));
            }
            tiling::Divider::Rows(_, below) if *kind == below => {
                assert!(near(new.top() - old.top(), distance));
            }
            tiling::Divider::Rows(..) => {
                assert!((new.min - old.min).length() < 0.1);
                assert!((new.max - old.max).length() < 0.1);
            }
        }
    }
}
#[test]
fn full_length_workspace_dividers_resize_adjacent_tiles_and_preserve_live_state() {
    let (_dir, ctx, mut app) = browsing(
        "sample",
        "id INTEGER PRIMARY KEY, body TEXT",
        "(1,'retained')",
        true,
    );
    app.select_cell(0, 1, false);
    app.stage_cell(0, 1, text("draft"));
    settle(&mut app, &ctx, DESKTOP);
    let ticket = visible_result(&app).current;
    let sequence = app.database.sequence;
    let focus = app.database.workspace.focused;
    use tiling::Kind::{Data, Map, Record, Sql, Structure};
    for divider in [
        tiling::Divider::Columns(0),
        tiling::Divider::Rows(Map, Data),
        tiling::Divider::Rows(Record, Structure),
        tiling::Divider::Rows(Structure, Sql),
    ] {
        for (fraction, distance) in [(0.08, 30.0), (0.92, -20.0)] {
            // The assistant is hidden, so its column is a line and five tiles show.
            let before = app.database.workspace.pane_rects.clone();
            assert_eq!(before.len(), 5);
            let rect = handles(&app)
                .into_iter()
                .find(|(d, _)| *d == divider)
                .unwrap()
                .1;
            let original_layout = app.database.workspace.layout.clone();
            click(&mut app, &ctx, rect.center());
            assert_eq!(
                app.database.workspace.layout, original_layout,
                "clicking without dragging must not change proportions"
            );
            // Anywhere along a divider's length drags it.
            let (start, delta) = if divider.vertical() {
                let y = rect.top() + rect.height() * fraction;
                (egui::pos2(rect.center().x, y), egui::vec2(distance, 0.0))
            } else {
                let x = rect.left() + rect.width() * fraction;
                (egui::pos2(x, rect.center().y), egui::vec2(0.0, distance))
            };
            drag_by(&mut app, &ctx, start, delta);
            assert_resized(
                divider,
                &before,
                &app.database.workspace.pane_rects,
                distance,
            );
        }
    }
    for (distance, column) in [(10000.0, 1), (-10000.0, 0)] {
        let handle = handles(&app)[0].1;
        drag_by(&mut app, &ctx, handle.center(), egui::vec2(distance, 0.0));
        // Dragging far into a column folds it to a line; restoring it
        // keeps the clamped split.
        let workspace = &mut app.database.workspace;
        assert!(workspace.layout.all_minimized(column, &workspace.minimized));
        for kind in &workspace.layout.columns[column].tiles {
            workspace.minimized.remove(kind);
        }
        draw(&mut app, &ctx);
        let rects = &app.database.workspace.pane_rects;
        assert!(rects.iter().all(|(_, rect)| rect.width() >= 179.9));
        let handle = handles(&app)[1].1;
        drag_by(&mut app, &ctx, handle.center(), egui::vec2(0.0, distance));
        let rects = &app.database.workspace.pane_rects;
        assert!(rects.iter().all(|(_, rect)| rect.height() >= 95.9));
    }
    let layout = app.database.workspace.layout.clone();
    for fraction in [0.08, 0.92] {
        let before = app.database.ui.sidebar_rect;
        let pos = egui::pos2(before.right(), before.top() + before.height() * fraction);
        drag_by(&mut app, &ctx, pos, egui::vec2(30.0, 0.0));
        assert!((app.database.ui.sidebar_rect.width() - before.width() - 30.0).abs() < 1.0);
        assert_eq!(app.database.workspace.layout, layout);
    }
    assert_eq!(visible_result(&app).current, ticket);
    assert_retained(&app, 0, 1, text("retained"));
    assert_staged(&app, 0, 1, text("draft"));
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    assert_eq!(app.database.workspace.focused, focus);
    assert_eq!(
        app.database.sequence, sequence,
        "resizing must not fetch or execute SQL"
    );
    assert_eq!(
        app.preferences.workspace_layouts.first(),
        app.workspace_snapshot().as_ref()
    );
    app.close();
}

#[test]
fn workspace_geometry_roundtrips_without_query_data_or_bindings() {
    let (_dir, ctx, mut app) = browsing(
        "chunks",
        "id TEXT PRIMARY KEY, part INTEGER, body BLOB",
        "('buffer-1780913432386559807-118',0,x'001122')",
        false,
    );
    let data_id = app.database.document.id;
    app.expand_window(data_id);
    frame_at(&mut app, &ctx, WIDE, vec![]);
    let widths = app.database.document.tools.columns.widths.clone();
    assert!(widths.iter().all(|w| *w <= 300.0));
    assert!(widths[1] < 100.0);
    frame_at(&mut app, &ctx, LAPTOP, vec![]);
    assert_eq!(widths, app.database.document.tools.columns.widths);
    app.open_record_window(0, 0);
    assert_eq!(
        app.database.document.id, data_id,
        "record shares the live table buffer"
    );
    assert!(visible_result(&app).current.is_some());
    app.focus_window(data_id);
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.open_sql_window(false);
    let editor = app.database.document.id;
    app.database.document.sql = "SELECT 'private-literal'".into();
    app.database.document.params = "[\"private-binding\"]".into();
    app.retile();
    app.minimize_pane(tiling::Kind::Map);
    app.expand_window(editor);
    let splits = tiling::Splits {
        column: 0.48,
        rows: [1.0, 2.0, 1.5, 0.8, 1.0],
        sql_editor: 0.6,
    };
    app.database.workspace.layout = splits.into();
    // A moved tile and a new column are part of what is saved.
    let layout = &mut app.database.workspace.layout;
    assert!(layout.move_tile(tiling::Kind::Record, tiling::Target::NewColumn));
    let arranged = layout.clone();
    let snapshot = app.workspace_snapshot().unwrap();
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("private") && !encoded.contains("buffer-"));
    let parsed = || serde_json::from_str::<serde_json::Value>(&encoded).unwrap();
    // Layouts saved before tiles could move carry no arrangement, or only the
    // two-column splits; both open in the default arrangement.
    let mut legacy = parsed();
    legacy.as_object_mut().unwrap().remove("layout");
    app.preferences.workspace_layouts = vec![serde_json::from_value(legacy.clone()).unwrap()];
    assert!(app.preferences.workspace_layouts[0].valid());
    app.restore_workspace();
    assert_eq!(app.database.workspace.layout, tiling::Layout::default());
    legacy["splits"] =
        serde_json::json!({"column": 0.48, "rows": [1.0, 2.0, 1.5, 0.8, 1.0], "sql_editor": 0.6});
    app.preferences.workspace_layouts = vec![serde_json::from_value(legacy).unwrap()];
    assert!(app.preferences.workspace_layouts[0].valid());
    app.restore_workspace();
    assert_eq!(app.database.workspace.layout, tiling::Layout::from(splits));
    for (key, invalid) in [
        ("splits", serde_json::json!({"column": 0.0})),
        (
            "splits",
            serde_json::json!({"rows": [1.0, 1.0, 1.0, 0.0, 1.0]}),
        ),
        // Every tile must sit in exactly one column, and no column is empty.
        (
            "layout",
            serde_json::json!({"columns": [], "rows": {}, "sql_editor": 0.5}),
        ),
        (
            "layout",
            serde_json::json!({"columns": [{"tiles": ["Map", "Map", "Data", "Record", "Structure", "Sql"], "width": 1.0}], "rows": {}, "sql_editor": 0.5}),
        ),
    ] {
        let mut value = parsed();
        value[key] = invalid;
        app.preferences.workspace_layouts = vec![serde_json::from_value(value).unwrap()];
        assert!(!app.preferences.workspace_layouts[0].valid());
    }
    app.preferences.workspace_layouts = vec![serde_json::from_str(&encoded).unwrap()];
    app.restore_workspace();
    assert_eq!(app.workspace_snapshot(), Some(snapshot));
    assert_eq!(app.database.workspace.layout, arranged);
    assert_eq!(app.database.document.id, editor);
    assert!(app.database.document.sql.is_empty());
    assert_eq!(app.database.document.params, "[]");
    assert_eq!(app.database.workspace.expanded, Some(tiling::Kind::Sql));
    wait_until(&mut app, &ctx, |a| {
        let mut documents = a.database.documents.values();
        !a.executing() && documents.all(|d| !d.needs_load && d.tools.inspection_ticket.is_none())
    });
    assert_eq!(
        app.database.document.id, editor,
        "background reload preserves focus"
    );
    assert_eq!(app.document_ref(data_id).unwrap().result.rows.len(), 1);
    let mut invalid = parsed();
    invalid["focused"] = serde_json::json!(9999);
    let layouts = &mut app.preferences.workspace_layouts;
    layouts.push(serde_json::from_value(invalid).unwrap());
    assert!(!layouts.last().unwrap().valid());
    app.preferences.normalize();
    assert_eq!(app.preferences.workspace_layouts.len(), 1);
    close_and_wait(&mut app, &ctx);
    app.open(&ctx);
    wait_until(&mut app, &ctx, |a| a.database.ready);
    assert_eq!(
        app.database.document.id, editor,
        "Opened must keep the restored focus before the first UI frame"
    );
    assert_eq!(app.database.workspace.expanded, Some(tiling::Kind::Sql));
    app.close();
}

#[test]
fn automatic_map_fit_keeps_every_node_inside_its_resized_tile() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "CREATE TABLE c(id INTEGER PRIMARY KEY)",
        ],
        false,
    );
    let map = app.database.document.id;
    draw(&mut app, &ctx);
    browse_and_wait(&mut app, &ctx, "a");
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.retile();
    for expanded in [false, true, false] {
        if expanded != app.database.workspace.expanded.is_some() {
            app.expand_window(map);
        }
        settle(&mut app, &ctx, egui::vec2(1800.0, 1000.0));
        let tiles = &app.database.workspace.tiles;
        let tile = tiles.iter().find(|(id, _)| *id == map).unwrap().1;
        let nodes = &app.database.ui.diagram.table_rects;
        assert_eq!(nodes.len(), 3);
        for (_, rect) in nodes {
            assert!(
                tile.contains_rect(*rect),
                "node {rect:?} outside tile {tile:?}"
            );
        }
    }
    app.close();
}

#[test]
fn record_controls_move_rows_and_hidden_fields_without_fetching_or_losing_drafts() {
    let columns = (0..12)
        .map(|i| format!("field_{i} INTEGER"))
        .collect::<Vec<_>>()
        .join(",");
    let values = (0..12).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let (_dir, ctx, mut app) = accessible(
        &[
            &format!("CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT,{columns})"),
            &format!(
                "INSERT INTO sample VALUES(1,'first',{values}),(2,'second',{values}),(3,'third',{values})"
            ),
        ],
        "sample",
    );
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, text("unsaved"));
    app.expand_pane(tiling::Kind::Record);
    let sequence = app.database.sequence;
    let ticket = visible_result(&app).current;
    for (label, row, value) in [
        ("Next record", 1, "second"),
        ("Next record", 2, "third"),
        ("Previous record", 1, "second"),
    ] {
        let output = settle(&mut app, &ctx, DESKTOP);
        let bounds = control(&output, label);
        assert!(control(&output, "Record position").width() < 90.0);
        click(&mut app, &ctx, bounds.center());
        assert_eq!(app.database.document.inspect, Some((row, 1)));
        assert_eq!(app.database.document.tools.detail, Some(row));
        assert_eq!(app.database.document.tools.cell_editor.input, value);
        assert!(app.database.document.has_drafts());
        assert_eq!(visible_result(&app).current, ticket);
        assert_eq!(
            app.database.sequence, sequence,
            "stepper must not fetch more rows"
        );
    }
    click_control(&mut app, &ctx, "Record position");
    press_key(&mut app, &ctx, egui::Key::A, egui::Modifiers::CTRL);
    frame(&mut app, &ctx, vec![egui::Event::Text("1".into())]);
    tap(&mut app, &ctx, egui::Key::Enter);
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    assert_eq!(app.database.sequence, sequence);
    app.open_record_window(1, 0);
    draw(&mut app, &ctx);
    app.stage_cell(1, 2, Value::Integer(999));
    app.database.document.tools.columns.hidden.insert(6);
    let ticket = visible_result(&app).current;
    for _ in 0..6 {
        tap(&mut app, &ctx, egui::Key::ArrowDown);
    }
    assert_eq!(app.database.document.inspect, Some((1, 6)));
    assert_eq!(visible_result(&app).current, ticket);
    assert!(app.database.document.has_drafts());
    assert!(app.database.document.tools.columns.hidden.contains(&6));
    assert_eq!(app.database.document.tools.cell_editor.input, "4");
    assert!(
        app.database.document.tools.record_scroll_to.is_none(),
        "record consumed the scroll request"
    );
    tap(&mut app, &ctx, egui::Key::Enter);
    assert!(app.database.document.tools.inline.is_some());
    draw(&mut app, &ctx);
    let inline = app.database.document.tools.inline.as_mut().unwrap();
    inline.editor.kind = super::results::CellKind::Null;
    tap(&mut app, &ctx, egui::Key::Enter);
    assert!(app.database.document.tools.inline.is_none());
    assert_staged(&app, 1, 6, Value::Null);
    assert_retained(&app, 1, 6, Value::Integer(4));
    assert_eq!(app.database.sequence, sequence);
    app.close();
}

#[test]
fn table_header_selector_browses_tables_and_views_and_protects_drafts() {
    let (_dir, ctx, mut app) = accessible(
        &[
            "CREATE TABLE first_table(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO first_table VALUES(1, 'first')",
            "CREATE TABLE second_table(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO second_table VALUES(2, 'second')",
            "CREATE VIEW table_view AS SELECT * FROM second_table",
        ],
        "first_table",
    );
    let first_id = app.database.document.id;
    let choose = |app: &mut App, name: &str| {
        for label in ["Choose table", name] {
            let output = settle(app, &ctx, DESKTOP);
            let bounds = control_from(&output, label, app.database.workspace.area.left());
            click(app, &ctx, bounds.center());
        }
        wait(app, &ctx);
    };
    for name in ["second_table", "table_view", "first_table"] {
        choose(&mut app, name);
        assert_eq!(app.database.document.object.as_deref(), Some(name));
        assert_eq!(visible_result(&app).rows.len(), 1);
        assert_eq!(
            app.pane_document(tiling::Kind::Data),
            Some(app.database.document.id)
        );
    }
    assert_eq!(app.database.document.id, first_id);
    app.stage_cell(0, 1, text("unsaved"));
    choose(&mut app, "second_table");
    assert_eq!(app.database.document.id, first_id);
    assert!(app.database.document.has_drafts());
    assert!(app.database.status.contains("Commit or discard drafts"));
    app.close();
}

#[test]
fn fixed_panes_preserve_drafts_selection_and_results_through_layout_changes() {
    use tiling::Kind::{Data, Map, Record};
    let (_dir, ctx, mut app) = browsing(
        "sample",
        "id INTEGER PRIMARY KEY, body TEXT",
        "(1,'kept')",
        true,
    );
    let data = app.database.document.id;
    let ticket = visible_result(&app).current;
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, text("unsaved"));
    app.database.document.filter = "body = 'unfinished'".into();
    settle(&mut app, &ctx, DESKTOP);
    let controls = &app.database.workspace.controls;
    let (_, minimize, _) = *controls.iter().find(|(k, ..)| *k == Map).unwrap();
    click(&mut app, &ctx, minimize.center());
    let map_minimized = |app: &App| app.database.workspace.minimized.contains(&Map);
    assert!(map_minimized(&app));
    draw(&mut app, &ctx);
    let map = pane(&app, Map);
    assert!((map.height() - tiling::BAR).abs() < 0.1);
    click(&mut app, &ctx, map.left_center() + egui::vec2(70.0, 0.0));
    assert!(!map_minimized(&app));
    for kind in tiling::Kind::PANES {
        app.minimize_pane(kind);
    }
    app.expand_pane(Record);
    draw(&mut app, &ctx);
    assert_eq!(app.database.workspace.pane_rects.len(), 1);
    assert_eq!(app.database.document.id, data);
    assert!(app.database.document.has_drafts());
    app.expand_pane(Record);
    // Everything but the record stays minimized, the hidden assistant included.
    assert_eq!(app.database.workspace.minimized.len(), 5);
    app.retile();
    draw(&mut app, &ctx);
    assert_eq!(app.database.workspace.pane_rects.len(), 5);
    assert_eq!(visible_result(&app).current, ticket);
    assert_retained(&app, 0, 1, text("kept"));
    assert_eq!(app.database.document.filter, "body = 'unfinished'");
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    let output = draw(&mut app, &ctx);
    let data_rect = pane(&app, Data);
    let commit = texts(&output)
        .find(|t| t.galley.job.text == "Commit" && data_rect.contains(t.pos))
        .map(|t| t.pos + egui::vec2(8.0, 6.0))
        .expect("direct commit action in data dock");
    assert!(!app.database.document.show_preview);
    click(&mut app, &ctx, commit);
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_retained(&app, 0, 1, text("unsaved"));
    app.close();
}

#[test]
fn run_all_command_bar_icon_preserves_sequential_and_atomic_script_policies() {
    for (atomic, policy, expected) in [
        (false, "Run statements · stop on error", 1),
        (true, "Run as one atomic transaction", 0),
    ] {
        let (_dir, ctx, mut app) =
            fixture(&["CREATE TABLE script_test(id INTEGER PRIMARY KEY)"], true);
        app.open_sql_window(false);
        let editor = app.database.document.id;
        let script = "INSERT INTO script_test VALUES(1) RETURNING id; INSERT INTO script_test VALUES(1); INSERT INTO script_test VALUES(2);";
        app.database.document.sql = script.into();
        // Run all ignores a selection; Run retains the selection/cursor behavior.
        app.database.document.tools.selection = 0.."INSERT INTO script_test VALUES(1);".len();
        app.database.workspace.expanded = Some(tiling::Kind::Sql);
        ctx.enable_accesskit();
        let output = settle(&mut app, &ctx, DESKTOP);
        assert!(!label_exists(
            &output,
            "Run selection / statement at cursor"
        ));
        let before = app.database.sequence;
        click(&mut app, &ctx, control(&output, "Run all").center());
        assert_eq!(
            app.database.sequence, before,
            "choosing a policy must precede execution"
        );
        click_control(&mut app, &ctx, policy);
        wait(&mut app, &ctx);
        assert_eq!(app.database.document.script_results.len(), 2);
        assert!(errored(visible_result(&app)));
        if atomic {
            assert!(app.database.document.script_message.contains("rolled back"));
        }
        // Selecting an outcome preserves every statement's rows without
        // copying a second payload into the visible result.
        for i in [0, 1, 0, 0] {
            app.database.document.select_script_result(i);
            let result = visible_result(&app);
            let returned = vec![vec![Value::Integer(1)]];
            assert_eq!(result.rows, if i == 0 { returned } else { vec![] });
            assert_eq!(errored(result), i == 1);
        }
        assert_eq!(app.document_ref(editor).unwrap().sql, script);
        execute_and_wait(&mut app, &ctx, "SELECT count(*) FROM script_test");
        assert_eq!(visible_result(&app).rows, [[Value::Integer(expected)]]);
        close_and_wait(&mut app, &ctx);
    }
}

#[test]
fn every_tile_exposes_its_actions_and_relationship_zoom_steps_and_resets() {
    let (_dir, ctx, mut app) = browsing(
        "sample",
        "id INTEGER PRIMARY KEY, body TEXT",
        "(1,'kept')",
        false,
    );
    ctx.enable_accesskit();
    // The zoom buttons change the map's scale; its percentage resets it.
    app.expand_pane(tiling::Kind::Map);
    for (label, zoom, shown) in [
        ("Reset zoom to 100%", 1.0, "100%"),
        ("Zoom in", 1.25, "125%"),
        ("Zoom out", 1.0, "100%"),
        ("Zoom out", 0.8, "80%"),
        ("Reset zoom to 100%", 1.0, "100%"),
    ] {
        let output = settle(&mut app, &ctx, DESKTOP);
        click(&mut app, &ctx, control(&output, label).center());
        assert_eq!(app.database.ui.diagram.camera.zoom, zoom, "{label}");
        let output = draw(&mut app, &ctx);
        assert!(
            texts(&output).any(|t| t.galley.job.text == shown),
            "{shown}"
        );
    }
    let cases: &[(tiling::Kind, &[&str])] = &[
        (
            tiling::Kind::Map,
            &[
                "Fit relationships",
                "Arrange nodes",
                "Columns",
                "Find",
                "Refresh schema",
                "Toggle system tables",
                "Reset zoom to 100%",
                "Zoom out",
                "Zoom in",
            ],
        ),
        (
            tiling::Kind::Data,
            &[
                "Refresh",
                "Filter",
                "Fit columns",
                "Export",
                "Inspect selected record",
                "Show all columns",
                "Copy retained rows",
                "Count table rows (full scan)",
                "New row (staged)",
                "Visible columns",
            ],
        ),
        (
            tiling::Kind::Record,
            &[
                "Copy record",
                "Clone record",
                "Stage deletion",
                "Copy selected value",
            ],
        ),
        (tiling::Kind::Structure, &["Copy DDL", "Refresh"]),
        (
            tiling::Kind::Sql,
            &[
                "Run",
                "Run all",
                "Explain query plan",
                "Explain bytecode",
                "Typed parameters",
                "Completions",
                "Save query",
                "New SQL editor",
                "Begin editor transaction",
                "Commit transaction",
                "Roll back transaction",
            ],
        ),
    ];
    for &(kind, expected) in cases {
        app.database.workspace.expanded = Some(kind);
        app.focus_pane(kind);
        // An expanded wide tile should show all commands without opening any popup.
        let output = settle(&mut app, &ctx, WIDE);
        let controls = super::test_ui::nodes(&output)
            .iter()
            .filter_map(|(_, node)| node.label())
            .collect::<Vec<_>>();
        for label in expected.iter().copied().chain([
            "Restore all tiles",
            "Minimize to header",
            "Restore workspace",
        ]) {
            assert!(
                controls.contains(&label),
                "{kind:?} missing direct control: {label}"
            );
        }
        assert!(!controls.contains(&"More actions"));
    }
    app.database.workspace.expanded = None;
    for kind in [
        tiling::Kind::Map,
        tiling::Kind::Structure,
        tiling::Kind::Sql,
    ] {
        app.minimize_pane(kind);
    }
    let output = settle(&mut app, &ctx, WIDE);
    for label in ["Relationships", "SQL"] {
        control(&output, label);
    }
    assert!(super::test_ui::nodes(&output).iter().any(|(_, n)| {
        n.label()
            .is_some_and(|s| s.starts_with("Restore Structure · ") && s.contains("columns"))
    }));
    app.close();
}

#[test]
fn sql_result_ribbon_actions_use_the_linked_result_and_its_divider_shares_the_tile() {
    let (_dir, ctx, mut app) = browsing("sample", "id INTEGER PRIMARY KEY", "(1)", false);
    let table = app.database.document.id;
    app.database.document.tools.columns.hidden.insert(0);
    app.open_sql_window(false);
    let editor = app.database.document.id;
    let sql = "SELECT 42 AS answer, 'retained' AS text";
    app.database.document.sql = sql.into();
    app.run_sql();
    wait(&mut app, &ctx);
    let result = app.database.document.id;
    let ticket = visible_result(&app).current;
    assert_eq!(app.database.document.kind, tiling::Kind::Result);
    app.database.document.inspect = Some((0, 0));
    let columns = &mut app.database.document.tools.columns;
    columns.hidden.insert(0);
    columns.manual.insert(0);
    app.focus_pane(tiling::Kind::Sql);
    app.database.workspace.expanded = Some(tiling::Kind::Sql);
    ctx.enable_accesskit();
    // The line between the SQL and its results drags to share the tile.
    let output = settle(&mut app, &ctx, DESKTOP);
    let start = control(&output, "Resize SQL and results").center();
    let before = app.database.workspace.layout.sql_editor;
    drag_by(&mut app, &ctx, start, egui::vec2(0.0, 120.0));
    let after = app.database.workspace.layout.sql_editor;
    assert!(after > before + 0.05, "{before} -> {after}");
    // The new share is part of the saved layout.
    let saved = serde_json::to_value(app.workspace_snapshot()).unwrap();
    assert_eq!(
        saved["layout"]["sql_editor"].as_f64().unwrap() as f32,
        after
    );
    let press = |app: &mut App, label: &str| {
        frame_at(app, &ctx, WIDE, vec![]);
        let pos = click_control_at(app, &ctx, WIDE, label).center();
        assert!(app.database.workspace.pane_rects[0].1.contains(pos));
    };
    press(&mut app, "Show all result columns");
    let hidden = &app.document_ref(result).unwrap().tools.columns.hidden;
    assert!(hidden.is_empty());
    let hidden = &app.document_ref(table).unwrap().tools.columns.hidden;
    assert!(hidden.contains(&0));
    press(&mut app, "Fit result columns");
    let manual = &app.document_ref(result).unwrap().tools.columns.manual;
    assert!(manual.is_empty());
    press(&mut app, "Export query results");
    assert!(app.database.ui.export_open);
    assert_eq!(app.database.workspace.focused, result);
    assert_eq!(app.database.document.id, result);
    assert_eq!(visible_result(&app).current, ticket);
    assert_eq!(visible_result(&app).rows[0][0], Value::Integer(42));
    assert_eq!(app.document_ref(editor).unwrap().sql, sql);
    app.close();
}

#[test]
fn redesigned_dialog_actions_remain_visible_at_minimum_and_laptop_sizes() {
    let (_dir, ctx, mut app) = accessible(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'stored')",
        ],
        "sample",
    );
    app.stage_cell(0, 1, text("local draft"));
    let sizes = [(720.0, 480.0, 1.0), (1366.0, 768.0, 1.0)];
    across_themes(&mut app, &ctx, &sizes, |app, width, height| {
        let size = egui::vec2(width, height);
        let font_size = if width < 1000.0 { 14 } else { 20 };
        app.preferences.font_size = font_size;
        app.apply_theme(&ctx);
        for (scene, label) in [
            (0, "COMMIT staged edits to database"),
            (1, "Stage insert (no database write)"),
            (2, "Close cleanly and reopen"),
            (3, "Roll back & close"),
            (4, "Export retained result…"),
            (5, "Copy stored value"),
        ] {
            app.database.document.show_preview = scene == 0;
            app.database.document.show_insert = false;
            if scene == 1 {
                app.begin_insert();
            }
            app.database.ui.access_dialog = scene == 2;
            app.database.confirm_close = scene == 3;
            app.database.ui.export_open = scene == 4;
            let document = &mut app.database.document;
            document.ui.inspector_detached = scene == 5;
            document.ui.inspector_open = scene == 5;
            document.ui.inspector = shell::DetailMode::Cell;
            document.inspect = Some((0, 1));
            settle(app, &ctx, size);
            let rect = control(&settle(app, &ctx, size), label);
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                "{label} outside {size:?} with font {font_size}: {rect:?}"
            );
            assert_retained(app, 0, 1, text("stored"));
            assert!(app.has_drafts());
        }
    });
    app.close();
}

#[test]
fn database_workspaces_isolate_transactions_drafts_and_inactive_results() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(1,'first')",
        ],
        true,
    );
    let first = app.database.id;
    let first_path = app.database.path.clone();
    execute_and_wait(&mut app, &ctx, "BEGIN");
    execute_and_wait(&mut app, &ctx, "INSERT INTO t VALUES(2,'pending')");
    assert_eq!(app.database.transaction, TransactionState::Active);
    // Leave a query queued on the first workspace while opening the second.
    app.run_text("SELECT count(*) FROM t".into(), None);
    let first_result = app.database.document.id;
    let second_path = dir.path().join("second.db");
    crate::test_support::create(
        &second_path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(10,'second')",
        ],
    );
    open_database(&mut app, &ctx, second_path, true);
    let second = app.database.id;
    assert_ne!(first, second);
    execute_and_wait(&mut app, &ctx, "SELECT value FROM t");
    assert_eq!(visible_result(&app).rows, [[text("second")]]);
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    // Sidebar rendering must never exchange parked workspaces or retained buffers.
    let output = frame_at(&mut app, &ctx, LAPTOP, vec![]);
    assert_eq!(app.database.id, second);
    let row = control(&output, &format!("Select database: {first_path}"));
    click(&mut app, &ctx, row.center());
    assert_eq!(app.database.id, first);
    app.select_workspace(second);
    let first_state = &app.parked[&first];
    let result = if first_state.document.id == first_result {
        &first_state.document
    } else {
        &first_state.documents[&first_result]
    };
    assert!(result.active.is_none());
    assert_eq!(result.result.rows, [[Value::Integer(2)]]);
    app.select_workspace(first);
    assert_eq!(app.database.transaction, TransactionState::Active);
    browse_and_wait(&mut app, &ctx, "t");
    assert_eq!(
        visible_result(&app).rows.len(),
        1,
        "browse sees committed state"
    );
    app.select_workspace(second);
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, text("draft"));
    assert!(app.has_drafts());
    app.select_workspace(first);
    assert!(!app.has_drafts());
    assert_eq!(app.database.transaction, TransactionState::Active);
    app.select_workspace(second);
    assert!(app.has_drafts());
    // Duplicate aliases focus the existing session instead of opening another.
    let alias = dir.path().join("alias.db");
    std::os::unix::fs::symlink(&first_path, &alias).unwrap();
    app.select_workspace(second);
    app.request_database(&ctx, alias);
    assert_eq!(app.database.id, first);
    assert_eq!(app.parked.len(), 1);
    // Close a background row through the sidebar, preserving the other database's drafts.
    app.select_workspace(second);
    let close = format!("Close database: {first_path}");
    click_control_at(&mut app, &ctx, LAPTOP, &close);
    assert_eq!(app.database.id, first);
    assert!(app.database.confirm_close);
    let session = app.database.session.as_ref().unwrap().id;
    let keep = click_control_at(&mut app, &ctx, LAPTOP, "Keep open");
    assert!(app.database.ui.sidebar_rect.contains_rect(keep));
    assert!(!app.database.confirm_close);
    assert_eq!(app.database.id, second);
    assert!(app.has_drafts());
    app.select_workspace(first);
    assert_eq!(app.database.transaction, TransactionState::Active);
    app.request_close();
    assert!(app.database.confirm_close);
    tap(&mut app, &ctx, egui::Key::Escape);
    assert!(!app.database.closing && !app.database.confirm_close);
    assert_eq!(app.database.session.as_ref().unwrap().id, session);
    // Confirming inline rolls back only the selected session.
    app.request_close();
    let confirm = click_control_at(&mut app, &ctx, LAPTOP, "Roll back & close");
    assert!(app.database.ui.sidebar_rect.contains_rect(confirm));
    wait_closed(&mut app, &ctx);
    app.select_workspace(second);
    assert!(app.has_drafts());
    app.database.document.drafts = None;
    execute_and_wait(&mut app, &ctx, "SELECT value FROM t");
    assert_eq!(visible_result(&app).rows, [[text("second")]]);
    draw(&mut app, &ctx);
    close_and_wait(&mut app, &ctx);
    // Closed background workspaces must not steal the global header inspection.
    app.select_workspace(first);
    app.databases.inspection.recheck();
    wait_until(&mut app, &ctx, |a| {
        a.databases.inspection.ready(&a.database.path)
    });
    app.open(&ctx);
    wait(&mut app, &ctx);
    execute_and_wait(&mut app, &ctx, "SELECT count(*) FROM t");
    assert_eq!(app.database.document.result.rows, [[Value::Integer(1)]]);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn schema_comparison_routes_inactive_snapshots_guards_target_drafts_and_quit_settles_all() {
    let (dir, ctx, mut app) = fixture(
        &["CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)"],
        false,
    );
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let source = app.database.id;
    let target_path = dir.path().join("target.db");
    crate::test_support::create(
        &target_path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY)",
            "INSERT INTO t VALUES(1)",
        ],
    );
    open_database(&mut app, &ctx, target_path, true);
    let target = app.database.id;
    app.comparison.source = source;
    app.comparison.target = target;
    ctx.enable_accesskit();
    tap(&mut app, &ctx, egui::Key::F7);
    click_control(&mut app, &ctx, "Capture / refresh both schemas");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.comparison_ready() {
        app.poll(&ctx);
        app.poll_comparison(&ctx);
        assert!(
            Instant::now() < deadline,
            "snapshot comparison did not settle"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    draw(&mut app, &ctx);
    assert!(app.parked[&source].tools.snapshot.snapshot.is_some());
    app.select_workspace(source);
    click_control(&mut app, &ctx, "Modified · table t");
    click_control(&mut app, &ctx, "Target DDL");
    draw(&mut app, &ctx);
    click_control(
        &mut app,
        &ctx,
        "Open guarded migration draft in target editor",
    );
    assert_eq!(app.database.id, target);
    assert!(app.database.document.migration.is_some());
    let editor = app.database.document.id;
    app.run_all(false); // Guarded drafts force atomic execution even through this action.
    wait(&mut app, &ctx);
    assert!(!errored(visible_result(&app)));
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    assert!(app.database.document.migration.is_some());
    assert!(app.database.tools.snapshot.stale);
    app.poll_comparison(&ctx);
    assert!(!app.comparison_ready());
    app.switch_document(editor);
    app.database.document.sql = "CREATE TABLE main.stale(x)".into();
    app.run_all(true);
    wait(&mut app, &ctx);
    let status = &app.database.status;
    assert!(status.contains("schema changed"), "{status}");
    app.load_sql_window("SELECT id,value FROM t".into());
    assert!(app.database.document.migration.is_none());
    app.run_current(None);
    wait(&mut app, &ctx);
    assert_eq!(
        visible_result(&app).rows,
        [[Value::Integer(1), Value::Null]]
    );
    // A full workspace limit refuses another database, and quitting settles
    // every live session.
    assert_eq!(app.parked.len(), 1);
    app.preferences.database_limit = Some(2);
    assert!(!app.new_workspace(&ctx));
    assert!(app.database.status.contains("Database limit 2"));
    app.quit = true;
    app.continue_quit(&ctx);
    wait_until(&mut app, &ctx, |a| {
        let mut workspaces = std::iter::once(&a.database).chain(a.parked.values());
        workspaces.all(|d| d.session.is_none())
    });
    assert!(app.parked.values().all(|d| !d.closing));
}

#[test]
fn sidebar_search_loads_parked_columns_and_respects_scopes_and_keyboard() {
    let (dir, ctx, mut app) = browsing(
        "first_table",
        "id INTEGER PRIMARY KEY, first_value TEXT",
        "(1,'keep')",
        false,
    );
    let first = app.database.id;
    let retained = app.database.document.result.current;
    let second_path = dir.path().join("second.db");
    crate::test_support::create(
        &second_path,
        &["CREATE TABLE second_table(id INTEGER PRIMARY KEY, needle_column TEXT)"],
    );
    open_database(&mut app, &ctx, second_path, false);
    let second = app.database.id;
    app.select_workspace(first);
    app.parked.get_mut(&second).unwrap().ui.diagram.data = None;
    click_control(
        &mut app,
        &ctx,
        "This database: tables, columns and schema objects",
    );
    let lists = |app: &mut App, query: &str, label: &str| {
        app.sidebar_search.query = query.into();
        label_exists(&frame_at(app, &ctx, LAPTOP, vec![]), label)
    };
    let needle = "second_table.needle_column · column";
    assert!(!lists(&mut app, "needle_column", needle));
    assert!(app.parked[&second].ui.diagram.ticket.is_none());
    click_control(
        &mut app,
        &ctx,
        "All open: schema objects and available database files",
    );
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, LAPTOP, vec![]);
    assert_eq!(app.database.id, first);
    assert_eq!(app.database.document.result.current, retained);
    wait_until(&mut app, &ctx, |a| {
        a.parked[&second].ui.diagram.data.is_some()
    });
    control(&frame_at(&mut app, &ctx, LAPTOP, vec![]), needle);
    tap(&mut app, &ctx, egui::Key::Enter);
    wait(&mut app, &ctx);
    assert_eq!(app.database.id, second);
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("second_table".into(), "needle_column".into()))
    );
    assert!(app.sidebar_search.query.is_empty());
    click_control(
        &mut app,
        &ctx,
        "Databases: names and paths, including unopened files",
    );
    assert!(!lists(&mut app, "first_table", "first_table · table"));
    app.open_palette();
    app.database.ui.palette_query = "second_table".into();
    assert!(!lists(&mut app, "second_table", "second_table · table"));
    app.database.ui.palette_open = false;
    close_and_wait(&mut app, &ctx);
    app.select_workspace(first);
    close_and_wait(&mut app, &ctx);
}

mod sidebar_review;

#[test]
fn compact_record_rows_align_and_separate_null_composite_parent_links() {
    let (_dir, ctx, mut app) = accessible(
        &[
            "CREATE TABLE parent(a INTEGER,b INTEGER,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,status TEXT,FOREIGN KEY(a,b) REFERENCES parent(a,b))",
            "INSERT INTO parent VALUES(7,8)",
            "INSERT INTO child VALUES(1,7,NULL,'pending'),(2,7,8,'ready')",
        ],
        "child",
    );
    app.open_record_window(0, 0);
    let sequence = app.database.sequence;
    let output = draw_panel(&ctx, |ui| app.record_inspector(ui, 0));
    let at = |label: &str| texts(&output).find(|t| t.galley.job.text == label);
    let (id, status) = (at("id").unwrap().pos, at("status").unwrap().pos);
    assert!((status.y - id.y - 72.0).abs() < 1.0);
    assert_eq!(id.x, status.x);
    assert!(!texts(&output).any(|t| t.galley.job.text.contains("Parent:")));
    assert!(
        !label_exists(&output, "Follow parent"),
        "navigation must not take space from field values"
    );
    let relationships = draw_panel(&ctx, |ui| app.record_relationships(ui, 0));
    let parents: Vec<_> = super::test_ui::nodes(&relationships)
        .iter()
        .filter(|(_, n)| n.label() == Some("Follow parent"))
        .collect();
    assert_eq!(parents.len(), 1, "one link per composite relationship");
    assert!(parents.iter().all(|(_, n)| n.is_disabled()));
    assert_eq!(app.database.sequence, sequence);
    assert!(!app.has_drafts());
    click_panel(&ctx, "Follow parent", |ui| app.record_relationships(ui, 1));
    wait(&mut app, &ctx);
    assert_eq!(table_name(&app), "parent");
    assert_eq!(
        visible_result(&app).rows,
        [[Value::Integer(7), Value::Integer(8)]]
    );
    app.close();
}

#[test]
fn record_structure_selection_value_visuals_and_revert_preserve_stored_values() {
    let (_dir, ctx, mut app) = accessible(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,body TEXT,priority INTEGER,progress INTEGER,data BLOB)",
            "INSERT INTO sample VALUES(1,'original',4,75,X'00FF')",
        ],
        "sample",
    );
    app.open_record_window(0, 0);
    app.stage_cell(0, 1, text("edited"));
    let sequence = app.database.sequence;
    click_panel(&ctx, "Revert body draft", |ui| app.record_inspector(ui, 0));
    assert!(!app.has_drafts());
    assert_retained(&app, 0, 1, text("original"));
    app.open_cell(0, 3);
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("sample".into(), "progress".into()))
    );
    click_panel(&ctx, "priority · INTEGER · ", |ui| {
        app.structure_inspector(ui)
    });
    assert_eq!(app.database.document.inspect, Some((0, 2)));
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("sample".into(), "priority".into()))
    );
    assert!(app.database.document.tools.scroll_to_column);
    assert_eq!(app.database.sequence, sequence);
    let output = draw_panel(&ctx, |ui| app.record_inspector(ui, 0));
    assert!(texts(&output).any(|t| t.galley.job.text == "00 ff 2 bytes"));
    assert_retained(&app, 0, 4, Value::Blob(vec![0, 255]));
    app.close();
}

mod resources;
mod tools;

mod spreadsheet;

mod discoverability;

mod panels;
mod tiles;

mod assistant;
