use super::*;
use std::time::{Duration, Instant};
use tursobench_core::Budget;
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
            && app.active.is_none()
            && app.catalog_ticket.is_none()
            && app.commit_ticket.is_none()
            && app.tools.inspection_ticket.is_none()
            && app.ui.diagram.ticket.is_none()
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
        std::thread::sleep(Duration::from_millis(5));
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
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ui| app.render(ui),
    );
    out.textures_delta.clear();
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
    let started = Instant::now();
    while app.session.is_some() {
        app.poll(&ctx);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!app.ready && !app.closing && !app.confirm_close);
    assert!(app.active.is_none());
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
    let started = Instant::now();
    while app.session.is_some() {
        app.poll(&ctx);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
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
fn typing_a_different_database_path_resets_consent() {
    let ctx = egui::Context::default();
    let mut app = App::new(&ctx);
    app.path = "previous.db".into();
    app.exclusive = true;
    app.writable = true;
    app.databases.editing_path = true;
    frame(&mut app, &ctx, vec![]);
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("database-path")));
    frame(&mut app, &ctx, vec![egui::Event::Text("x".into())]);
    assert_ne!(app.path, "previous.db");
    assert!(app.path.contains('x'));
    assert!(!app.exclusive && !app.writable);
}

#[test]
fn diagram_preserves_drafts_and_result_ownership_and_chrome_fits_both_themes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, "scratchpad")).unwrap();
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("buffer_ids".into());
    wait(&mut app, &ctx);
    let retained = app.result.rows.clone();
    let ticket = app.result.current;
    app.stage_cell(0, 0, Value::Text("pending diagram edit".into()));
    assert!(app.has_drafts());
    app.ui.workspace = shell::Workspace::Diagram;
    app.show_structure();
    app.load_diagram();
    wait(&mut app, &ctx);
    assert_eq!(app.result.current, ticket);
    assert_eq!(app.result.rows, retained);
    assert!(app.has_drafts());
    assert!(
        app.ui
            .diagram
            .data
            .as_ref()
            .unwrap()
            .tables
            .iter()
            .any(|t| t.name == "buffer_ids")
    );
    let draft = format!("{:?}", app.drafts);
    for dark in [true, false] {
        app.preferences.dark_mode = dark;
        app.apply_theme(&ctx);
        for (width, height, scale) in [
            (960.0, 640.0, 1.0),
            (1280.0, 720.0, 1.0),
            (1366.0, 768.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (3440.0, 1440.0, 1.0),
            (1440.0, 960.0, 1.5),
        ] {
            ctx.set_pixels_per_point(scale);
            for mode in [
                shell::Workspace::Query,
                shell::Workspace::Data,
                shell::Workspace::Diagram,
            ] {
                app.ui.workspace = mode;
                app.ui.diagram.fit = true;
                for _ in 0..2 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            ..Default::default()
                        },
                        |ui| app.render(ui),
                    );
                    output.textures_delta.clear();
                    assert!(!output.shapes.is_empty());
                    assert_eq!(app.ui.transaction_rects.len(), 3);
                    for rect in &app.ui.transaction_rects {
                        assert!(
                            ctx.content_rect().contains_rect(*rect),
                            "{mode:?} {width}x{height} @{scale}: {rect:?}"
                        );
                    }
                    assert!(
                        app.ui.workspace_rect.width() >= 500.0,
                        "{mode:?}: {:?}",
                        app.ui.workspace_rect
                    );
                }
            }
        }
    }
    assert_eq!(format!("{:?}", app.drafts), draft);
    assert_eq!(app.result.rows, retained);
    ctx.set_pixels_per_point(1.0);
    app.ui.workspace = shell::Workspace::Diagram;
    app.ui.diagram.fit = true;
    frame(&mut app, &ctx, vec![]);
    let before_selection = app.ui.diagram.selected.clone();
    let (name, rect) = app.ui.diagram.table_rects[0].clone();
    let pos = rect.center();
    for pressed in [true, false] {
        frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(app.ui.diagram.selected, before_selection);
    assert_eq!(app.ui.inspector, shell::Inspector::Structure);
    // Diagram navigation must not bypass the draft guard.
    app.browse_table(name.clone());
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert!(app.has_drafts());
    app.drafts = None;
    app.browse_table(name.clone());
    wait(&mut app, &ctx);
    assert_eq!(app.table.as_ref().unwrap().name, name);
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert!(app.ui.drawer.open);
    app.close();
    while app.session.is_some() {
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn reverted_empty_batch_cannot_carry_identity_to_another_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY,value TEXT)",
            "INSERT INTO a VALUES(1,'same')",
            "INSERT INTO b VALUES(1,'same')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("temporary".into()));
    app.drafts.as_mut().unwrap().revert_row(&app.result.rows[0]);
    assert!(!app.has_drafts());
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert_eq!(app.drafts.as_ref().unwrap().table.name, "b");
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert_eq!(app.result.rows[0][1], Value::Text("only b".into()));
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.result.rows[0][1], Value::Text("same".into()));
    app.sql = "BEGIN".into();
    app.run_sql();
    wait(&mut app, &ctx);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Active);
    assert!(!app.can_edit());
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert!(!app.has_drafts());
    app.sql = "ROLLBACK".into();
    app.run_sql();
    wait(&mut app, &ctx);
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    let retained = app.result.rows.clone();
    app.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.session.is_some() {
        assert!(Instant::now() < deadline);
        app.poll(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.status.contains("cleanup confirmed"));
    app.ui.workspace = shell::Workspace::Data;
    app.first_cell_rect = None;
    frame(&mut app, &ctx, vec![]);
    assert!(app.first_cell_rect.is_some());
    assert_eq!(app.result.rows, retained);
    app.copy_results(&ctx, crate::export::Format::Json);
    assert!(app.status.contains("Copied retained"));
    app.browse_table("a".into());
    assert_eq!(app.table.as_ref().unwrap().name, "b");
    assert_eq!(app.result.rows, retained);
    app.ui.workspace = shell::Workspace::Query;
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
        assert_eq!(app.result.rows.len(), 100);
        assert!(!app.can_edit());
        frame(&mut app, &ctx, vec![]);
        let export = dir.path().join("page.jsonl");
        crate::export::write_result_as(
            &export,
            crate::export::Format::JsonLines,
            "table-page",
            &app.result.columns,
            &app.result.rows,
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
            app.sql =
                "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)".into();
            app.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
            app.run_sql();
            wait(&mut app, &ctx);
            assert_eq!(
                app.result.rows,
                vec![vec![Value::Text("entry-0000".into())]]
            );
            frame(&mut app, &ctx, vec![]);
            crate::export::write_result_as(
                &dir.path().join("fts.jsonl"),
                crate::export::Format::JsonLines,
                "complete-query-result",
                &app.result.columns,
                &app.result.rows,
                "t",
            )
            .unwrap();
        }
        app.close();
        let start = Instant::now();
        while app.session.is_some() {
            app.poll(&ctx);
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.status.contains("cleanup confirmed"));
    }
}

#[test]
fn click_inspect_stage_no_write_failed_commit_retains_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO items VALUES (1, 'original'), (2, 'other')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("items".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
    );
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    );
    assert_eq!(app.inspect, Some((0, 0)));
    app.clone_row(0);
    let clone: Vec<Option<Value>> = serde_json::from_str(&app.insert_json).unwrap();
    assert_eq!(clone, vec![None, Some(Value::Text("original".into()))]);
    assert!(!app.has_drafts());
    app.show_insert = false;
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let original = app.result.rows[0].clone();
    assert_eq!(
        app.drafts.as_ref().unwrap().displayed(&original, 1),
        Some(&Value::Text("draft".into()))
    );
    app.drafts.as_mut().unwrap().revert_cell(&original, 1);
    assert!(!app.has_drafts());
    app.stage_delete(0);
    assert!(app.drafts.as_ref().unwrap().deleted(&original));
    app.drafts.as_mut().unwrap().revert_row(&original);
    assert!(!app.has_drafts());
    app.stage_cell(0, 0, Value::Integer(2));
    assert!(app.has_drafts());
    assert!(app.commit_ticket.is_none());
    // Inspecting, rendering and staging produced no SQL request or row mutation.
    assert_eq!(app.result.rows[0][0], Value::Integer(1));
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
    assert_eq!(app.result.rows[1][0], Value::Integer(3));
    app.close();
}

#[test]
fn selection_named_history_metadata_reference_back_and_tools_render() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("navigation.db");
    crate::test_support::run(async {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        for sql in [
            "CREATE TABLE parent(a INTEGER,b TEXT,payload TEXT,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'001','{\"x\":1}')",
            "INSERT INTO child VALUES(1,7,'001'),(2,NULL,NULL)",
        ] {
            c.execute(sql, ()).await.unwrap();
        }
    });
    let (ctx, mut app) = connected(&path, false);
    assert!(
        app.schema
            .iter()
            .any(|o| o.name == "child" && o.ddl.as_ref().unwrap().contains("FOREIGN KEY"))
    );
    app.browse_table("child".into());
    wait(&mut app, &ctx);
    assert!(app.tools.inspection_error.is_empty());
    let key = app.tools.inspection.as_ref().unwrap().foreign_keys[0].clone();
    assert!(app.cell_references(1, 1).is_empty());
    app.ui.drawer.expanded = true;
    frame(&mut app, &ctx, vec![]);
    let pos = app
        .fk_cell_rect
        .expect("composite FK rendered as a link")
        .center();
    for pressed in [true, false] {
        frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    wait(&mut app, &ctx);
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("parent"));
    assert_eq!(app.table.as_ref().unwrap().name, "parent");
    assert_eq!(app.result.rows.len(), 1);
    assert_eq!(app.export_scope(), "related-row-lookup");
    app.follow_incoming(0, "child".into(), key.clone());
    wait(&mut app, &ctx);
    assert_eq!(app.table.as_ref().unwrap().name, "child");
    assert_eq!(app.result.rows.len(), 1);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("parent"));
    app.tools.detail = Some(0);
    app.tools.show_metadata = true;
    app.tools.help = true;
    app.tools.ddl = app.schema.iter().find(|o| o.name == "parent").cloned();
    frame(&mut app, &ctx, vec![]);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.table.as_ref().unwrap().name, "child");
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("child"));
    assert_eq!(app.result.rows.len(), 2);
    app.follow_key(1, key);
    assert!(app.status.contains("NULL foreign key"));
    assert!(app.tools.related.is_none());
    app.sql = "SELECT 'λ;'; SELECT :name AS value;".into();
    let byte = app.sql.find("SELECT :name").unwrap();
    let cursor = app.sql[..byte].chars().count();
    app.tools.selection = cursor..cursor;
    app.params = r#"{":name":{"type":"Text","value":"bound-only-secret"}}"#.into();
    app.run_selection();
    wait(&mut app, &ctx);
    assert_eq!(
        app.result.rows,
        vec![vec![Value::Text("bound-only-secret".into())]]
    );
    assert!(app.sql.contains("λ;"));
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
    app.sql = "SELECT * FROM parent".into();
    app.params = "[]".into();
    app.run_text(app.sql.clone(), Some(false));
    wait(&mut app, &ctx);
    assert_eq!(app.view, View::Explain);
    assert!(!app.result.rows.is_empty());
    app.tools.complete = true;
    frame(&mut app, &ctx, vec![]);
    app.close();
}

#[test]
fn map_home_drawer_preserves_camera_and_rejects_navigation_with_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("map-home.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, value TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1, 'original')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert!(!app.ui.drawer.open);
    assert_eq!(app.ui.diagram.data.as_ref().unwrap().tables.len(), 2);
    frame(&mut app, &ctx, vec![]);
    let camera = (app.ui.diagram.zoom, app.ui.diagram.pan);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    assert!(app.ui.drawer.open);
    assert_eq!(app.result.rows[0][1], Value::Text("original".into()));
    let retained_ticket = app.result.current;
    app.refresh_catalog();
    wait(&mut app, &ctx);
    assert_eq!(app.result.current, retained_ticket);
    assert_eq!(app.result.rows[0][1], Value::Text("original".into()));
    for expanded in [false, true, false] {
        app.ui.drawer.expanded = expanded;
        frame(&mut app, &ctx, vec![]);
        assert_eq!((app.ui.diagram.zoom, app.ui.diagram.pan), camera);
    }
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = app.result.current;
    app.browse_table("b".into());
    assert_eq!(app.result.current, ticket);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("a"));
    assert_eq!(app.table.as_ref().unwrap().name, "a");
    app.drafts = None;
    app.close();
}

#[test]
fn rapid_map_selection_keeps_latest_rows_and_structure_including_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selection.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "INSERT INTO b VALUES(2)",
            "CREATE VIEW v AS SELECT * FROM b",
            "CREATE TRIGGER b_insert AFTER INSERT ON b BEGIN SELECT 1; END",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("a".into());
    app.browse_table("b".into());
    wait(&mut app, &ctx);
    assert_eq!(app.result.rows, vec![vec![Value::Integer(2)]]);
    assert_eq!(app.table.as_ref().unwrap().name, "b");
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("b"));
    assert!(
        app.schema
            .iter()
            .any(|o| o.kind == "trigger" && o.table == "b")
    );
    app.ui.map_list = true;
    app.browse_table("v".into());
    wait(&mut app, &ctx);
    assert_eq!(app.tools.inspection.as_ref().unwrap().table.name, "v");
    assert!(!app.can_edit());
    app.ui.drawer.view = shell::TableView::Structure;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.ui.diagram.selected.as_deref(), Some("v"));
    app.close();
}

#[test]
fn sql_drawer_keeps_result_identity_and_read_transactions_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sql-drawer.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    frame(&mut app, &ctx, vec![]);
    assert!(app.ui.transaction_rects.is_empty());
    app.toggle_sql();
    app.run_text("SELECT 42 AS answer".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert_eq!(app.view, View::Query);
    assert!(app.table.is_none());
    assert_eq!(app.result.rows, vec![vec![Value::Integer(42)]]);
    let ticket = app.result.current;
    app.toggle_sql();
    assert!(!app.ui.sql_open);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.result.current, ticket);
    app.toggle_sql();
    app.run_text("BEGIN".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Active);
    app.toggle_sql();
    app.ui.drawer.open = false;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.ui.transaction_rects.len(), 3);
    app.run_text("ROLLBACK".into(), None);
    wait(&mut app, &ctx);
    assert_eq!(app.transaction, TransactionState::Autocommit);
    app.open_palette();
    frame(&mut app, &ctx, vec![]);
    app.ui.palette_query = "a.id".into();
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    wait(&mut app, &ctx);
    assert_eq!(app.table.as_ref().unwrap().name, "a");
    assert_eq!(app.tools.highlight_column, Some(("a".into(), "id".into())));
    assert!(!app.ui.palette_open);
    app.close();
}

#[test]
fn access_switch_waits_for_closed_then_opens_a_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.db");
    crate::test_support::create(&path, &["CREATE TABLE a(id INTEGER PRIMARY KEY)"]);
    let (ctx, mut app) = connected(&path, false);
    let old_session = app.session.as_ref().unwrap().id;
    app.ui.diagram.zoom = 0.7;
    app.ui.diagram.pan = egui::vec2(30.0, 42.0);
    app.pending_access = Some(true);
    app.close();
    assert!(app.closing);
    assert!(!app.writable);
    wait(&mut app, &ctx);
    assert!(app.writable);
    assert_ne!(app.session.as_ref().unwrap().id, old_session);
    assert_eq!(app.ui.workspace, shell::Workspace::Diagram);
    assert!(app.pending_access.is_none());
    assert_eq!(app.ui.diagram.zoom, 0.7);
    assert_eq!(app.ui.diagram.pan, egui::vec2(30.0, 42.0));
    app.close();
}

#[test]
fn drawer_headers_fit_small_large_and_scaled_viewports_in_both_themes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drawer-viewports.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, text TEXT)",
            "INSERT INTO a VALUES(1,'value')",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    for dark in [true, false] {
        app.preferences.dark_mode = dark;
        app.apply_theme(&ctx);
        for (width, height, scale) in [
            (960.0, 640.0, 1.0),
            (1280.0, 720.0, 1.0),
            (1366.0, 768.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (3440.0, 1440.0, 1.0),
            (1440.0, 960.0, 1.5),
        ] {
            ctx.set_pixels_per_point(scale);
            for expanded in [false, true] {
                for structure in [false, true] {
                    app.ui.drawer.expanded = expanded;
                    app.ui.drawer.view = if structure {
                        shell::TableView::Structure
                    } else {
                        shell::TableView::Data
                    };
                    for _ in 0..2 {
                        let mut out = ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, height),
                                )),
                                ..Default::default()
                            },
                            |ui| app.render(ui),
                        );
                        out.textures_delta.clear();
                    }
                    assert!(
                        ctx.content_rect().contains_rect(app.ui.drawer_header),
                        "{width}x{height}, expanded={expanded}: {:?}",
                        app.ui.drawer_header
                    );
                    assert!(app.ui.drawer_header.width() > 200.0);
                }
            }
        }
    }
    app.close();
}
