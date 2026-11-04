use super::{
    App, View, context, egui, shell,
    test_ui::{control, control_from, pointer_button},
    tiling, workspace,
};
use std::time::{Duration, Instant};
use tursobench_core::{Budget, Event, EventKind, TransactionState, Value};
use tursobench_db::{Operation, Request};
fn assert_staged(app: &App, row: usize, col: usize, value: Value) {
    assert_eq!(
        app.document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(&app.document.result.rows[row], col)),
        Some(&value),
        "row {row}, column {col}"
    );
}
fn inspect_selected(app: &mut App, ctx: &egui::Context) {
    let start = Instant::now();
    loop {
        app.poll_database_inspection(ctx);
        if app.databases.inspection.ready(&app.path) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn wait(app: &mut App, ctx: &egui::Context) {
    let start = Instant::now();
    loop {
        app.poll(ctx);
        if app.ready
            && !app.closing
            && app.document.active.is_none()
            && app.document.tools.pages.pending.is_none()
            && app.document.tools.context.pending.is_none()
            && app.catalog_ticket.is_none()
            && app.commit_ticket.is_none()
            && app.tools.count_request.is_none()
            && app.document.tools.inspection_ticket.is_none()
            && app.ui.diagram.ticket.is_none()
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_closed(app: &mut App, ctx: &egui::Context) {
    let start = Instant::now();
    while app.session.is_some() {
        app.poll(ctx);
        assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn connected(path: &std::path::Path, writable: bool) -> (egui::Context, App) {
    let ctx = egui::Context::default();
    let mut app = App::new(&ctx);
    app.path = path.to_string_lossy().into_owned();
    app.exclusive = true;
    app.writable = writable;
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
fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    for pressed in [true, false] {
        frame(
            app,
            ctx,
            vec![egui::Event::PointerMoved(pos), pointer_button(pos, pressed)],
        );
    }
}
fn press_key(app: &mut App, ctx: &egui::Context, key: egui::Key, modifiers: egui::Modifiers) {
    frame(app, ctx, vec![key_event(key, modifiers)]);
}
fn frame_at(
    app: &mut App,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    super::test_ui::render(ctx, size, events, |ui| app.render(ui))
}
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    frame_at(app, ctx, egui::vec2(1400.0, 900.0), events);
}

#[test]
fn committed_sql_invalidates_other_tiles_and_refreshes_schema() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(1,'old')",
        ],
        true,
    );
    app.browse_table("t".into());
    wait(&mut app, &ctx);
    let table_view = app.document.id;
    app.run_text("BEGIN".into(), None);
    wait(&mut app, &ctx);
    app.run_text("UPDATE t SET value='new'".into(), None);
    wait(&mut app, &ctx);
    assert!(!app.document_ref(table_view).unwrap().stale);
    app.run_text("ROLLBACK".into(), None);
    wait(&mut app, &ctx);
    assert!(!app.document_ref(table_view).unwrap().stale);
    app.run_text("UPDATE t SET value='new'".into(), None);
    wait(&mut app, &ctx);
    assert!(app.document_ref(table_view).unwrap().stale);
    assert_eq!(
        app.document_ref(table_view).unwrap().result.rows[0][1],
        Value::Text("old".into())
    );
    app.run_text("CREATE TABLE added(id INTEGER PRIMARY KEY)".into(), None);
    wait(&mut app, &ctx);
    assert!(app.schema.iter().any(|o| o.name == "added"));
    app.close();
    wait_closed(&mut app, &ctx);
}

#[test]
fn sql_selection_shortcut_and_window_expansion_have_distinct_targets() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE sample(id INTEGER PRIMARY KEY)"], false);
    app.open_sql_window(false);
    let editor = app.document.id;
    app.document.sql = "SELECT 73 AS answer".into();
    frame(&mut app, &ctx, vec![]);
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(73)]]);
    assert_eq!(app.document.kind, tiling::Kind::Result);
    assert_eq!(app.workspace.expanded, None);
    assert_eq!(app.document_ref(editor).unwrap().sql, "SELECT 73 AS answer");
    let ticket = app.document.result.current;
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::CTRL);
    wait(&mut app, &ctx);
    assert_ne!(app.document.result.current, ticket);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(73)]]);
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
    );
    assert_eq!(app.workspace.expanded, Some(tiling::Kind::Sql));
    assert!(app.ui.sidebar_rect.right() <= app.workspace.area.left());
    app.focus_window(editor);
    app.minimize_pane(tiling::Kind::Sql);
    assert!(app.workspace.minimized.contains(&tiling::Kind::Sql));
    app.focus_pane(tiling::Kind::Sql);
    assert_eq!(app.document.sql, "SELECT 73 AS answer");
    app.close();
}

#[test]
fn independent_documents_route_results_and_keep_filters_drafts_and_sql() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, body TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO a VALUES(1, 'alpha')",
            "INSERT INTO b VALUES(2, 'beta')",
        ],
        true,
    );
    app.browse_table("a".into());
    let a_id = app.document.id;
    let a_ticket = app.document.result.current.unwrap();
    app.document.sql = "SELECT 'draft a'".into();
    app.document.filter = "body = alpha".into();
    app.browse_table("b".into());
    let b_id = app.document.id;
    app.document.sql = "SELECT 'draft b'".into();
    wait(&mut app, &ctx);
    assert_eq!(
        app.document.id, b_id,
        "background completions must not change focus"
    );
    assert_eq!(app.document.result.rows[0][1], Value::Text("beta".into()));
    app.switch_document(a_id);
    assert_eq!(app.document.result.current, Some(a_ticket));
    assert_eq!(app.document.result.rows[0][1], Value::Text("alpha".into()));
    assert_eq!(app.document.sql, "SELECT 'draft a'");
    assert_eq!(app.document.filter, "body = alpha");
    app.stage_cell(0, 1, Value::Text("staged alpha".into()));
    assert!(app.document.has_drafts());
    app.switch_document(b_id);
    assert!(app.has_drafts(), "draft protection is session wide");
    assert!(!app.can_edit(), "another view owns the pending edit batch");
    assert!(app.document.drafts.is_none());
    assert_eq!(app.document.sql, "SELECT 'draft b'");
    // Discarded and stale deliveries cannot change either view or steal focus.
    let b_ticket = app.document.result.current.unwrap();
    let original = app.document.result.rows.clone();
    app.documents.get_mut(&a_id).unwrap().discard_ticket = Some(a_ticket);
    for ticket in [
        a_ticket,
        tursobench_core::Ticket {
            generation: b_ticket.generation + 1,
            ..b_ticket
        },
        tursobench_core::Ticket {
            database: b_ticket.database + 1,
            ..b_ticket
        },
        tursobench_core::Ticket {
            session: b_ticket.session + 1,
            ..b_ticket
        },
    ] {
        for kind in [
            EventKind::Rows(vec![vec![Value::Integer(99)]]),
            EventKind::Columns(vec![]),
        ] {
            app.handle_event(&ctx, Event { ticket, kind });
            assert_eq!(app.document.id, b_id);
            assert_eq!(app.document.result.rows, original);
            assert_eq!(app.document.result.columns.len(), 2);
            assert_eq!(app.documents[&a_id].result.rows.len(), 1);
            assert_eq!(app.documents[&a_id].result.columns.len(), 2);
        }
    }
    // A closed view loses its rows, but its completed write still updates the session.
    app.documents.remove(&a_id);
    let completion = tursobench_core::Completion {
        elapsed_ms: 1,
        delivery: tursobench_core::Delivery::Complete,
        rows_seen: 1,
        changed: 1,
        transaction: TransactionState::Active,
        error: None,
    };
    let activity = app.activity.len();
    app.handle_event(
        &ctx,
        Event {
            ticket: tursobench_core::Ticket {
                session: a_ticket.session + 1,
                ..a_ticket
            },
            kind: EventKind::Finished(completion.clone()),
        },
    );
    assert_eq!(app.activity.len(), activity);
    assert_eq!(app.transaction, TransactionState::Autocommit);
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
    assert_eq!(app.transaction, TransactionState::Active);
    assert_eq!(app.activity.len(), activity + 1);
    assert_eq!(app.document.result.rows, original);
    assert_eq!(app.document.result.columns.len(), 2);
    app.close();
}

#[test]
fn failed_open_after_close_can_retry_a_valid_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("valid.db");
    crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);
    let ctx = egui::Context::default();
    let mut app = App::new(&ctx);
    app.set_database_path(dir.path().join("missing.db").to_str().unwrap().into());
    app.exclusive = true;
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    assert!(app.session.is_some());
    app.request_close();
    assert!(app.closing);
    wait_closed(&mut app, &ctx);
    assert!(!app.ready && !app.closing && !app.confirm_close);
    assert!(app.document.active.is_none());
    assert!(
        app.preferences.discovery.recent.is_empty(),
        "failed opens must not enter history"
    );

    app.set_database_path(path.to_str().unwrap().into());
    app.exclusive = true;
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    assert!(app.schema.iter().any(|table| table.name == "example"));
    assert_eq!(app.preferences.discovery.recent.len(), 1);
    assert_eq!(app.preferences.discovery.recent[0].path, path);
    app.close();
    wait_closed(&mut app, &ctx);
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.set_database_path(dir.path().join("missing.db").to_str().unwrap().into());
        app.exclusive = true;
        app.writable = true;
        inspect_selected(&mut app, &ctx);
        app.open(&ctx);
        app.quit = true;
        app.request_close();
        assert!(app.confirm_close);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if pending_export {
            app.export_job = Some(rx);
        }
        let mut close_sent = false;
        let started = Instant::now();
        while app.session.is_some() {
            close_sent |= poll_and_check_close(&mut app, &ctx);
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!app.closing && !app.confirm_close);
        assert_eq!(close_sent, !pending_export);
        if pending_export {
            tx.send(Ok("Export completed".into())).unwrap();
            assert!(poll_and_check_close(&mut app, &ctx));
        }
    }
}

#[test]
fn diagram_preserves_drafts_and_result_ownership_and_chrome_fits_both_themes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, "scratchpad")).unwrap();
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("buffer_ids".into());
    wait(&mut app, &ctx);
    let data = app.document.id;
    let retained = app.document.result.rows.clone();
    let ticket = app.document.result.current;
    app.stage_cell(0, 0, Value::Text("pending diagram edit".into()));
    app.show_structure();
    wait(&mut app, &ctx);
    assert_eq!(app.workspace.active, tiling::Kind::Structure);
    assert_eq!(app.document_ref(data).unwrap().result.current, ticket);
    assert_eq!(app.document_ref(data).unwrap().result.rows, retained);
    app.overview();
    let map = app.document.id;
    assert!(app.has_drafts());
    assert_eq!(app.document.kind, tiling::Kind::Map);
    for dark in [true, false] {
        app.preferences.appearance = Some(crate::appearance::Mode::from_dark(dark));
        app.apply_theme(&ctx);
        for (width, height, scale) in [
            (720.0, 480.0, 1.0),
            (1366.0, 768.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (3440.0, 1440.0, 1.0),
            (1440.0, 960.0, 1.5),
        ] {
            ctx.set_pixels_per_point(scale);
            for _ in 0..2 {
                frame_at(&mut app, &ctx, egui::vec2(width, height), vec![]);
            }
            assert!(app.ui.sidebar_rect.right() <= app.workspace.area.left());
            let viewport = ctx.content_rect();
            assert_eq!(app.ui.sidebar_rect.left(), viewport.left());
            assert_eq!(app.ui.sidebar_rect.top(), viewport.top());
            assert_eq!(app.ui.sidebar_rect.bottom(), viewport.bottom());
            for rect in &app.ui.ribbon_actions {
                assert!(
                    viewport.contains_rect(*rect),
                    "ribbon action outside viewport: {rect:?}"
                );
                assert!(rect.left() >= app.ui.sidebar_rect.right());
                assert!(rect.height() >= 24.0);
                assert!(
                    app.workspace
                        .pane_rects
                        .iter()
                        .any(|(_, tile)| tile.contains_rect(*rect))
                );
            }
            for (_, minimize, expand) in &app.workspace.controls {
                assert!((minimize.center().y - expand.center().y).abs() < 1.0);
                assert!(
                    app.workspace
                        .pane_rects
                        .iter()
                        .any(|(_, tile)| tile.contains_rect(*minimize)
                            && tile.contains_rect(*expand)),
                    "controls outside tile at {width}x{height}: {minimize:?}, {expand:?}; {:?}",
                    app.workspace.pane_rects
                );
                for action in &app.ui.ribbon_actions {
                    if app.workspace.pane_rects.iter().any(|(_, tile)| {
                        tile.contains_rect(*minimize) && tile.contains_rect(*action)
                    }) {
                        assert!(
                            (action.center().y - minimize.center().y).abs() < 1.0,
                            "header controls must share one horizontal line: {action:?}, {minimize:?}"
                        );
                    }
                }
            }
            for (_, rect) in &app.workspace.tiles {
                assert!(app.workspace.area.contains_rect(*rect));
            }
            let saved = app.workspace.minimized.clone();
            app.expand_window(map);
            frame_at(&mut app, &ctx, egui::vec2(width, height), vec![]);
            assert_eq!(app.workspace.tiles.len(), 1);
            assert_eq!(app.workspace.tiles[0].0, map);
            assert!(app.ui.sidebar_rect.right() <= app.workspace.tiles[0].1.left());
            app.expand_window(map);
            assert_eq!(app.workspace.minimized, saved);
        }
    }
    assert!(app.document_ref(data).unwrap().has_drafts());
    assert_eq!(app.document_ref(data).unwrap().result.rows, retained);
    app.activate_table("buffer_ids".into());
    assert_eq!(app.document.id, data);
    app.close();
}

#[test]
fn reverted_empty_batch_cannot_carry_identity_to_another_table() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY,value TEXT)",
            "INSERT INTO a VALUES(1,'same')",
            "INSERT INTO b VALUES(1,'same')",
        ],
        true,
    );
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("temporary".into()));
    app.document
        .drafts
        .as_mut()
        .unwrap()
        .revert_row(&app.document.result.rows[0]);
    assert!(!app.has_drafts());
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert_eq!(app.document.drafts.as_ref().unwrap().table.name, "b");
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows[0][1], Value::Text("only b".into()));
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows[0][1], Value::Text("same".into()));
    app.document.sql = "BEGIN".into();
    app.run_sql();
    wait(&mut app, &ctx);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Active);
    assert!(!app.can_edit());
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert!(!app.has_drafts());
    app.document.sql = "ROLLBACK".into();
    app.run_sql();
    wait(&mut app, &ctx);
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    let retained = app.document.result.rows.clone();
    app.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.session.is_some() {
        assert!(Instant::now() < deadline);
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.status.contains("cleanup confirmed"));
    app.document.ui.nav = shell::Nav::Table;
    app.first_cell_rect = None;
    frame(&mut app, &ctx, vec![]);
    assert!(app.first_cell_rect.is_some());
    assert_eq!(app.document.result.rows, retained);
    app.copy_results(&ctx, crate::export::Format::Json);
    assert!(app.status.contains("Copied retained"));
    app.browse_table("a".into());
    assert_eq!(app.document.table.as_ref().unwrap().name, "a");
    assert_eq!(app.document.result.rows[0][1], Value::Text("same".into()));
    app.document.ui.nav = shell::Nav::Result;
    frame(&mut app, &ctx, vec![]);
    assert!(app.session.is_none());
}
#[test]
fn local_project_pages_and_fts_results_render_and_export() {
    for name in crate::local_project_fixtures::NAMES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.db");
        crate::test_support::run(crate::local_project_fixtures::create(&path, name)).unwrap();
        let (ctx, mut app) = connected(&path, false);
        let table = match name {
            "scratchpad" => "buffer_ids",
            "agenda" => "items",
            "notifications" => "notifications",
            _ => "search_content",
        };
        app.browse_table(table.into());
        wait(&mut app, &ctx);
        assert_eq!(app.document.result.rows.len(), 100);
        assert!(!app.can_edit());
        frame(&mut app, &ctx, vec![]);
        let export = dir.path().join("page.jsonl");
        crate::export::write_result_as(
            &export,
            crate::export::Format::JsonLines,
            "table-page",
            &app.document.result.columns,
            &app.document.result.rows,
            "t",
        )
        .unwrap();
        let contents = std::fs::read_to_string(export).unwrap();
        let records: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(records.len(), 102);
        assert_eq!(records[0]["scope"], "table-page");
        assert_eq!(records.last().unwrap()["rows"], 100);
        if name == "agenda" {
            assert_eq!(records[1]["values"][5]["type"], "blob");
        }
        if name == "notifications" {
            assert_eq!(records[1]["values"][8]["type"], "null");
        }
        if name == "clipboard" {
            app.document.sql =
                "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)".into();
            app.document.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
            app.run_sql();
            wait(&mut app, &ctx);
            assert_eq!(
                app.document.result.rows,
                vec![vec![Value::Text("entry-0000".into())]]
            );
            frame(&mut app, &ctx, vec![]);
            crate::export::write_result_as(
                &dir.path().join("fts.jsonl"),
                crate::export::Format::JsonLines,
                "complete-query-result",
                &app.document.result.columns,
                &app.document.result.rows,
                "t",
            )
            .unwrap();
        }
        app.close();
        wait_closed(&mut app, &ctx);
        assert!(app.status.contains("cleanup confirmed"));
    }
}

#[test]
fn click_inspect_stage_no_write_failed_commit_retains_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO items VALUES (1, 'original'), (2, 'other')",
        ],
        true,
    );
    app.browse_table("items".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)],
    );
    frame(&mut app, &ctx, vec![pointer_button(pos, false)]);
    assert_eq!(app.document.inspect, Some((0, 0)));
    app.clone_row(0);
    let clone: Vec<Option<Value>> = serde_json::from_str(&app.document.insert_json).unwrap();
    assert_eq!(clone, vec![None, Some(Value::Text("original".into()))]);
    assert!(!app.has_drafts());
    app.document.show_insert = false;
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let original = app.document.result.rows[0].clone();
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
    app.document
        .drafts
        .as_mut()
        .unwrap()
        .revert_cell(&original, 1);
    assert!(!app.has_drafts());
    app.stage_delete(0);
    assert!(app.document.drafts.as_ref().unwrap().deleted(&original));
    app.document.drafts.as_mut().unwrap().revert_row(&original);
    assert!(!app.has_drafts());
    app.stage_cell(0, 0, Value::Integer(2));
    assert!(app.has_drafts());
    assert!(app.commit_ticket.is_none());
    // Inspecting, rendering and staging produced no SQL request or row mutation.
    assert_eq!(app.document.result.rows[0][0], Value::Integer(1));
    frame(&mut app, &ctx, vec![]);
    let s = app.session.as_ref().unwrap();
    app.sequence += 1;
    s.submit(Request {
        ticket: s.ticket(app.sequence, 0),
        operation: Operation::Execute {
            sql: "SELECT id FROM items ORDER BY id".into(),
            parameters: vec![],
            budget: Budget::default(),
        },
    })
    .unwrap();
    let mut rows = vec![];
    loop {
        match s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind {
            EventKind::Rows(r) => rows.extend(r),
            EventKind::Finished(c) => {
                assert!(c.error.is_none());
                break;
            }
            _ => {}
        }
    }
    assert_eq!(rows, vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]);
    // Deliberate key collision fails and retains local drafts.
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.has_drafts());
    assert!(app.status.contains("drafts retained"));
    app.stage_cell(0, 0, Value::Integer(3));
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_eq!(app.document.result.rows[1][0], Value::Integer(3));
    // An ambiguous acknowledgement must retain the owning view's draft and
    // prevent both another commit and further staging, even in the background.
    app.stage_cell(0, 1, Value::Text("uncertain draft".into()));
    let owner = app.document.id;
    let other = *app.documents.keys().next().unwrap();
    app.sequence += 1;
    let ticket = app
        .session
        .as_ref()
        .unwrap()
        .ticket(app.sequence, app.generation);
    app.commit_ticket = Some(ticket);
    app.commit_owner = Some(owner);
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
    assert_eq!(app.document.id, other);
    assert!(app.uncertain_edit && app.has_drafts());
    assert!(app.status.contains("UNKNOWN") && app.status.contains("Do not retry"));
    assert!(app.commit_ticket.is_none() && app.commit_owner.is_none());
    app.switch_document(owner);
    assert!(!app.can_edit());
    let sequence = app.sequence;
    app.commit_drafts();
    app.stage_cell(0, 1, Value::Text("blocked edit".into()));
    assert_eq!(app.sequence, sequence);
    assert_staged(&app, 0, 1, Value::Text("uncertain draft".into()));
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
    assert!(
        app.schema
            .iter()
            .any(|o| o.name == "child" && o.ddl.as_ref().unwrap().contains("FOREIGN KEY"))
    );
    app.browse_table("child".into());
    wait(&mut app, &ctx);
    assert!(app.document.tools.inspection_error.is_empty());
    let key = app.document.tools.inspection.as_ref().unwrap().foreign_keys[0].clone();
    assert!(app.cell_references(1, 1).is_empty());
    app.document.ui.nav = shell::Nav::Table;
    frame(&mut app, &ctx, vec![]);
    let pos = app
        .fk_cell_rect
        .expect("composite FK rendered as a link")
        .center();
    click(&mut app, &ctx, pos);
    wait(&mut app, &ctx);
    assert_eq!(app.document.ui.nav, shell::Nav::Table);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("parent"));
    assert_eq!(app.document.table.as_ref().unwrap().name, "parent");
    assert_eq!(app.document.result.rows.len(), 1);
    assert_eq!(app.export_scope(), "related-row-lookup");
    app.follow_incoming(0, "child".into(), key.clone());
    wait(&mut app, &ctx);
    assert_eq!(app.document.table.as_ref().unwrap().name, "child");
    assert_eq!(app.document.result.rows.len(), 1);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("parent"));
    app.document.tools.detail = Some(0);
    app.show_structure();
    app.tools.help = true;
    app.document.tools.ddl = app.schema.iter().find(|o| o.name == "parent").cloned();
    frame(&mut app, &ctx, vec![]);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.document.table.as_ref().unwrap().name, "child");
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("child"));
    assert_eq!(app.document.result.rows.len(), 2);
    app.follow_key(1, key);
    assert!(app.status.contains("NULL foreign key"));
    assert!(app.document.tools.related.is_none());
    app.document.sql = "SELECT 'λ;'; SELECT :name AS value;".into();
    let byte = app.document.sql.find("SELECT :name").unwrap();
    let cursor = app.document.sql[..byte].chars().count();
    app.document.tools.selection = cursor..cursor;
    app.document.params = r#"{":name":{"type":"Text","value":"bound-only-secret"}}"#.into();
    app.run_selection();
    wait(&mut app, &ctx);
    assert_eq!(
        app.document.result.rows,
        vec![vec![Value::Text("bound-only-secret".into())]]
    );
    assert!(app.document.sql.contains("λ;"));
    let history = &app.preferences.query_history[0];
    assert_eq!(history.database, app.path);
    assert!(history.elapsed_ms.is_some());
    assert!(!history.sql.contains("λ"));
    assert!(!history.failed);
    assert!(
        !serde_json::to_string(&app.preferences)
            .unwrap()
            .contains("bound-only-secret")
    );
    app.run_text("SELECT * FROM parent WHERE a=?1".into(), Some(false));
    wait(&mut app, &ctx);
    assert!(app.preferences.query_history[0].failed);
    assert!(
        app.preferences.query_history[0]
            .sql
            .starts_with("EXPLAIN QUERY PLAN")
    );
    app.document.sql = "SELECT * FROM parent".into();
    app.document.params = "[]".into();
    app.run_text(app.document.sql.clone(), Some(false));
    wait(&mut app, &ctx);
    assert_eq!(app.document.view, View::Explain);
    assert!(!app.document.result.rows.is_empty());
    app.document.tools.complete = true;
    frame(&mut app, &ctx, vec![]);
    app.close();
}

#[test]
fn overview_and_table_views_preserve_camera_and_rejects_navigation_with_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, value TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1, 'original')",
        ],
        true,
    );
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert_eq!(app.ui.diagram.data.as_ref().unwrap().tables.len(), 2);
    frame(&mut app, &ctx, vec![]);
    // Explicit user camera choices survive opening/resizing other views.
    app.ui.diagram.freeze_camera();
    let camera = (app.ui.diagram.camera.zoom, app.ui.diagram.camera.pan);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.document.ui.nav, shell::Nav::Table);
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Text("original".into())
    );
    let retained_ticket = app.document.result.current;
    app.refresh_catalog();
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.current, retained_ticket);
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Text("original".into())
    );
    for view in [
        shell::TableView::Data,
        shell::TableView::Structure,
        shell::TableView::Data,
    ] {
        app.document.ui.table_view = view;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            (app.ui.diagram.camera.zoom, app.ui.diagram.camera.pan),
            camera
        );
    }
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = app.document.result.current;
    app.browse_table("b".into());
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("a"));
    assert_eq!(app.document.table.as_ref().unwrap().name, "a");
    app.overview();
    app.activate_table("a".into());
    assert_eq!(app.document.ui.nav, shell::Nav::Table);
    assert_eq!(app.document.result.current, ticket);
    assert!(app.has_drafts());
    app.document.drafts = None;
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
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(2)]]);
    assert_eq!(app.document.table.as_ref().unwrap().name, "b");
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("b"));
    assert!(
        app.schema
            .iter()
            .any(|o| o.kind == "trigger" && o.table == "b")
    );
    app.ui.sidebar = shell::ToolPanel::Schema;
    app.browse_table("v".into());
    wait(&mut app, &ctx);
    assert_eq!(
        app.document.tools.inspection.as_ref().unwrap().table.name,
        "v"
    );
    assert!(!app.can_edit());
    app.document.ui.table_view = shell::TableView::Structure;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("v"));
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
    frame(&mut app, &ctx, vec![]);
    assert!(app.ui.transaction_rects.is_empty());
    app.toggle_sql();
    app.run_text("SELECT 42 AS answer".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.document.ui.nav, shell::Nav::Result);
    assert_eq!(app.document.view, View::Query);
    assert!(app.document.table.is_none());
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(42)]]);
    let ticket = app.document.result.current;
    let result_id = app.document.id;
    app.toggle_sql();
    assert_eq!(app.document.kind, tiling::Kind::Sql);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.document_ref(result_id).unwrap().result.current, ticket);
    app.toggle_sql();
    app.run_text("BEGIN".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Active);
    app.toggle_sql();
    app.overview();
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.ui.transaction_rects.len(), 2);
    let rollback = app.ui.transaction_rects[1].center();
    click(&mut app, &ctx, rollback);
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Autocommit);
    app.open_palette();
    frame(&mut app, &ctx, vec![]);
    app.ui.palette_query = "a.id".into();
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(app.document.table.as_ref().unwrap().name, "a");
    assert_eq!(
        app.document.tools.highlight_column,
        Some(("a".into(), "id".into()))
    );
    assert!(!app.ui.palette_open);
    app.close();
}

#[test]
fn access_switch_waits_for_closed_then_opens_a_new_session() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE a(id INTEGER PRIMARY KEY)"], false);
    let old_session = app.session.as_ref().unwrap().id;
    app.ui.diagram.camera.zoom = 0.7;
    app.ui.diagram.camera.pan = egui::vec2(30.0, 42.0);
    app.pending_access = Some(true);
    app.close();
    assert!(app.closing);
    assert!(!app.writable);
    wait(&mut app, &ctx);
    assert!(app.writable);
    assert_ne!(app.session.as_ref().unwrap().id, old_session);
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert!(app.pending_access.is_none());
    assert_eq!(app.ui.diagram.camera.zoom, 0.7);
    assert_eq!(app.ui.diagram.camera.pan, egui::vec2(30.0, 42.0));
    app.close();
}

#[test]
fn navigation_restores_typed_browse_context_and_query_return_without_dropping_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO a VALUES(1,'first'),(2,'second')",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
        ],
        true,
    );
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    let browse = app.document.browse.as_mut().unwrap();
    browse.sort = Some(1);
    browse.descending = true;
    browse.filters.push(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Equal,
        value: Value::Integer(2),
    });
    app.reload();
    wait(&mut app, &ctx);
    app.document.inspect = Some((0, 1));
    app.document.tools.detail = Some(0);
    app.document.tools.grid_scroll = egui::vec2(40.0, 20.0);
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.document.browse.as_ref().unwrap().sort, Some(1));
    assert!(app.document.browse.as_ref().unwrap().descending);
    assert_eq!(
        app.document.result.rows,
        vec![vec![Value::Integer(2), Value::Text("second".into())]]
    );
    assert_eq!(app.document.inspect, Some((0, 1)));
    assert_eq!(app.document.tools.detail, Some(0));
    assert_eq!(
        app.document.tools.restore_scroll,
        Some(egui::vec2(40.0, 20.0))
    );
    app.run_text("SELECT 42".into(), None);
    wait(&mut app, &ctx);
    assert!(!app.can_edit());
    assert!(app.tools.result_return.is_some());
    app.return_to_table();
    wait(&mut app, &ctx);
    assert_eq!(app.document.inspect, Some((0, 1)));
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = app.document.result.current;
    app.browse_table("b".into());
    app.run_text("SELECT 99".into(), None);
    app.navigate_back();
    assert_eq!(app.document.result.current, ticket);
    assert!(app.has_drafts());
    app.close();
}

#[test]
fn settings_shortcut_uses_the_left_panel_before_a_database_is_open() {
    let ctx = egui::Context::default();
    let mut app = App::new(&ctx);
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::Comma, egui::Modifiers::CTRL);
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert_eq!(app.ui.sidebar, shell::ToolPanel::Settings);
    assert_eq!(app.document.ui.nav, shell::Nav::Databases);
    let heading = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Settings" => Some(text),
            _ => None,
        })
        .expect("Settings is visible before opening a database");
    assert!(
        app.ui
            .sidebar_rect
            .contains_rect(heading.galley.rect.translate(heading.pos.to_vec2()))
    );
    app.select_tool(shell::ToolPanel::Schema);
    frame(&mut app, &ctx, vec![]);
    assert!(app.session.is_none());
    assert_eq!(app.document.ui.nav, shell::Nav::Databases);
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
    app.browse_table("example".into());
    wait(&mut app, &ctx);
    app.ui.diagram.data = Some(tursobench_core::SchemaDiagram {
        tables: vec![],
        limited: true,
    });
    let rows = app.document.result.rows.clone();
    let ticket = app.document.result.current;
    for panel in [
        shell::ToolPanel::History,
        shell::ToolPanel::Saved,
        shell::ToolPanel::Diagnostics,
        shell::ToolPanel::Settings,
    ] {
        let nav = app.document.ui.nav;
        app.select_tool(panel);
        assert_eq!(app.ui.sidebar, panel);
        assert_eq!(app.document.ui.nav, nav);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        assert!(output.shapes.len() > 10);
        if panel == shell::ToolPanel::Settings {
            let heading = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Settings" => Some(text),
                    _ => None,
                })
                .expect("Settings is visible in the navigator");
            assert!(
                app.ui
                    .sidebar_rect
                    .contains_rect(heading.galley.rect.translate(heading.pos.to_vec2()))
            );
            assert!(app.ui.sidebar_rect.width() <= 300.0);
            assert!(
                app.ui.visible_rows > 0,
                "Settings must keep the data workspace visible"
            );
        }
        app.select_tool(shell::ToolPanel::Schema);
        output.textures_delta.clear();
        assert_eq!(app.document.result.current, ticket);
        assert_eq!(app.document.result.rows, rows);
    }
    app.browse_table("example_view".into());
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, rows);
    assert!(!app.can_edit());
    app.close();
}

#[test]
fn explicit_counts_keep_table_rows_and_drafts_owned_by_their_original_ticket() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY,value TEXT)",
            "INSERT INTO a VALUES(1,'original'),(2,'second')",
        ],
        true,
    );
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = app.document.result.current;
    let rows = app.document.result.rows.clone();
    app.count_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.tools.counts.get("a"), Some(&2));
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(app.document.result.rows, rows);
    assert!(app.has_drafts());
    app.count_table("missing table".into());
    wait(&mut app, &ctx);
    assert!(app.status.contains("Count failed"));
    assert!(!app.tools.counts.contains_key("missing table"));
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(app.document.result.rows, rows);
    assert!(app.has_drafts());
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.tools.counts.is_empty());
    app.close();
}

#[test]
fn table_views_sidebar_and_transactions_fit_themes_and_scaled_viewports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minimum.db");
    let name = "a_table_with_a_deliberately_long_name_to_exercise_header_truncation";
    crate::test_support::create(
        &path,
        &[
            &format!("CREATE TABLE {name}(id INTEGER PRIMARY KEY,value TEXT)"),
            &format!(
                "INSERT INTO {name} WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200) SELECT x,'value' FROM n"
            ),
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    ctx.enable_accesskit();
    app.browse_table(name.into());
    wait(&mut app, &ctx);
    for dark in [true, false] {
        app.preferences.appearance = Some(crate::appearance::Mode::from_dark(dark));
        app.apply_theme(&ctx);
        for (width, height, scale) in [
            (1280.0, 720.0, 1.0),
            (720.0, 480.0, 1.0),
            (960.0, 640.0, 1.0),
            (1366.0, 768.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (3440.0, 1440.0, 1.0),
            (1440.0, 960.0, 1.5),
        ] {
            ctx.set_pixels_per_point(scale);
            for (sql, structure, details) in [
                (false, false, false),
                (false, false, true),
                (false, true, false),
                (true, false, false),
                (true, true, false),
            ] {
                app.document.ui.sql_open = sql;
                app.document.ui.table_view = if structure {
                    shell::TableView::Structure
                } else {
                    shell::TableView::Data
                };
                app.document.ui.inspector_open = details;
                app.document.inspect = Some((0, 0));
                for _ in 0..2 {
                    let output = frame_at(&mut app, &ctx, egui::vec2(width, height), vec![]);
                    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
                    assert!(
                        !tree
                            .nodes
                            .iter()
                            .any(|(_, node)| node.label() == Some("Insert row"))
                    );
                }
                assert!(app.ui.sidebar_rect.width() >= 180.0);
                assert_eq!(app.workspace.pane_rects.len(), 5);
                if !structure {
                    assert!(
                        app.ui.workspace_rect.contains_rect(app.ui.grid_rect),
                        "{width}x{height}: workspace {:?}, grid {:?}, tiles {:?}",
                        app.ui.workspace_rect,
                        app.ui.grid_rect,
                        app.workspace.tiles
                    );
                }
                if width == 1280.0 && !details && !sql && !structure {
                    assert!(
                        app.ui.visible_rows >= 9,
                        "{} fully visible rows",
                        app.ui.visible_rows
                    );
                }
            }
        }
    }
    ctx.set_pixels_per_point(1.0);
    app.run_text("BEGIN".into(), None);
    wait(&mut app, &ctx);
    app.document.ui.sql_open = false;
    app.overview();
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(720.0, 480.0), vec![]);
    }
    assert_eq!(app.ui.transaction_rects.len(), 2);
    for r in &app.ui.transaction_rects {
        assert!(ctx.content_rect().contains_rect(*r), "{r:?}");
    }
    app.close();
}

#[test]
fn escape_and_search_close_popups_without_replacing_the_retained_result() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
        ],
        false,
    );
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    let ticket = app.document.result.current;
    let popup = egui::Id::unique("test-menu");
    egui::Popup::open_id(&ctx, popup);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&ctx));
    egui::Popup::open_id(&ctx, popup);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(app.ui.palette_open);
    assert!(!egui::Popup::is_any_open(&ctx));
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.ui.palette_open);
    app.document.ui.sql_open = true;
    app.document.ui.filter_open = true;
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.document.ui.filter_open);
    assert!(app.document.ui.sql_open);
    for (key, modifiers, opened) in [
        (
            egui::Key::E,
            egui::Modifiers::CTRL,
            (|a: &App| a.ui.export_open) as fn(&App) -> bool,
        ),
        (egui::Key::F, egui::Modifiers::CTRL, |a: &App| {
            a.document.ui.filter_open
        }),
        (egui::Key::F1, egui::Modifiers::NONE, |a: &App| a.tools.help),
    ] {
        press_key(&mut app, &ctx, key, modifiers);
        assert!(opened(&app), "{key:?}");
        press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!opened(&app), "Escape must close {key:?}'s surface");
    }
    app.document.tools.columns.hidden.insert(0);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Text("Show hidden columns".into())],
    );
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert!(!app.ui.palette_open);
    assert!(app.document.tools.columns.hidden.is_empty());
    assert_eq!(app.document.result.current, ticket);
    app.close();
}

#[test]
fn column_presentation_preserves_schema_indices_and_manual_widths() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, label TEXT, number INTEGER)",
            "INSERT INTO sample VALUES(1,'a long string',42)",
        ],
        true,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    ctx.enable_accesskit();
    for (label, sorted, descending) in [
        ("Column number", Some(2), false),
        ("Column number ascending", Some(2), true),
        ("Column number descending", None, false),
    ] {
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        click(&mut app, &ctx, control(&output, label).center());
        wait(&mut app, &ctx);
        let browse = app.document.browse.as_ref().unwrap();
        assert_eq!((browse.sort, browse.descending), (sorted, descending));
    }
    app.document
        .tools
        .columns
        .visuals
        .insert(1, super::columns::CellStyle::Status);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Text("a long string".into())
    );
    app.document.tools.columns.move_before(2, 0);
    for (from, target, expected) in [
        (2, 2, vec![2, 0, 1]),
        (9, 0, vec![2, 0, 1]),
        (0, 9, vec![2, 0, 1]),
        (2, 1, vec![0, 2, 1]),
        (2, 0, vec![2, 0, 1]),
    ] {
        app.document.tools.columns.move_before(from, target);
        assert_eq!(app.document.tools.columns.order, expected);
    }
    app.document.tools.columns.hidden.insert(1);
    app.document.tools.columns.manual.insert(2);
    app.document.tools.columns.widths[2] = 217.0;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.visible_columns(), vec![2, 0]);
    assert_eq!(app.document.tools.columns.widths[2], 217.0);
    app.open_cell(0, 2);
    assert_eq!(app.document.tools.cell_editor.input, "42");
    app.stage_cell(0, 2, Value::Integer(55));
    assert_staged(&app, 0, 2, Value::Integer(55));
    assert_eq!(app.document.result.rows[0][0], Value::Integer(1));
    super::test_ui::render(&ctx, egui::vec2(300.0, 240.0), vec![], |ui| {
        let rect = ui.available_rect_before_wrap();
        for (style, name, value, painted) in [
            (
                super::columns::CellStyle::Auto,
                "status",
                Value::Text("blocked".into()),
                true,
            ),
            (
                super::columns::CellStyle::Status,
                "state",
                Value::Text("pending".into()),
                true,
            ),
            (
                super::columns::CellStyle::Percent,
                "progress",
                Value::Real(50.0),
                false,
            ),
            (
                super::columns::CellStyle::Priority,
                "priority",
                Value::Integer(3),
                false,
            ),
            (
                super::columns::CellStyle::Priority,
                "priority",
                Value::Null,
                false,
            ),
            (
                super::columns::CellStyle::Percent,
                "progress",
                Value::Real(f64::NAN),
                false,
            ),
        ] {
            assert_eq!(style.paint(ui, rect, name, &value), painted);
        }
    });
    let menu_ctx = egui::Context::default();
    menu_ctx.enable_accesskit();
    for label in ["Fit contents", "Move left", "Move right", "Hide column"] {
        let mut draw = |events| {
            super::test_ui::render(&menu_ctx, egui::vec2(300.0, 240.0), events, |ui| {
                app.document.tools.columns.header_menu(ui, 2)
            })
        };
        draw(vec![]);
        let output = draw(vec![]);
        let pos = control(&output, label).center();
        for pressed in [true, false] {
            draw(super::test_ui::pointer(pos, pressed));
        }
    }
    assert!(!app.document.tools.columns.manual.contains(&2));
    assert!(app.document.tools.columns.refit);
    assert_eq!(app.document.tools.columns.order, vec![0, 2, 1]);
    assert!(app.document.tools.columns.hidden.contains(&2));
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
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    let owner = app.document.result.current;
    let first = app.document.result.rows.clone();
    app.request_page(false);
    assert_eq!(app.document.result.rows, first);
    assert!(app.document.tools.pages.pending.is_some());
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows.len(), 200);
    assert_eq!(app.document.result.current, owner);
    app.request_page(false);
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.select_cell(150, 0, false);
    app.document.tools.grid_scroll.y = 3000.0;
    app.run_text("SELECT 765".into(), None);
    wait(&mut app, &ctx);
    app.return_to_table();
    wait(&mut app, &ctx);
    for _ in 0..2 {
        frame(&mut app, &ctx, vec![]);
        wait(&mut app, &ctx);
    }
    assert_eq!(app.document.result.rows.len(), 300);
    assert_eq!(app.document.inspect, Some((150, 0)));
    assert_eq!(app.document.tools.grid_selection.unwrap().cursor, (150, 0));
    assert!(app.document.tools.restore_navigation.is_none());
    app.select_cell(200, 1, false);
    app.request_page(false);
    wait(&mut app, &ctx);
    assert_eq!(app.document.browse.as_ref().unwrap().offset, 100);
    assert_eq!(app.document.result.rows.len(), 300);
    assert_eq!(app.document.inspect, Some((100, 1)));
    app.request_page(true);
    wait(&mut app, &ctx);
    assert_eq!(app.document.browse.as_ref().unwrap().offset, 0);
    assert_eq!(app.document.result.rows[0][0], Value::Integer(1));
    app.stage_cell(0, 1, Value::Integer(999));
    app.request_page(false);
    assert!(app.document.tools.pages.pending.is_none());
    assert!(app.has_drafts());
    app.document.drafts = None;
    app.request_page(false);
    app.run_text("SELECT 987 AS answer".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(987)]]);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    for _ in 0..5 {
        app.request_page(false);
        wait(&mut app, &ctx);
    }
    assert_eq!(app.document.tools.pages.end, Some(450));
    assert_eq!(
        app.document.result.rows.last().unwrap()[0],
        Value::Integer(450)
    );
    let rows = app.document.result.rows.clone();
    app.request_page(false);
    assert!(app.document.tools.pages.pending.is_none());
    assert_eq!(app.document.result.rows, rows);
    assert!(app.add_filter(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Equal,
        value: Value::Integer(1)
    }));
    wait(&mut app, &ctx);
    assert_eq!(
        app.document.result.rows,
        vec![vec![Value::Integer(1), Value::Integer(1)]],
        "filter queries the entire table, including evicted rows"
    );
    assert_eq!(app.document.browse.as_ref().unwrap().offset, 0);
    app.close();
}

#[test]
fn contextual_reads_and_wide_related_grid_preserve_parent_result_and_selection() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT COLLATE NOCASE,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a TEXT,b TEXT,payload TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'Alpha')",
            "INSERT INTO child VALUES(1,'007','alpha','child-only')",
        ],
        false,
    );
    app.browse_table("child".into());
    wait(&mut app, &ctx);
    let child = app.document.tools.inspection.as_ref().unwrap().foreign_keys[0].clone();
    let owner = app.document.result.current;
    let original = app.document.result.rows.clone();
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    wait(&mut app, &ctx);
    let submitted = app.sequence;
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    assert_eq!(
        app.sequence, submitted,
        "cache hits must not submit another request"
    );
    assert_eq!(app.document.result.current, owner);
    assert_eq!(app.document.result.rows, original);
    app.browse_table("parent".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.select_cell(0, 0, false);
    let owner = app.document.result.current;
    let original = app.document.result.rows.clone();
    app.request_context(0, "child", &child, context::ReadKind::Count);
    wait(&mut app, &ctx);
    frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    wait(&mut app, &ctx);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    }
    let output = frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| {
            if let egui::Shape::Text(t) = &s.shape {
                Some(t.galley.job.text.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(texts.iter().any(|t| t.contains("child-only")), "{texts:?}");
    assert_eq!(app.document.result.current, owner);
    assert_eq!(app.document.result.rows, original);
    assert_eq!(app.document.inspect, Some((0, 0)));
    app.request_context(0, "child", &child, context::ReadKind::Count);
    app.run_text("SELECT 654".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(654)]]);
    app.close();
}

#[test]
fn actual_grid_keys_extend_copy_and_stage_inline_without_writing() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER,text TEXT)",
            "INSERT INTO sample VALUES(1,9007199254740993,''),(2,9007199254740995,'two')",
        ],
        true,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
    click(&mut app, &ctx, pos);
    let send = |app: &mut App, mods, mut events: Vec<egui::Event>| {
        events.insert(0, egui::Event::ModifiersChanged(mods));
        frame_at(app, &ctx, egui::vec2(1280.0, 720.0), events)
    };
    send(
        &mut app,
        egui::Modifiers::SHIFT,
        vec![key_event(egui::Key::ArrowRight, egui::Modifiers::SHIFT)],
    );
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 0), (0, 1)]
    );
    let output = send(
        &mut app,
        egui::Modifiers::CTRL,
        vec![key_event(egui::Key::C, egui::Modifiers::CTRL)],
    );
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c,egui::OutputCommand::CopyText(t) if t=="1\t9007199254740993"))
    );
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![key_event(egui::Key::ArrowRight, egui::Modifiers::NONE)],
    );
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(app.document.tools.inline.is_some());
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![
            egui::Event::Text("draft".into()),
            key_event(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert!(app.document.tools.inline.is_none());
    assert!(app.has_drafts());
    assert_eq!(app.document.result.rows[0][2], Value::Text(String::new()));
    // The editable record inspector uses the same focus/submission policy as the grid.
    app.focus_pane(tiling::Kind::Record);
    app.start_inline(0, 1);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.record_inspector(ui, 0));
    output.textures_delta.clear();
    for (input, accepted) in [("invalid", false), ("123", true)] {
        app.document.tools.inline.as_mut().unwrap().editor.input = input.into();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
                ..Default::default()
            },
            |ui| app.record_inspector(ui, 0),
        );
        output.textures_delta.clear();
        assert_eq!(app.document.tools.inline.is_none(), accepted);
    }
    assert_staged(&app, 0, 1, Value::Integer(123));
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Integer(9007199254740993)
    );
    app.start_inline(0, 1);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.document.tools.inline.is_none());
    app.select_cell(0, 1, false);
    app.select_cell(1, 1, true);
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 1), (1, 1)]
    );
    let summary = app.selection_summary();
    assert!(summary.contains("18014398509481988"), "{summary}");
    app.document.tools.columns.move_before(2, 1);
    app.select_cell(0, 2, false);
    app.select_cell(1, 1, true);
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 2), (0, 1), (1, 2), (1, 1)]
    );
    app.close();
}

#[test]
fn token_entry_enter_retains_typed_filter_and_empty_clipboard_fields() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER,text TEXT)",
            "INSERT INTO sample VALUES(1,1,''),(2,2,'two')",
        ],
        false,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.document.tools.filter_focus = true;
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("n>=2".into()),
            key_event(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows.len(), 1);
    assert_eq!(app.document.result.rows[0][0], Value::Integer(2));
    assert_eq!(
        app.document.browse.as_ref().unwrap().filters[0].op,
        tursobench_core::FilterOp::GreaterOrEqual
    );
    app.document.tools.filter_focus = true;
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::Backspace, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    assert!(app.document.browse.as_ref().unwrap().filters.is_empty());
    assert_eq!(app.document.result.rows.len(), 2);
    app.document.tools.columns.move_before(2, 0);
    app.select_cell(0, 2, false);
    app.select_cell(0, 0, true);
    assert_eq!(app.selection_tsv().unwrap(), "\t1");
    app.close();
}

#[test]
fn clipboard_grid_and_record_share_live_rows_without_overlays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clipboard.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, "clipboard")).unwrap();
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("entries".into());
    wait(&mut app, &ctx);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    }
    assert!(!app.document.ui.inspector_open);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1920.0, 1080.0), vec![]);
    }
    assert!(!app.document.ui.inspector_open);
    assert_eq!(app.document.result.columns.len(), 11);
    app.expand_window(app.document.id);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1920.0, 1080.0), vec![]);
    }
    let width: f32 = app.document.tools.columns.widths.iter().sum::<f32>()
        + 44.0
        + app.document.result.columns.len() as f32
            * ctx.style_of(egui::Theme::Dark).spacing.item_spacing.x;
    assert!(
        width <= app.ui.grid_rect.width() + 1.0,
        "columns {width} > grid {}",
        app.ui.grid_rect.width()
    );
    assert!(app.ui.visible_rows >= 30, "{} rows", app.ui.visible_rows);
    let data_id = app.document.id;
    app.open_record_window(0, 0);
    assert_eq!(app.workspace.active, tiling::Kind::Record);
    assert_eq!(app.document.id, data_id);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(2560.0, 1440.0), vec![]);
    }
    assert!(!app.document.ui.inspector_open);
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
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.tools.schema_filter = "sample_value".into();
    frame(&mut app, &ctx, vec![]);
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    let pos = output
        .shapes
        .iter()
        .rev()
        .find_map(|s| {
            if let egui::Shape::Text(t) = &s.shape {
                (t.galley.job.text == "sample_value" && app.ui.sidebar_rect.contains(t.pos))
                    .then_some(t.pos + egui::vec2(8.0, 6.0))
            } else {
                None
            }
        })
        .expect("filtered index row");
    let data_id = app.document.id;
    let owner = app.document.result.current;
    let rows = app.document.result.rows.clone();
    click(&mut app, &ctx, pos);
    assert_eq!(
        app.document.tools.ddl.as_ref().unwrap().name,
        "sample_value"
    );
    assert_eq!(app.document.ui.table_view, shell::TableView::Structure);
    app.refresh_table();
    wait(&mut app, &ctx);
    assert_eq!(app.document.ui.table_view, shell::TableView::Structure);
    assert_eq!(app.document_ref(data_id).unwrap().result.current, owner);
    assert_eq!(app.document_ref(data_id).unwrap().result.rows, rows);
    app.close();
}

#[test]
fn cancelling_a_recent_database_switch_clears_the_pending_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("active.db");
    crate::test_support::create(&path, &["CREATE TABLE sample(id INTEGER PRIMARY KEY)"]);
    let (ctx, mut app) = connected(&path, true);
    ctx.enable_accesskit();
    let session = app.session.as_ref().unwrap().id;
    let other = dir.path().join("other.db");
    app.preferences.discovery.record_open(other.clone(), 1);
    fn control(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        let bounds = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                node.label()
                    .filter(|name| name.starts_with(label))
                    .and(node.bounds())
            })
            .unwrap_or_else(|| panic!("Missing switcher control: {label}"));
        egui::pos2(
            (bounds.x0 + bounds.x1) as f32 / 2.0,
            (bounds.y0 + bounds.y1) as f32 / 2.0,
        )
    }
    for target in [&path, &other] {
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        click(&mut app, &ctx, control(&output, "Switch database"));
        frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        click(&mut app, &ctx, control(&output, target.to_str().unwrap()));
        if target == &path {
            assert!(!app.confirm_close && !app.closing);
            assert!(app.pending_database.is_none());
            assert_eq!(app.session.as_ref().unwrap().id, session);
        }
    }
    assert_eq!(app.pending_database.as_ref(), Some(&other));
    assert!(app.confirm_close);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.pending_database.is_none());
    assert!(!app.confirm_close);
    assert!(!app.closing);
    assert_eq!(app.session.as_ref().unwrap().id, session);
    assert_eq!(app.path, path.to_string_lossy());
    for action in ["Choose database", "Close database"] {
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        click(&mut app, &ctx, control(&output, "Switch database"));
        frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        click(&mut app, &ctx, control(&output, action));
        assert!(app.confirm_close);
        assert!(app.pending_database.is_none());
        press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!app.closing && !app.confirm_close);
        assert_eq!(app.session.as_ref().unwrap().id, session);
    }
    app.close();
}

#[test]
fn full_length_workspace_dividers_resize_adjacent_tiles_and_preserve_live_state() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'retained')",
        ],
        true,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.select_cell(0, 1, false);
    app.stage_cell(0, 1, Value::Text("draft".into()));
    for _ in 0..2 {
        frame(&mut app, &ctx, vec![]);
    }
    let ticket = app.document.result.current;
    let sequence = app.sequence;
    let focus = app.workspace.focused;
    let drag = |app: &mut App, start: egui::Pos2, delta: egui::Vec2| {
        frame(
            app,
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                pointer_button(start, true),
            ],
        );
        frame(app, &ctx, vec![egui::Event::PointerMoved(start + delta)]);
        frame(app, &ctx, vec![]);
        frame(app, &ctx, vec![pointer_button(start + delta, false)]);
        frame(app, &ctx, vec![]);
    };
    for divider in [
        tiling::Divider::Columns,
        tiling::Divider::Rows(0, 1),
        tiling::Divider::Rows(2, 3),
        tiling::Divider::Rows(3, 4),
    ] {
        for (fraction, distance) in [(0.08, 30.0), (0.92, -20.0)] {
            let before = app.workspace.pane_rects.clone();
            let rect = tiling::dividers(&before, &app.workspace.minimized, app.workspace.area)
                .into_iter()
                .find(|(d, _)| *d == divider)
                .unwrap()
                .1;
            let original_splits = app.workspace.splits;
            click(&mut app, &ctx, rect.center());
            assert_eq!(
                app.workspace.splits, original_splits,
                "clicking without dragging must not change proportions"
            );
            let (start, delta) = if divider.vertical() {
                (
                    egui::pos2(rect.center().x, rect.top() + rect.height() * fraction),
                    egui::vec2(distance, 0.0),
                )
            } else {
                (
                    egui::pos2(rect.left() + rect.width() * fraction, rect.center().y),
                    egui::vec2(0.0, distance),
                )
            };
            drag(&mut app, start, delta);
            let after = &app.workspace.pane_rects;
            match divider {
                tiling::Divider::Columns => {
                    for i in 0..5 {
                        let expected = if i < 2 { distance } else { -distance };
                        assert!((after[i].1.width() - before[i].1.width() - expected).abs() < 0.1);
                        assert!((after[i].1.height() - before[i].1.height()).abs() < 0.1);
                    }
                }
                tiling::Divider::Rows(a, b) => {
                    assert!((after[a].1.bottom() - before[a].1.bottom() - distance).abs() < 0.1);
                    assert!((after[b].1.top() - before[b].1.top() - distance).abs() < 0.1);
                    for i in 0..5 {
                        if i != a && i != b {
                            assert!((after[i].1.min - before[i].1.min).length() < 0.1);
                            assert!((after[i].1.max - before[i].1.max).length() < 0.1);
                        }
                    }
                }
            }
        }
    }
    for distance in [10000.0, -10000.0] {
        let panes = &app.workspace.pane_rects;
        let handle = tiling::dividers(panes, &app.workspace.minimized, app.workspace.area)[0].1;
        drag(&mut app, handle.center(), egui::vec2(distance, 0.0));
        assert!(
            app.workspace
                .pane_rects
                .iter()
                .all(|(_, rect)| rect.width() >= 179.9)
        );
        let panes = &app.workspace.pane_rects;
        let handle = tiling::dividers(panes, &app.workspace.minimized, app.workspace.area)[1].1;
        drag(&mut app, handle.center(), egui::vec2(0.0, distance));
        assert!(
            app.workspace
                .pane_rects
                .iter()
                .all(|(_, rect)| rect.height() >= 95.9)
        );
    }
    let splits = app.workspace.splits;
    for fraction in [0.08, 0.92] {
        let before = app.ui.sidebar_rect;
        let pos = egui::pos2(before.right(), before.top() + before.height() * fraction);
        drag(&mut app, pos, egui::vec2(30.0, 0.0));
        assert!((app.ui.sidebar_rect.width() - before.width() - 30.0).abs() < 1.0);
        assert_eq!(app.workspace.splits, splits);
    }
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Text("retained".into())
    );
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
    assert_eq!(app.document.inspect, Some((0, 1)));
    assert_eq!(app.workspace.focused, focus);
    assert_eq!(
        app.sequence, sequence,
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
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE chunks(id TEXT PRIMARY KEY, part INTEGER, body BLOB)",
            "INSERT INTO chunks VALUES('buffer-1780913432386559807-118',0,x'001122')",
        ],
        false,
    );
    app.browse_table("chunks".into());
    wait(&mut app, &ctx);
    let data_id = app.document.id;
    app.expand_window(data_id);
    frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    assert!(
        app.document
            .tools
            .columns
            .widths
            .iter()
            .all(|w| *w <= 300.0)
    );
    assert!(app.document.tools.columns.widths[1] < 100.0);
    let widths = app.document.tools.columns.widths.clone();
    frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert_eq!(widths, app.document.tools.columns.widths);
    app.open_record_window(0, 0);
    assert_eq!(
        app.document.id, data_id,
        "record shares the live table buffer"
    );
    assert!(app.document.result.current.is_some());
    app.focus_window(data_id);
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.open_sql_window(false);
    let editor = app.document.id;
    app.document.sql = "SELECT 'private-literal'".into();
    app.document.params = "[\"private-binding\"]".into();
    app.retile();
    app.minimize_pane(tiling::Kind::Map);
    app.expand_window(editor);
    app.workspace.splits = tiling::Splits {
        column: 0.48,
        rows: [1.0, 2.0, 1.5, 0.8, 1.0],
    };
    let snapshot = app.workspace_snapshot().unwrap();
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("private") && !encoded.contains("buffer-"));
    let mut legacy: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    legacy.as_object_mut().unwrap().remove("splits");
    let legacy: workspace::Snapshot = serde_json::from_value(legacy).unwrap();
    assert!(legacy.valid());
    app.preferences.workspace_layouts = vec![legacy];
    app.restore_workspace();
    assert_eq!(app.workspace.splits, tiling::Splits::default());
    for invalid in [
        serde_json::json!({"column": 0.0}),
        serde_json::json!({"rows": [1.0, 1.0, 1.0, 0.0, 1.0]}),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        value["splits"] = invalid;
        assert!(
            !serde_json::from_value::<workspace::Snapshot>(value)
                .unwrap()
                .valid()
        );
    }
    app.preferences.workspace_layouts = vec![serde_json::from_str(&encoded).unwrap()];
    app.restore_workspace();
    assert_eq!(app.workspace_snapshot(), Some(snapshot));
    assert_eq!(app.document.id, editor);
    assert!(app.document.sql.is_empty());
    assert_eq!(app.document.params, "[]");
    assert_eq!(app.workspace.expanded, Some(tiling::Kind::Sql));
    let start = Instant::now();
    loop {
        app.poll(&ctx);
        if !app.executing()
            && app
                .documents
                .values()
                .all(|d| !d.needs_load && d.tools.inspection_ticket.is_none())
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.document.id, editor, "background reload preserves focus");
    assert_eq!(app.document_ref(data_id).unwrap().result.rows.len(), 1);
    let mut invalid: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    invalid["focused"] = serde_json::json!(9999);
    let invalid: workspace::Snapshot = serde_json::from_value(invalid).unwrap();
    assert!(!invalid.valid());
    app.preferences.workspace_layouts.push(invalid);
    app.preferences.normalize();
    assert_eq!(app.preferences.workspace_layouts.len(), 1);
    app.close();
    wait_closed(&mut app, &ctx);
    app.open(&ctx);
    let start = Instant::now();
    while !app.ready {
        app.poll(&ctx);
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        app.document.id, editor,
        "Opened must keep the restored focus before the first UI frame"
    );
    assert_eq!(app.workspace.expanded, Some(tiling::Kind::Sql));
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
    let map = app.document.id;
    frame(&mut app, &ctx, vec![]);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.retile();
    for expanded in [false, true, false] {
        if expanded != app.workspace.expanded.is_some() {
            app.expand_window(map);
        }
        for _ in 0..2 {
            frame_at(&mut app, &ctx, egui::vec2(1800.0, 1000.0), vec![]);
        }
        let tile = app
            .workspace
            .tiles
            .iter()
            .find(|(id, _)| *id == map)
            .unwrap()
            .1;
        assert_eq!(app.ui.diagram.table_rects.len(), 3);
        for (_, rect) in &app.ui.diagram.table_rects {
            assert!(
                tile.contains_rect(*rect),
                "node {rect:?} outside tile {tile:?}"
            );
        }
    }
    app.close();
}

#[test]
fn record_slider_moves_between_loaded_rows_and_keeps_field_and_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'first'),(2,'second'),(3,'third')",
        ],
        true,
    );
    ctx.enable_accesskit();
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    app.expand_pane(tiling::Kind::Record);
    let sequence = app.sequence;
    let ticket = app.document.result.current;
    for (x_offset, row, value) in [(139.0, 2, "third"), (1.0, 0, "first")] {
        frame(&mut app, &ctx, vec![]);
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        let bounds = control(&output, "Record position");
        click(
            &mut app,
            &ctx,
            egui::pos2(bounds.left() + x_offset, bounds.center().y),
        );
        assert_eq!(app.document.inspect, Some((row, 1)));
        assert_eq!(app.document.tools.detail, Some(row));
        assert_eq!(app.document.tools.cell_editor.input, value);
        assert!(app.document.has_drafts());
        assert_eq!(app.document.result.current, ticket);
        assert_eq!(app.sequence, sequence, "slider must not fetch more rows");
    }
    app.close();
}

#[test]
fn record_header_selects_hidden_fields_without_changing_row_or_drafts() {
    let columns = (0..12)
        .map(|i| format!("field_{i} INTEGER"))
        .collect::<Vec<_>>()
        .join(",");
    let values = (0..12).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let (_dir, ctx, mut app) = fixture(
        &[
            &format!("CREATE TABLE sample(id INTEGER PRIMARY KEY,{columns})"),
            &format!("INSERT INTO sample VALUES(1,{values}),(2,{values})"),
        ],
        true,
    );
    ctx.enable_accesskit();
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.open_record_window(1, 0);
    frame(&mut app, &ctx, vec![]);
    app.stage_cell(1, 2, Value::Integer(999));
    app.document.tools.columns.hidden.insert(5);
    let ticket = app.document.result.current;
    for label in ["Choose field", "field_4"] {
        frame(&mut app, &ctx, vec![]);
        let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        let left = app
            .workspace
            .pane_rects
            .iter()
            .find(|(kind, _)| *kind == tiling::Kind::Record)
            .unwrap()
            .1
            .left();
        let bounds = control_from(&output, label, left);
        click(&mut app, &ctx, bounds.center());
    }
    assert_eq!(app.document.inspect, Some((1, 5)));
    assert_eq!(app.document.result.current, ticket);
    assert!(app.document.has_drafts());
    assert!(app.document.tools.columns.hidden.contains(&5));
    assert_eq!(app.document.tools.cell_editor.input, "4");
    assert!(
        app.document.tools.record_scroll_to.is_none(),
        "record consumed the scroll request"
    );
    app.close();
}

#[test]
fn table_header_selector_browses_tables_and_views_and_protects_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE first_table(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO first_table VALUES(1, 'first')",
            "CREATE TABLE second_table(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO second_table VALUES(2, 'second')",
            "CREATE VIEW table_view AS SELECT * FROM second_table",
        ],
        true,
    );
    ctx.enable_accesskit();
    app.browse_table("first_table".into());
    wait(&mut app, &ctx);
    let first_id = app.document.id;
    let choose = |app: &mut App, name: &str| {
        for label in ["Choose table", name] {
            frame(app, &ctx, vec![]);
            let output = frame_at(app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
            let bounds = control_from(&output, label, app.workspace.area.left());
            click(app, &ctx, bounds.center());
        }
        wait(app, &ctx);
    };
    for name in ["second_table", "table_view", "first_table"] {
        choose(&mut app, name);
        assert_eq!(app.document.object.as_deref(), Some(name));
        assert_eq!(app.document.result.rows.len(), 1);
        assert_eq!(app.pane_document(tiling::Kind::Data), Some(app.document.id));
    }
    assert_eq!(app.document.id, first_id);
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    choose(&mut app, "second_table");
    assert_eq!(app.document.id, first_id);
    assert!(app.document.has_drafts());
    assert!(app.status.contains("Commit or discard drafts"));
    app.close();
}

#[test]
fn fixed_panes_preserve_drafts_selection_and_results_through_layout_changes() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'kept')",
        ],
        true,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    let data = app.document.id;
    let ticket = app.document.result.current;
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    app.document.filter = "body = 'unfinished'".into();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let (_, minimize, _) = *app
        .workspace
        .controls
        .iter()
        .find(|(k, _, _)| *k == tiling::Kind::Map)
        .unwrap();
    click(&mut app, &ctx, minimize.center());
    assert!(app.workspace.minimized.contains(&tiling::Kind::Map));
    frame(&mut app, &ctx, vec![]);
    let map = app
        .workspace
        .pane_rects
        .iter()
        .find(|(k, _)| *k == tiling::Kind::Map)
        .unwrap()
        .1;
    assert!((map.height() - tiling::BAR).abs() < 0.1);
    click(&mut app, &ctx, map.left_center() + egui::vec2(70.0, 0.0));
    assert!(!app.workspace.minimized.contains(&tiling::Kind::Map));
    for kind in tiling::Kind::PANES {
        app.minimize_pane(kind);
    }
    app.expand_pane(tiling::Kind::Record);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.workspace.pane_rects.len(), 1);
    assert_eq!(app.document.id, data);
    assert!(app.document.has_drafts());
    app.expand_pane(tiling::Kind::Record);
    assert_eq!(app.workspace.minimized.len(), 4);
    app.retile();
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.workspace.pane_rects.len(), 5);
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(app.document.result.rows[0][1], Value::Text("kept".into()));
    assert_eq!(app.document.filter, "body = 'unfinished'");
    assert_eq!(app.document.inspect, Some((0, 1)));
    let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
    let data_rect = app
        .workspace
        .pane_rects
        .iter()
        .find(|(k, _)| *k == tiling::Kind::Data)
        .unwrap()
        .1;
    let commit = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.job.text == "Commit" && data_rect.contains(t.pos) => {
                Some(t.pos + egui::vec2(8.0, 6.0))
            }
            _ => None,
        })
        .expect("direct commit action in data dock");
    assert!(!app.document.show_preview);
    click(&mut app, &ctx, commit);
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Text("unsaved".into())
    );
    app.close();
}

#[test]
fn every_tile_exposes_its_actions_without_an_overflow_menu() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'kept')",
        ],
        false,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    ctx.enable_accesskit();
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
                "Run selection / statement at cursor",
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
        app.workspace.expanded = Some(kind);
        app.focus_pane(kind);
        // An expanded wide tile should show all commands without opening any popup.
        frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
        let output = frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
        let tree = output.platform_output.accesskit_update.unwrap();
        let controls = tree
            .nodes
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
        assert!(!controls.contains(&"SQL actions"));
        assert!(!controls.contains(&"Review staged edits"));
        assert!(!controls.contains(&"Discard results"));
        assert!(
            !tree.nodes.iter().any(|(_, node)| {
                node.role() == egui::accesskit::Role::Button
                    && matches!(
                        node.label(),
                        Some("Table data" | "Structure" | "Relationships" | "SQL")
                    )
            }),
            "{kind:?} must not repeat tile navigation buttons"
        );
        for &(pane, minimize, expand) in &app.workspace.controls {
            assert_eq!(pane, kind);
            assert_eq!(minimize.size(), egui::vec2(30.0, 30.0));
            assert_eq!(expand.size(), egui::vec2(30.0, 30.0));
        }
    }
    app.close();
}

#[test]
fn sql_result_ribbon_actions_use_the_linked_result_not_the_editor_or_table() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY)",
            "INSERT INTO sample VALUES(1)",
        ],
        false,
    );
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    let table = app.document.id;
    app.document.tools.columns.hidden.insert(0);
    app.open_sql_window(false);
    let editor = app.document.id;
    app.document.sql = "SELECT 42 AS answer, 'retained' AS text".into();
    app.run_sql();
    wait(&mut app, &ctx);
    let result = app.document.id;
    let ticket = app.document.result.current;
    assert_eq!(app.document.kind, tiling::Kind::Result);
    app.document.inspect = Some((0, 0));
    app.document.tools.columns.hidden.insert(0);
    app.document.tools.columns.manual.insert(0);
    app.focus_pane(tiling::Kind::Sql);
    app.workspace.expanded = Some(tiling::Kind::Sql);
    ctx.enable_accesskit();
    let size = egui::vec2(3440.0, 1440.0);
    let press = |app: &mut App, label: &str| {
        frame_at(app, &ctx, size, vec![]);
        let output = frame_at(app, &ctx, size, vec![]);
        let bounds = control(&output, label);
        let pos = egui::pos2(bounds.center().x, bounds.center().y);
        assert!(app.workspace.pane_rects[0].1.contains(pos));
        for pressed in [true, false] {
            frame_at(
                app,
                &ctx,
                size,
                vec![egui::Event::PointerMoved(pos), pointer_button(pos, pressed)],
            );
        }
    };
    press(&mut app, "Show all result columns");
    assert!(
        app.document_ref(result)
            .unwrap()
            .tools
            .columns
            .hidden
            .is_empty()
    );
    assert!(
        app.document_ref(table)
            .unwrap()
            .tools
            .columns
            .hidden
            .contains(&0)
    );
    press(&mut app, "Fit result columns");
    assert!(
        app.document_ref(result)
            .unwrap()
            .tools
            .columns
            .manual
            .is_empty()
    );
    press(&mut app, "Export query results");
    assert!(app.ui.export_open);
    assert_eq!(app.workspace.focused, result);
    assert_eq!(app.document.id, result);
    assert_eq!(app.document_ref(result).unwrap().result.current, ticket);
    assert_eq!(
        app.document_ref(result).unwrap().result.rows[0][0],
        Value::Integer(42)
    );
    assert_eq!(
        app.document_ref(editor).unwrap().sql,
        "SELECT 42 AS answer, 'retained' AS text"
    );
    app.close();
}

#[test]
fn relationship_zoom_buttons_change_scale_and_percentage_resets_it() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE sample(id INTEGER PRIMARY KEY)"], false);
    app.expand_pane(tiling::Kind::Map);
    ctx.enable_accesskit();
    let press = |app: &mut App, label: &str| {
        frame(app, &ctx, vec![]);
        let output = frame_at(app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
        let bounds = control(&output, label);
        click(app, &ctx, egui::pos2(bounds.center().x, bounds.center().y));
    };
    press(&mut app, "Reset zoom to 100%");
    assert_eq!(app.ui.diagram.camera.zoom, 1.0);
    press(&mut app, "Zoom in");
    assert_eq!(app.ui.diagram.camera.zoom, 1.25);
    let output = frame_at(&mut app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
    assert!(output.shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Text(t) if t.galley.job.text == "125%")));
    press(&mut app, "Zoom out");
    assert_eq!(app.ui.diagram.camera.zoom, 1.0);
    press(&mut app, "Zoom out");
    assert_eq!(app.ui.diagram.camera.zoom, 0.8);
    press(&mut app, "Reset zoom to 100%");
    assert_eq!(app.ui.diagram.camera.zoom, 1.0);
    app.close();
}

#[test]
fn redesigned_dialog_actions_remain_visible_at_minimum_and_laptop_sizes() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO sample VALUES(1,'stored')",
        ],
        true,
    );
    ctx.enable_accesskit();
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("local draft".into()));
    for dark in [true, false] {
        for (size, font_size) in [
            (egui::vec2(720.0, 480.0), 14),
            (egui::vec2(1366.0, 768.0), 20),
        ] {
            app.preferences.appearance = Some(crate::appearance::Mode::from_dark(dark));
            app.preferences.font_size = font_size;
            app.apply_theme(&ctx);
            for (scene, label) in [
                (0, "COMMIT staged edits to database"),
                (1, "Stage insert (no database write)"),
                (2, "Close cleanly and reopen"),
                (3, "Wait, roll back transaction, and close"),
                (4, "Export retained result…"),
                (5, "Copy stored value"),
            ] {
                app.document.show_preview = scene == 0;
                app.document.show_insert = false;
                if scene == 1 {
                    app.begin_insert();
                }
                app.ui.access_dialog = scene == 2;
                app.confirm_close = scene == 3;
                app.ui.export_open = scene == 4;
                app.document.ui.inspector_detached = scene == 5;
                app.document.ui.inspector_open = scene == 5;
                app.document.ui.inspector = shell::DetailMode::Cell;
                app.document.inspect = Some((0, 1));
                for _ in 0..3 {
                    frame_at(&mut app, &ctx, size, vec![]);
                }
                let output = frame_at(&mut app, &ctx, size, vec![]);
                let rect = control(&output, label);
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                    "{label} outside {size:?} with font {font_size}: {rect:?}"
                );
                assert_eq!(app.document.result.rows[0][1], Value::Text("stored".into()));
                assert!(app.has_drafts());
            }
        }
    }
    app.close();
}
