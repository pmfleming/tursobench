use super::*;
use std::time::{Duration, Instant};
use tursobench_core::{Budget, Value};
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
fn frame_at(
    app: &mut App,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| app.render(ui),
    );
    output.textures_delta.clear();
    output
}
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    frame_at(app, ctx, egui::vec2(1400.0, 900.0), events);
}

#[test]
fn sql_selection_shortcut_and_window_expansion_have_distinct_targets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ribbon.db");
    crate::test_support::create(&path, &["CREATE TABLE sample(id INTEGER PRIMARY KEY)"]);
    let (ctx, mut app) = connected(&path, false);
    app.open_sql_window(false);
    let editor = app.document.id;
    app.document.sql = "SELECT 73 AS answer".into();
    frame(&mut app, &ctx, vec![]);
    let key = |modifiers| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT)],
    );
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows, vec![vec![Value::Integer(73)]]);
    assert_eq!(app.document.kind, tiling::Kind::Result);
    assert_eq!(app.workspace.expanded, None);
    assert_eq!(app.document_ref(editor).unwrap().sql, "SELECT 73 AS answer");
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Modifiers::CTRL | egui::Modifiers::ALT)],
    );
    assert_eq!(app.workspace.expanded, Some(app.document.id));
    assert!(app.ui.sidebar_rect.right() <= app.workspace.area.left());
    app.focus_window(editor);
    app.request_close_window(editor);
    assert_eq!(
        app.workspace.close_pending,
        Some(editor),
        "closing an SQL draft requires an in-app choice"
    );
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.workspace.close_pending, None);
    assert!(app.document_ref(editor).is_some());
    app.close();
}

#[test]
fn independent_documents_route_results_and_keep_filters_drafts_and_sql() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("documents.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, body TEXT)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY, body TEXT)",
            "INSERT INTO a VALUES(1, 'alpha')",
            "INSERT INTO b VALUES(2, 'beta')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
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
    // A closed view's late delivery cannot append to another view's rows.
    app.documents.remove(&a_id);
    app.handle_event(
        &ctx,
        Event {
            ticket: a_ticket,
            kind: EventKind::Rows(vec![vec![Value::Integer(99)]]),
        },
    );
    assert_eq!(app.document.result.rows.len(), 1);
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
    let started = Instant::now();
    while app.session.is_some() {
        app.poll(&ctx);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
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
    assert_eq!(app.document.kind, tiling::Kind::Structure);
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
            for rect in [app.ui.sidebar_rect, app.ui.rail_rect] {
                assert_eq!(rect.top(), viewport.top());
                assert_eq!(rect.bottom(), viewport.bottom());
            }
            for rect in &app.ui.ribbon_actions {
                assert!(
                    viewport.contains_rect(*rect),
                    "ribbon action outside viewport: {rect:?}"
                );
                assert!(rect.left() >= app.ui.rail_rect.right());
                assert!(rect.height() >= 40.0);
            }
            for (_, rect) in &app.workspace.tiles {
                assert!(app.workspace.area.contains_rect(*rect));
            }
            let saved = app.workspace.tree.clone();
            app.expand_window(map);
            frame_at(&mut app, &ctx, egui::vec2(width, height), vec![]);
            assert_eq!(app.workspace.tiles.len(), 1);
            assert_eq!(app.workspace.tiles[0].0, map);
            assert!(app.ui.sidebar_rect.right() <= app.workspace.tiles[0].1.left());
            app.expand_window(map);
            assert_eq!(app.workspace.tree, saved);
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
    assert_eq!(app.document.inspect, Some((0, 0)));
    app.clone_row(0);
    let clone: Vec<Option<Value>> = serde_json::from_str(&app.document.insert_json).unwrap();
    assert_eq!(clone, vec![None, Some(Value::Text("original".into()))]);
    assert!(!app.has_drafts());
    app.document.show_insert = false;
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let original = app.document.result.rows[0].clone();
    assert_eq!(
        app.document
            .drafts
            .as_ref()
            .unwrap()
            .displayed(&original, 1),
        Some(&Value::Text("draft".into()))
    );
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
    assert!(app.document.tools.inspection_error.is_empty());
    let key = app.document.tools.inspection.as_ref().unwrap().foreign_keys[0].clone();
    assert!(app.cell_references(1, 1).is_empty());
    app.document.ui.nav = shell::Nav::Table;
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
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert_eq!(app.ui.diagram.data.as_ref().unwrap().tables.len(), 2);
    frame(&mut app, &ctx, vec![]);
    // Explicit user camera choices survive opening/resizing other views.
    app.ui.diagram.freeze_camera();
    let camera = (app.ui.diagram.zoom, app.ui.diagram.pan);
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
        assert_eq!((app.ui.diagram.zoom, app.ui.diagram.pan), camera);
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
    assert_eq!(app.document.ui.nav, shell::Nav::Overview);
    assert!(app.pending_access.is_none());
    assert_eq!(app.ui.diagram.zoom, 0.7);
    assert_eq!(app.ui.diagram.pan, egui::vec2(30.0, 42.0));
    app.close();
}

#[test]
fn navigation_restores_typed_browse_context_and_query_return_without_dropping_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("navigation.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO a VALUES(1,'first'),(2,'second')",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
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
fn sidebar_catalog_includes_views_when_map_is_incomplete_and_tools_preserve_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog-sidebar.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE example(id INTEGER PRIMARY KEY)",
            "INSERT INTO example VALUES(1)",
            "CREATE VIEW example_view AS SELECT * FROM example",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
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
        app.select_tool(panel);
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("count.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY,value TEXT)",
            "INSERT INTO a VALUES(1,'original'),(2,'second')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
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
                assert!(
                    ctx.content_rect().contains_rect(app.ui.view_header),
                    "{width}x{height}: {:?}",
                    app.ui.view_header
                );
                assert!(app.ui.view_header.width() > 200.0);
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
                        app.ui.visible_rows >= 15,
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "INSERT INTO a VALUES(1)",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("a".into());
    wait(&mut app, &ctx);
    let ticket = app.document.result.current;
    let popup = egui::Id::new("test-menu");
    egui::Popup::open_id(&ctx, popup);
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(!egui::Popup::is_any_open(&ctx));
    egui::Popup::open_id(&ctx, popup);
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::K, egui::Modifiers::CTRL)],
    );
    assert!(app.ui.palette_open);
    assert!(!egui::Popup::is_any_open(&ctx));
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(!app.ui.palette_open);
    app.document.ui.sql_open = true;
    app.document.ui.filter_open = true;
    frame(
        &mut app,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(!app.document.ui.filter_open);
    assert!(app.document.ui.sql_open);
    assert_eq!(app.document.result.current, ticket);
    app.close();
}

#[test]
fn column_presentation_preserves_schema_indices_and_manual_widths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("columns.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, label TEXT, number INTEGER)",
            "INSERT INTO sample VALUES(1,'a long string',42)",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.document.tools.columns.move_before(2, 0);
    app.document.tools.columns.hidden.insert(1);
    app.document.tools.columns.manual.insert(2);
    app.document.tools.columns.widths[2] = 217.0;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.visible_columns(), vec![2, 0]);
    assert_eq!(app.document.tools.columns.widths[2], 217.0);
    app.open_cell(0, 2);
    assert_eq!(app.document.tools.cell_input, "42");
    app.stage_cell(0, 2, Value::Integer(55));
    let draft = app.document.drafts.as_ref().unwrap();
    assert_eq!(
        draft.displayed(&app.document.result.rows[0], 2),
        Some(&Value::Integer(55))
    );
    assert_eq!(app.document.result.rows[0][0], Value::Integer(1));
    app.close();
}

#[test]
fn range_selection_uses_visible_order_and_exact_numeric_values_and_inline_stages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selection.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER,text TEXT)",
            "INSERT INTO sample VALUES(1,9007199254740993,''),(2,9007199254740995,'two')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.select_cell(0, 1, false);
    app.select_cell(1, 1, true);
    assert_eq!(app.selected_cells(), vec![(0, 1), (1, 1)]);
    let summary = app.selection_summary();
    assert!(summary.contains("18014398509481988"), "{summary}");
    app.document.tools.columns.move_before(2, 1);
    app.select_cell(0, 2, false);
    app.select_cell(1, 1, true);
    assert_eq!(app.selected_cells(), vec![(0, 2), (0, 1), (1, 2), (1, 1)]);
    app.start_inline(0, 1);
    assert!(app.document.tools.inline.is_some());
    let edit = app.document.tools.inline.as_mut().unwrap();
    edit.input = "123".into();
    let value = edit.kind.parse(&edit.input).unwrap();
    app.stage_cell(0, 1, value);
    app.document.tools.inline = None;
    assert_eq!(
        app.document.result.rows[0][1],
        Value::Integer(9007199254740993)
    );
    assert_eq!(
        app.document
            .drafts
            .as_ref()
            .unwrap()
            .displayed(&app.document.result.rows[0], 1),
        Some(&Value::Integer(123))
    );
    app.close();
}

#[test]
fn scroll_pages_append_evict_reload_and_reject_stale_or_draft_owned_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pages.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER)",
            "INSERT INTO sample WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<450) SELECT x,x FROM n",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
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
    app.close();
}

#[test]
fn contextual_reads_and_wide_related_grid_preserve_parent_result_and_selection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("context.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT COLLATE NOCASE,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a TEXT,b TEXT,payload TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'Alpha')",
            "INSERT INTO child VALUES(1,'007','alpha','child-only')",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("child".into());
    wait(&mut app, &ctx);
    let child = app.document.tools.inspection.as_ref().unwrap().foreign_keys[0].clone();
    let owner = app.document.result.current;
    let original = app.document.result.rows.clone();
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    wait(&mut app, &ctx);
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER,text TEXT)",
            "INSERT INTO sample VALUES(1,9007199254740993,'')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
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
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let send = |app: &mut App, mods, mut events: Vec<egui::Event>| {
        events.insert(0, egui::Event::ModifiersChanged(mods));
        frame_at(app, &ctx, egui::vec2(1280.0, 720.0), events)
    };
    send(
        &mut app,
        egui::Modifiers::SHIFT,
        vec![key(egui::Key::ArrowRight, egui::Modifiers::SHIFT)],
    );
    assert_eq!(app.selected_cells(), vec![(0, 0), (0, 1)]);
    let output = send(
        &mut app,
        egui::Modifiers::CTRL,
        vec![key(egui::Key::C, egui::Modifiers::CTRL)],
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
        vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
    );
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(app.document.tools.inline.is_some());
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![
            egui::Event::Text("draft".into()),
            key(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert!(app.document.tools.inline.is_none());
    assert!(app.has_drafts());
    assert_eq!(app.document.result.rows[0][2], Value::Text(String::new()));
    app.close();
}

#[test]
fn token_entry_enter_retains_typed_filter_and_empty_clipboard_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,n INTEGER,text TEXT)",
            "INSERT INTO sample VALUES(1,1,''),(2,2,'two')",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    app.document.tools.filter_focus = true;
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("n>=2".into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    wait(&mut app, &ctx);
    assert_eq!(app.document.result.rows.len(), 1);
    assert_eq!(app.document.result.rows[0][0], Value::Integer(2));
    assert_eq!(
        app.document.browse.as_ref().unwrap().filters[0].op,
        tursobench_core::FilterOp::GreaterOrEqual
    );
    app.document.browse.as_mut().unwrap().filters.clear();
    app.reload();
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.document.tools.columns.move_before(2, 0);
    app.select_cell(0, 2, false);
    app.select_cell(0, 0, true);
    assert_eq!(app.selection_tsv().unwrap(), "\t1");
    app.close();
}

#[test]
fn clipboard_grid_and_record_use_independent_tiles_without_overlays() {
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
    assert!(app.ui.visible_rows >= 33, "{} rows", app.ui.visible_rows);
    let data_id = app.document.id;
    app.open_record_window(0, 0);
    assert_eq!(app.document.kind, tiling::Kind::Record);
    assert_eq!(app.document.source, Some(data_id));
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(2560.0, 1440.0), vec![]);
    }
    assert!(!app.document.ui.inspector_open);
    app.close();
}

#[test]
fn incomplete_page_keeps_primary_rows_and_does_not_invent_end_of_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page-error.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY)",
            "INSERT INTO sample WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<250) SELECT x FROM n",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    let original = app.document.result.rows.clone();
    let owner = app.document.result.current;
    app.request_page(false);
    let ticket = app
        .document
        .tools
        .pages
        .pending
        .as_ref()
        .unwrap()
        .result
        .current
        .unwrap();
    app.handle_event(
        &ctx,
        Event {
            ticket,
            kind: EventKind::Finished(tursobench_core::Completion {
                elapsed_ms: 1,
                delivery: tursobench_core::Delivery::Limited,
                rows_seen: 100,
                changed: 0,
                transaction: TransactionState::Autocommit,
                error: None,
            }),
        },
    );
    assert!(app.document.tools.pages.pending.is_none());
    assert!(!app.document.tools.pages.error.is_empty());
    assert!(app.document.tools.pages.end.is_none());
    assert_eq!(app.document.result.rows, original);
    assert_eq!(app.document.result.current, owner);
    app.close();
}

#[test]
fn query_back_restores_a_multi_page_window_before_matching_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("back-window.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY)",
            "INSERT INTO sample WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<600) SELECT x FROM n",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
    app.preferences.row_limit = 400;
    app.browse_table("sample".into());
    wait(&mut app, &ctx);
    for _ in 0..2 {
        app.request_page(false);
        wait(&mut app, &ctx);
    }
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
    app.close();
}

#[test]
fn filtered_index_tree_opens_original_ddl_without_replacing_primary_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ddl-tree.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE INDEX sample_value ON sample(value)",
            "INSERT INTO sample VALUES(1,'one')",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
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
                (t.galley.job.text == "sample_value").then_some(t.pos + egui::vec2(8.0, 6.0))
            } else {
                None
            }
        })
        .expect("filtered index row");
    let data_id = app.document.id;
    let owner = app.document.result.current;
    let rows = app.document.result.rows.clone();
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
    let session = app.session.as_ref().unwrap().id;
    app.pending_database = Some(dir.path().join("other.db"));
    app.request_close();
    assert!(app.confirm_close);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(app.pending_database.is_none());
    assert!(!app.confirm_close);
    assert!(!app.closing);
    assert_eq!(app.session.as_ref().unwrap().id, session);
    assert_eq!(app.path, path.to_string_lossy());
    app.close();
}

#[test]
fn workspace_geometry_roundtrips_without_query_data_or_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE chunks(id TEXT PRIMARY KEY, part INTEGER, body BLOB)",
            "INSERT INTO chunks VALUES('buffer-1780913432386559807-118',0,x'001122')",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
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
    assert_eq!(app.export_scope(), "pinned-record");
    assert!(
        app.document.result.current.is_none(),
        "pinned records never own a worker ticket"
    );
    app.focus_window(data_id);
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.open_sql_window(false);
    let editor = app.document.id;
    app.document.sql = "SELECT 'private-literal'".into();
    app.document.params = "[\"private-binding\"]".into();
    app.retile();
    app.expand_window(editor);
    let snapshot = app.workspace_snapshot().unwrap();
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("private") && !encoded.contains("buffer-"));
    app.preferences.workspace_layouts = vec![serde_json::from_str(&encoded).unwrap()];
    app.restore_workspace();
    assert_eq!(app.workspace_snapshot(), Some(snapshot));
    assert_eq!(app.document.id, editor);
    assert!(app.document.sql.is_empty());
    assert_eq!(app.document.params, "[]");
    assert_eq!(app.workspace.expanded, Some(editor));
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
    let start = Instant::now();
    while app.session.is_some() {
        app.poll(&ctx);
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
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
    assert_eq!(app.workspace.expanded, Some(editor));
    app.close();
}

#[test]
fn automatic_map_fit_keeps_every_node_inside_its_resized_tile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("map-fit.db");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY)",
            "CREATE TABLE c(id INTEGER PRIMARY KEY)",
        ],
    );
    let (ctx, mut app) = connected(&path, false);
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
fn wide_tables_open_below_and_header_moves_keep_drafts_and_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moving.db");
    let columns = (0..12)
        .map(|n| format!("field{n} TEXT"))
        .collect::<Vec<_>>()
        .join(",");
    crate::test_support::create(
        &path,
        &[
            "CREATE TABLE narrow(id INTEGER PRIMARY KEY)",
            &format!("CREATE TABLE wide(id INTEGER PRIMARY KEY, {columns})"),
            "INSERT INTO wide(id,field0) VALUES(1,'kept')",
        ],
    );
    let (ctx, mut app) = connected(&path, true);
    let size = egui::vec2(1800.0, 1000.0);
    let map = app.document.id;
    frame_at(&mut app, &ctx, size, vec![]);
    app.browse_table("narrow".into());
    wait(&mut app, &ctx);
    frame_at(&mut app, &ctx, size, vec![]);
    app.browse_table("wide".into());
    let wide = app.document.id;
    let tree = app.workspace.tree.clone();
    frame_at(&mut app, &ctx, size, vec![]);
    let rect = app
        .workspace
        .tiles
        .iter()
        .find(|(id, _)| *id == wide)
        .unwrap()
        .1;
    assert_eq!(
        rect.width(),
        app.workspace.area.width(),
        "schema width should place the table below the whole group before receiving rows"
    );
    wait(&mut app, &ctx);
    assert_eq!(
        app.workspace.tree, tree,
        "streaming data never rearranges windows"
    );
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    app.document.filter = "field0 = 'unfinished'".into();
    let ticket = app.document.result.current;
    let rows = app.document.result.rows.clone();
    for _ in 0..2 {
        frame_at(&mut app, &ctx, size, vec![]);
    }
    let header = app
        .workspace
        .headers
        .iter()
        .find(|(id, _)| *id == wide)
        .unwrap()
        .1
        .center();
    let target = app
        .workspace
        .tiles
        .iter()
        .find(|(id, _)| *id == map)
        .unwrap()
        .1
        .center();
    let pointer_button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![
            egui::Event::PointerMoved(header),
            pointer_button(header, true),
        ],
    );
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(header + egui::vec2(20.0, 0.0))],
    );
    assert_eq!(app.workspace.dragging, Some(wide));
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(target)],
    );
    assert_eq!(app.workspace.tree, tree, "preview cannot mutate the tree");
    frame_at(&mut app, &ctx, size, vec![pointer_button(target, false)]);
    assert_eq!(app.workspace.dragging, None);
    assert_ne!(app.workspace.tree, tree);
    assert_eq!(app.workspace.focused, wide);
    assert!(app.document.has_drafts());
    assert_eq!(app.document.result.current, ticket);
    assert_eq!(app.document.result.rows, rows);
    assert_eq!(app.document.filter, "field0 = 'unfinished'");
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
        }],
    );
    assert!(matches!(
        app.workspace.tree,
        Some(tiling::Node::Split {
            horizontal: false,
            ..
        })
    ));
    let rect = app
        .workspace
        .tiles
        .iter()
        .find(|(id, _)| *id == wide)
        .unwrap()
        .1;
    assert_eq!(rect.width(), app.workspace.area.width());
    // Escape cancels another drag and restores its original expanded state.
    app.expand_window(wide);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, size, vec![]);
    }
    let tree = app.workspace.tree.clone();
    let header = app
        .workspace
        .headers
        .iter()
        .find(|(id, _)| *id == wide)
        .unwrap()
        .1
        .center();
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![
            egui::Event::PointerMoved(header),
            pointer_button(header, true),
        ],
    );
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(header + egui::vec2(20.0, 0.0))],
    );
    assert_eq!(app.workspace.dragging, Some(wide));
    frame_at(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame_at(&mut app, &ctx, size, vec![pointer_button(target, false)]);
    assert_eq!(app.workspace.tree, tree);
    assert_eq!(app.workspace.expanded, Some(wide));
    assert!(app.document.has_drafts());
    let saved = app.workspace_snapshot().unwrap();
    let saved: workspace::Snapshot =
        serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert!(saved.valid());
    app.close();
}
