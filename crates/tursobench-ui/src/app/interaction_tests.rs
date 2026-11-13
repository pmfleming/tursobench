use super::{
    App, View,
    columns::CellStyle,
    context, egui, shell,
    test_ui::{control, control_from, inspect_selected, key_event, pointer_button},
    tiling, workspace,
};
use std::time::{Duration, Instant};
use tursobench_core::{Budget, Event, EventKind, TransactionState, Value};
use tursobench_db::{Operation, Request};
fn visible_result(app: &App) -> &tursobench_core::ResultSet {
    &app.database.document.result
}
fn pane(app: &App, kind: tiling::Kind) -> egui::Rect {
    app.database
        .workspace
        .pane_rects
        .iter()
        .find(|(k, _)| *k == kind)
        .unwrap()
        .1
}
fn assert_retained(app: &App, row: usize, col: usize, value: Value) {
    assert_eq!(
        visible_result(app).rows[row][col],
        value,
        "row {row}, column {col}"
    );
}
fn assert_staged(app: &App, row: usize, col: usize, value: Value) {
    assert_eq!(
        app.database
            .document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(&visible_result(app).rows[row], col)),
        Some(&value),
        "row {row}, column {col}"
    );
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
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    frame_at(app, ctx, egui::vec2(1400.0, 900.0), events)
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
    browse_and_wait(&mut app, &ctx, "t");
    let table_view = app.database.document.id;
    execute_and_wait(&mut app, &ctx, "BEGIN");
    execute_and_wait(&mut app, &ctx, "UPDATE t SET value='new'");
    assert!(!app.document_ref(table_view).unwrap().stale);
    execute_and_wait(&mut app, &ctx, "ROLLBACK");
    assert!(!app.document_ref(table_view).unwrap().stale);
    execute_and_wait(&mut app, &ctx, "UPDATE t SET value='new'");
    assert!(app.document_ref(table_view).unwrap().stale);
    assert_eq!(
        app.document_ref(table_view).unwrap().result.rows[0][1],
        Value::Text("old".into())
    );
    execute_and_wait(&mut app, &ctx, "CREATE TABLE added(id INTEGER PRIMARY KEY)");
    assert!(app.database.schema.iter().any(|o| o.name == "added"));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn external_changes_refresh_clean_tables_keep_sql_results_and_preserve_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(1,'initial')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
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
    crate::test_support::run(c.execute("UPDATE t SET value='external'", ())).unwrap();
    wait_until(&mut app, &ctx, |a| {
        a.document_ref(table)
            .unwrap()
            .result
            .rows
            .first()
            .is_some_and(|r| r[1] == Value::Text("external".into()))
    });
    assert!(app.document_ref(query).unwrap().stale);
    assert_eq!(
        app.document_ref(query).unwrap().result.rows,
        vec![vec![Value::Text("initial".into())]]
    );
    assert_eq!(
        app.database.document.id, query,
        "automatic refresh keeps keyboard focus"
    );
    app.switch_document(table);
    wait(&mut app, &ctx);
    app.stage_cell(0, 1, Value::Text("draft".into()));
    crate::test_support::run(c.execute("UPDATE t SET value='changed again'", ())).unwrap();
    wait_until(&mut app, &ctx, |a| a.database.document.external_refresh);
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
    assert_retained(&app, 0, 1, Value::Text("external".into()));
    crate::test_support::run(c.execute("CREATE TABLE added(id INTEGER)", ())).unwrap();
    wait_until(&mut app, &ctx, |a| {
        a.database.schema.iter().any(|t| t.name == "added")
    });
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
    app.database.document.drafts = None;
    drop(c);
    drop(db);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn byte_limited_pages_continue_and_reverse_without_a_false_end() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<151) INSERT INTO t SELECT x,hex(zeroblob(25000)) FROM n",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "t");
    let first = visible_result(&app).rows.len();
    assert!(first < 100);
    assert!(app.database.document.tools.pages.end.is_none());
    app.request_page(false);
    wait(&mut app, &ctx);
    assert!(
        visible_result(&app)
            .rows
            .iter()
            .any(|r| r[0] == Value::Integer(first as i64 + 1))
    );
    while app.database.document.browse.as_ref().unwrap().offset == 0 {
        app.request_page(false);
        wait(&mut app, &ctx);
    }
    app.request_page(true);
    wait(&mut app, &ctx);
    assert!(
        app.database.document.tools.pages.error.is_empty(),
        "{}",
        app.database.document.tools.pages.error
    );
    assert_retained(&app, 0, 0, Value::Integer(1));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn bulk_selection_deletion_is_staged_until_an_explicit_commit() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT DEFAULT 'default')",
            "INSERT INTO t VALUES(1,'one'),(2,'two'),(3,'three')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    app.database.document.tools.grid_selection = Some(super::selection::Selection {
        ticket: visible_result(&app).current,
        anchor: (0, 0),
        cursor: (1, 1),
    });
    app.stage_selected_deletions();
    assert_eq!(
        app.database.document.drafts.as_ref().unwrap().edits.len(),
        2
    );
    assert_eq!(visible_result(&app).rows.len(), 3);
    app.database.document.drafts = None;
    app.begin_insert();
    assert_eq!(app.database.document.insert_fields.len(), 2);
    assert!(!app.database.document.insert_use_json);
    app.database.document.show_insert = false;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn worker_termination_preserves_drafts_and_reports_unknown_cleanup() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)",
            "INSERT INTO t VALUES(1,'before')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let old = app.database.session.take().unwrap();
    old.close();
    assert!(matches!(
        old.events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
    drop(old);
    // An actual worker unwind drops the event sender without a Closed ack.
    let session = tursobench_db::Session::open(
        app.database.path.clone().into(),
        tursobench_core::Access::ReadWrite,
        || panic!("injected worker termination"),
    )
    .unwrap();
    assert!(matches!(
        session
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .kind,
        EventKind::Opened(_)
    ));
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
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
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
        app.request_page(false);
        wait(&mut app, &ctx);
    }
    app.database.document.drafts = Some(tursobench_core::DraftBatch {
        table: app.database.document.table.clone().unwrap(),
        edits: visible_result(&app)
            .rows
            .iter()
            .map(|row| tursobench_core::Edit::Update {
                original: row.clone(),
                changes: vec![(1, Value::Text("changed".into()))],
            })
            .collect(),
    });
    for (label, review, frames) in [("grid", false, 20), ("review", true, 3)] {
        app.database.document.show_preview = review;
        frame(&mut app, &ctx, vec![]);
        let start = Instant::now();
        for _ in 0..frames {
            frame(&mut app, &ctx, vec![]);
        }
        println!(
            "{}",
            serde_json::json!({"profile":label,"drafts":1000,"frames":frames,"mean_ms":start.elapsed().as_secs_f64()*1000.0/f64::from(frames)})
        );
    }
    for cached in [false, true] {
        let start = Instant::now();
        for _ in 0..1000 {
            for r in 980..1000 {
                for col in 0..2 {
                    if cached {
                        std::hint::black_box(app.database.document.tools.draft_ui.value(
                            app.database.document.drafts.as_ref(),
                            r,
                            col,
                        ));
                    } else {
                        std::hint::black_box(
                            app.database
                                .document
                                .drafts
                                .as_ref()
                                .unwrap()
                                .displayed(&visible_result(&app).rows[r], col),
                        );
                    }
                }
            }
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
fn filtered_counts_belong_to_their_view_and_ignore_replaced_requests() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY)",
            "INSERT INTO t VALUES(1),(2),(3),(4),(5)",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "t");
    let owner = app.database.document.id;
    app.add_filter(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Less,
        value: Value::Integer(3),
    });
    wait(&mut app, &ctx);
    app.count_table("t".into());
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.tools.filtered_count, Some(2));
    assert!(!app.database.tools.counts.contains_key("t"));
    app.open_sql_window(false);
    app.count_table("t".into());
    wait(&mut app, &ctx);
    assert_eq!(app.database.tools.counts.get("t"), Some(&5));
    assert_eq!(
        app.document_ref(owner).unwrap().tools.filtered_count,
        Some(2)
    );
    app.focus_window(owner);
    app.count_table("t".into());
    app.add_filter(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Greater,
        value: Value::Integer(1),
    });
    wait(&mut app, &ctx);
    assert!(app.database.document.tools.filtered_count.is_none());
    close_and_wait(&mut app, &ctx);
}

#[test]
fn keyless_byte_limited_pages_and_deferred_sort_values_can_scroll_back() {
    for (key, size) in [("", 25000), ("PRIMARY KEY", 50000)] {
        let create = format!("CREATE TABLE t(id INTEGER {key},value TEXT)");
        let insert = format!(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<301) INSERT INTO t SELECT x,hex(zeroblob({size})) FROM n"
        );
        let (_dir, ctx, mut app) = fixture(&[&create, &insert], false);
        app.preferences.row_limit = 200;
        browse_and_wait(&mut app, &ctx, "t");
        if size == 50000 {
            app.database.document.browse.as_mut().unwrap().sort = Some(1);
            app.reload();
            wait(&mut app, &ctx);
        }
        while app.database.document.tools.pages.end.is_none() {
            app.request_page(false);
            wait(&mut app, &ctx);
        }
        assert!(app.database.document.browse.as_ref().unwrap().offset > 100);
        while app.database.document.browse.as_ref().unwrap().offset > 0 {
            let before = app.database.document.browse.as_ref().unwrap().offset;
            app.request_page(true);
            wait(&mut app, &ctx);
            assert!(
                app.database.document.tools.pages.error.is_empty(),
                "{}",
                app.database.document.tools.pages.error
            );
            let offset = app.database.document.browse.as_ref().unwrap().offset;
            assert!(offset < before);
            assert_retained(&app, 0, 0, Value::Integer(offset as i64 + 1));
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
    frame(&mut app, &ctx, vec![]);
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(73)]]);
    assert_eq!(app.database.document.kind, tiling::Kind::Result);
    assert_eq!(app.database.workspace.expanded, None);
    assert_eq!(app.document_ref(editor).unwrap().sql, "SELECT 73 AS answer");
    let ticket = visible_result(&app).current;
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::CTRL);
    wait(&mut app, &ctx);
    assert_ne!(visible_result(&app).current, ticket);
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(73)]]);
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
    assert!(
        app.database
            .workspace
            .minimized
            .contains(&tiling::Kind::Sql)
    );
    app.focus_pane(tiling::Kind::Sql);
    assert_eq!(app.database.document.sql, "SELECT 73 AS answer");
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
    let a_id = app.database.document.id;
    let a_ticket = visible_result(&app).current.unwrap();
    app.database.document.sql = "SELECT 'draft a'".into();
    app.database.document.filter = "body = alpha".into();
    app.browse_table("b".into());
    let b_id = app.database.document.id;
    app.database.document.sql = "SELECT 'draft b'".into();
    wait(&mut app, &ctx);
    assert_eq!(
        app.database.document.id, b_id,
        "background completions must not change focus"
    );
    assert_retained(&app, 0, 1, Value::Text("beta".into()));
    app.switch_document(a_id);
    assert_eq!(visible_result(&app).current, Some(a_ticket));
    assert_retained(&app, 0, 1, Value::Text("alpha".into()));
    assert_eq!(app.database.document.sql, "SELECT 'draft a'");
    assert_eq!(app.database.document.filter, "body = alpha");
    app.stage_cell(0, 1, Value::Text("staged alpha".into()));
    assert!(app.database.document.has_drafts());
    app.switch_document(b_id);
    assert!(app.has_drafts(), "draft protection is session wide");
    assert!(!app.can_edit(), "another view owns the pending edit batch");
    assert!(app.database.document.drafts.is_none());
    assert_eq!(app.database.document.sql, "SELECT 'draft b'");
    // Discarded and stale deliveries cannot change either view or steal focus.
    let b_ticket = visible_result(&app).current.unwrap();
    let original = visible_result(&app).rows.clone();
    app.database
        .documents
        .get_mut(&a_id)
        .unwrap()
        .discard_ticket = Some(a_ticket);
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
            ticket: tursobench_core::Ticket {
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

#[test]
fn failed_open_after_close_can_retry_a_valid_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("valid.db");
    crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);
    let (ctx, mut app) = crate::app::test_ui::app();
    let disappearing = dir.path().join("missing.db");
    crate::test_support::create(&disappearing, &["CREATE TABLE example(id INTEGER)"]);
    app.set_database_path(disappearing.to_str().unwrap().into());
    inspect_selected(&mut app, &ctx);
    // The file can disappear after advisory checks; actual admission must still
    // report its asynchronous failure and complete a concurrent close request.
    std::fs::remove_file(&disappearing).unwrap();
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
    assert!(
        app.database
            .schema
            .iter()
            .any(|table| table.name == "example")
    );
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
        let disappearing = dir.path().join("missing.db");
        crate::test_support::create(&disappearing, &["CREATE TABLE example(id INTEGER)"]);
        app.set_database_path(disappearing.to_str().unwrap().into());

        app.database.writable = true;
        inspect_selected(&mut app, &ctx);
        std::fs::remove_file(&disappearing).unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, "scratchpad")).unwrap();
    let (ctx, mut app) = connected(&path, true);
    browse_and_wait(&mut app, &ctx, "buffer_ids");
    let data = app.database.document.id;
    let retained = visible_result(&app).rows.clone();
    let ticket = visible_result(&app).current;
    app.stage_cell(0, 0, Value::Text("pending diagram edit".into()));
    app.show_structure();
    wait(&mut app, &ctx);
    assert_eq!(app.database.workspace.active, tiling::Kind::Structure);
    assert_eq!(app.document_ref(data).unwrap().result.current, ticket);
    assert_eq!(app.document_ref(data).unwrap().result.rows, retained);
    app.overview();
    let map = app.database.document.id;
    assert!(app.has_drafts());
    assert_eq!(app.database.document.kind, tiling::Kind::Map);
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
            assert!(app.database.ui.sidebar_rect.right() <= app.database.workspace.area.left());
            let viewport = ctx.content_rect();
            assert_eq!(app.database.ui.sidebar_rect.left(), viewport.left());
            assert_eq!(app.database.ui.sidebar_rect.top(), viewport.top());
            assert_eq!(app.database.ui.sidebar_rect.bottom(), viewport.bottom());
            for rect in &app.database.ui.ribbon_actions {
                assert!(
                    viewport.contains_rect(*rect),
                    "ribbon action outside viewport: {rect:?}"
                );
                assert!(rect.left() >= app.database.ui.sidebar_rect.right());
                assert!(rect.height() >= 24.0);
                assert!(
                    app.database
                        .workspace
                        .pane_rects
                        .iter()
                        .any(|(_, tile)| tile.contains_rect(*rect))
                );
            }
            for (_, minimize, expand) in &app.database.workspace.controls {
                assert!((minimize.center().y - expand.center().y).abs() < 1.0);
                assert!(
                    app.database
                        .workspace
                        .pane_rects
                        .iter()
                        .any(|(_, tile)| tile.contains_rect(*minimize)
                            && tile.contains_rect(*expand)),
                    "controls outside tile at {width}x{height}: {minimize:?}, {expand:?}; {:?}",
                    app.database.workspace.pane_rects
                );
                for action in &app.database.ui.ribbon_actions {
                    if app.database.workspace.pane_rects.iter().any(|(_, tile)| {
                        tile.contains_rect(*minimize) && tile.contains_rect(*action)
                    }) {
                        assert!(
                            (action.center().y - minimize.center().y).abs() < 1.0,
                            "header controls must share one horizontal line: {action:?}, {minimize:?}"
                        );
                    }
                }
            }
            for (_, rect) in &app.database.workspace.tiles {
                assert!(app.database.workspace.area.contains_rect(*rect));
            }
            let saved = app.database.workspace.minimized.clone();
            app.expand_window(map);
            frame_at(&mut app, &ctx, egui::vec2(width, height), vec![]);
            assert_eq!(app.database.workspace.tiles.len(), 1);
            assert_eq!(app.database.workspace.tiles[0].0, map);
            assert!(
                app.database.ui.sidebar_rect.right() <= app.database.workspace.tiles[0].1.left()
            );
            app.expand_window(map);
            assert_eq!(app.database.workspace.minimized, saved);
        }
    }
    assert!(app.document_ref(data).unwrap().has_drafts());
    assert_eq!(app.document_ref(data).unwrap().result.rows, retained);
    app.activate_table("buffer_ids".into());
    assert_eq!(app.database.document.id, data);
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
    browse_and_wait(&mut app, &ctx, "a");
    app.stage_cell(0, 1, Value::Text("temporary".into()));
    app.database
        .document
        .drafts
        .as_mut()
        .unwrap()
        .revert_row(&app.database.document.result.rows[0]);
    assert!(!app.has_drafts());
    browse_and_wait(&mut app, &ctx, "b");
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert_eq!(
        app.database.document.drafts.as_ref().unwrap().table.name,
        "b"
    );
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert_retained(&app, 0, 1, Value::Text("only b".into()));
    browse_and_wait(&mut app, &ctx, "a");
    assert_retained(&app, 0, 1, Value::Text("same".into()));
    app.database.document.sql = "BEGIN".into();
    app.run_sql();
    wait(&mut app, &ctx);
    browse_and_wait(&mut app, &ctx, "a");
    assert_eq!(app.database.transaction, TransactionState::Active);
    assert!(!app.can_edit());
    app.stage_cell(0, 1, Value::Text("only b".into()));
    assert!(!app.has_drafts());
    app.database.document.sql = "ROLLBACK".into();
    app.run_sql();
    wait(&mut app, &ctx);
    browse_and_wait(&mut app, &ctx, "b");
    let retained = visible_result(&app).rows.clone();
    app.close();
    wait_closed(&mut app, &ctx);
    assert!(app.database.status.contains("cleanup confirmed"));
    app.database.document.ui.nav = shell::Nav::Table;
    app.first_cell_rect = None;
    frame(&mut app, &ctx, vec![]);
    assert!(app.first_cell_rect.is_some());
    assert_eq!(visible_result(&app).rows, retained);
    app.copy_results(&ctx, crate::export::Format::Json);
    assert!(app.database.status.contains("Copied retained"));
    app.browse_table("a".into());
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "a");
    assert_retained(&app, 0, 1, Value::Text("same".into()));
    app.database.document.ui.nav = shell::Nav::Result;
    frame(&mut app, &ctx, vec![]);
    assert!(app.database.session.is_none());
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
        assert_eq!(visible_result(&app).rows.len(), 100);
        assert!(!app.can_edit());
        frame(&mut app, &ctx, vec![]);
        let export = dir.path().join("page.jsonl");
        crate::export::write_result_as(
            &export,
            crate::export::Format::JsonLines,
            "table-page",
            &visible_result(&app).columns,
            &visible_result(&app).rows,
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
            app.database.document.sql =
                "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)".into();
            app.database.document.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
            app.run_sql();
            wait(&mut app, &ctx);
            assert_eq!(
                visible_result(&app).rows,
                vec![vec![Value::Text("entry-0000".into())]]
            );
            frame(&mut app, &ctx, vec![]);
            crate::export::write_result_as(
                &dir.path().join("fts.jsonl"),
                crate::export::Format::JsonLines,
                "complete-query-result",
                &visible_result(&app).columns,
                &visible_result(&app).rows,
                "t",
            )
            .unwrap();
        }
        close_and_wait(&mut app, &ctx);
        assert!(app.database.status.contains("cleanup confirmed"));
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
    browse_and_wait(&mut app, &ctx, "items");
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
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
    assert_eq!(clone, vec![None, Some(Value::Text("original".into()))]);
    assert!(!app.has_drafts());
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Stage insert (no database write)").center(),
    );
    assert_eq!(
        app.database.document.drafts.as_ref().unwrap().edits,
        vec![tursobench_core::Edit::Insert { values: clone }]
    );
    assert_eq!(visible_result(&app).rows.len(), 2);
    app.database.document.drafts = None;
    app.database.document.show_insert = false;
    app.stage_cell(0, 1, Value::Text("draft".into()));
    app.update_draft_presentation();
    let draft = app.database.document.drafts.as_ref().unwrap();
    let cached = app
        .database
        .document
        .tools
        .draft_ui
        .value(Some(draft), 0, 1)
        .unwrap();
    assert!(std::ptr::eq(
        cached,
        draft.displayed(&visible_result(&app).rows[0], 1).unwrap()
    ));
    app.database.document.show_preview = true;
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Remove operation").center(),
    );
    assert!(
        !app.has_drafts(),
        "review removes the selected local operation"
    );
    assert_retained(&app, 0, 1, Value::Text("original".into()));
    app.database.document.show_preview = false;
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let original = visible_result(&app).rows[0].clone();
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
    app.database
        .document
        .drafts
        .as_mut()
        .unwrap()
        .revert_cell(&original, 1);
    assert!(!app.has_drafts());
    app.stage_delete(0);
    assert!(
        app.database
            .document
            .drafts
            .as_ref()
            .unwrap()
            .deleted(&original)
    );
    app.database
        .document
        .drafts
        .as_mut()
        .unwrap()
        .revert_row(&original);
    assert!(!app.has_drafts());
    app.stage_cell(0, 0, Value::Integer(2));
    assert!(app.has_drafts());
    assert!(app.database.commit_ticket.is_none());
    // Inspecting, rendering and staging produced no SQL request or row mutation.
    assert_retained(&app, 0, 0, Value::Integer(1));
    frame(&mut app, &ctx, vec![]);
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
    assert!(app.database.status.contains("drafts retained"));
    app.stage_cell(0, 0, Value::Integer(3));
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_retained(&app, 1, 0, Value::Integer(3));
    // An ambiguous acknowledgement must retain the owning view's draft and
    // prevent both another commit and further staging, even in the background.
    app.stage_cell(0, 1, Value::Text("uncertain draft".into()));
    let owner = app.database.document.id;
    let other = *app.database.documents.keys().next().unwrap();
    app.database.sequence += 1;
    let ticket = app
        .database
        .session
        .as_ref()
        .unwrap()
        .ticket(app.database.sequence, app.database.generation);
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
    assert!(
        app.database.status.contains("UNKNOWN") && app.database.status.contains("Do not retry")
    );
    assert!(app.database.commit_ticket.is_none() && app.database.commit_owner.is_none());
    app.switch_document(owner);
    assert!(!app.can_edit());
    let sequence = app.database.sequence;
    app.commit_drafts();
    app.stage_cell(0, 1, Value::Text("blocked edit".into()));
    assert_eq!(app.database.sequence, sequence);
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
        app.database
            .schema
            .iter()
            .any(|o| o.name == "child" && o.ddl.as_ref().unwrap().contains("FOREIGN KEY"))
    );
    browse_and_wait(&mut app, &ctx, "child");
    assert!(app.database.document.tools.inspection_error.is_empty());
    let key = app
        .database
        .document
        .tools
        .inspection
        .as_ref()
        .unwrap()
        .foreign_keys[0]
        .clone();
    assert!(app.cell_references(1, 1).is_empty());
    app.database.document.ui.nav = shell::Nav::Table;
    frame(&mut app, &ctx, vec![]);
    let pos = app
        .fk_cell_rect
        .expect("composite FK rendered as a link")
        .center();
    click(&mut app, &ctx, pos);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.ui.nav, shell::Nav::Table);
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("parent"));
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "parent");
    assert_eq!(visible_result(&app).rows.len(), 1);
    assert_eq!(app.export_scope(), "related-row-lookup");
    app.follow_incoming(0, "child".into(), key.clone());
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "child");
    assert_eq!(visible_result(&app).rows.len(), 1);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("parent"));
    app.database.document.tools.detail = Some(0);
    app.show_structure();
    app.database.tools.help = true;
    app.database.document.tools.ddl = app
        .database
        .schema
        .iter()
        .find(|o| o.name == "parent")
        .cloned();
    frame(&mut app, &ctx, vec![]);
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "child");
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("child"));
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
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Text("bound-only-secret".into())]]
    );
    assert!(app.database.document.sql.contains("λ;"));
    let history = &app.preferences.query_history[0];
    assert_eq!(history.database, app.database.path);
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
    app.database.document.sql = "SELECT * FROM parent".into();
    app.database.document.params = "[]".into();
    app.run_text(app.database.document.sql.clone(), Some(false));
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.view, View::Explain);
    assert!(!visible_result(&app).rows.is_empty());
    app.database.document.tools.complete = true;
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
    assert_eq!(app.database.document.ui.nav, shell::Nav::Overview);
    assert_eq!(
        app.database.ui.diagram.data.as_ref().unwrap().tables.len(),
        2
    );
    frame(&mut app, &ctx, vec![]);
    // Explicit user camera choices survive opening/resizing other views.
    app.database.ui.diagram.freeze_camera();
    let camera = (
        app.database.ui.diagram.camera.zoom,
        app.database.ui.diagram.camera.pan,
    );
    browse_and_wait(&mut app, &ctx, "a");
    assert_eq!(app.database.document.ui.nav, shell::Nav::Table);
    assert_retained(&app, 0, 1, Value::Text("original".into()));
    let retained_ticket = visible_result(&app).current;
    app.refresh_catalog();
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).current, retained_ticket);
    assert_retained(&app, 0, 1, Value::Text("original".into()));
    for view in [
        shell::TableView::Data,
        shell::TableView::Structure,
        shell::TableView::Data,
    ] {
        app.database.document.ui.table_view = view;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            (
                app.database.ui.diagram.camera.zoom,
                app.database.ui.diagram.camera.pan
            ),
            camera
        );
    }
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = visible_result(&app).current;
    app.browse_table("b".into());
    assert_eq!(visible_result(&app).current, ticket);
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("a"));
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "a");
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
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(2)]]);
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "b");
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("b"));
    assert!(
        app.database
            .schema
            .iter()
            .any(|o| o.kind == "trigger" && o.table == "b")
    );
    app.database.ui.sidebar = shell::ToolPanel::Schema;
    browse_and_wait(&mut app, &ctx, "v");
    assert_eq!(
        app.database
            .document
            .tools
            .inspection
            .as_ref()
            .unwrap()
            .table
            .name,
        "v"
    );
    assert!(!app.can_edit());
    app.database.document.ui.table_view = shell::TableView::Structure;
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.ui.diagram.selected.as_deref(), Some("v"));
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
    assert!(app.database.ui.transaction_rects.is_empty());
    app.toggle_sql();
    execute_and_wait(&mut app, &ctx, "SELECT 42 AS answer");
    assert_eq!(app.database.document.ui.nav, shell::Nav::Result);
    assert_eq!(app.database.document.view, View::Query);
    assert!(app.database.document.table.is_none());
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(42)]]);
    let ticket = visible_result(&app).current;
    let result_id = app.database.document.id;
    app.toggle_sql();
    assert_eq!(app.database.document.kind, tiling::Kind::Sql);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.document_ref(result_id).unwrap().result.current, ticket);
    app.toggle_sql();
    execute_and_wait(&mut app, &ctx, "BEGIN");
    assert_eq!(app.database.transaction, TransactionState::Active);
    app.toggle_sql();
    app.overview();
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.ui.transaction_rects.len(), 2);
    let rollback = app.database.ui.transaction_rects[1].center();
    click(&mut app, &ctx, rollback);
    wait(&mut app, &ctx);
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    app.sidebar_search.focus = true;
    app.sidebar_search.query = "a.id".into();
    frame(&mut app, &ctx, vec![]);
    wait(&mut app, &ctx);
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    control(&output, "a.id · column");
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "a");
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
    let output = frame(&mut app, &ctx, vec![]);
    app.database.ui.diagram.freeze_camera();
    app.database.ui.diagram.camera.zoom = 0.7;
    app.database.ui.diagram.camera.pan = egui::vec2(30.0, 42.0);
    let bounds = crate::app::test_ui::nodes(&output)
        .iter()
        .find_map(|(_, node)| {
            (node.label() == Some("Read only") && node.role() == egui::Role::Button)
                .then(|| node.bounds())
                .flatten()
        })
        .expect("status-bar access icon");
    click(
        &mut app,
        &ctx,
        egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        ),
    );
    assert!(app.database.ui.access_dialog);
    assert_eq!(app.database.session.as_ref().unwrap().id, old_session);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Close cleanly and reopen").center(),
    );
    assert!(app.database.closing);
    assert!(!app.database.writable);
    wait(&mut app, &ctx);
    assert!(app.database.writable);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(
        crate::app::test_ui::nodes(&output)
            .iter()
            .any(
                |(_, node)| node.label() == Some("Read/write") && node.role() == egui::Role::Button
            )
    );
    assert_ne!(app.database.session.as_ref().unwrap().id, old_session);
    assert_eq!(app.database.document.ui.nav, shell::Nav::Overview);
    assert!(app.database.pending_access.is_none());
    assert_eq!(app.database.ui.diagram.camera.zoom, 0.7);
    assert_eq!(app.database.ui.diagram.camera.pan, egui::vec2(30.0, 42.0));
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
    browse_and_wait(&mut app, &ctx, "a");
    let browse = app.database.document.browse.as_mut().unwrap();
    browse.sort = Some(1);
    browse.descending = true;
    browse.filters.push(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Equal,
        value: Value::Integer(2),
    });
    app.reload();
    wait(&mut app, &ctx);
    app.database.document.inspect = Some((0, 1));
    app.database.document.tools.detail = Some(0);
    app.database.document.tools.grid_scroll = egui::vec2(40.0, 20.0);
    browse_and_wait(&mut app, &ctx, "b");
    app.navigate_back();
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.browse.as_ref().unwrap().sort, Some(1));
    assert!(app.database.document.browse.as_ref().unwrap().descending);
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Integer(2), Value::Text("second".into())]]
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
    app.stage_cell(0, 1, Value::Text("draft".into()));
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
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::Comma, egui::Modifiers::CTRL);
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
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
    app.settings.page = super::settings::Page::Queries;
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.settings.open);
    app.execute_command(&ctx, super::commands::Command::Settings);
    assert_eq!(app.settings.page, super::settings::Page::Queries);
    assert!(app.settings.open);
    app.open_palette();
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
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
    let rows = visible_result(&app).rows.clone();
    let ticket = visible_result(&app).current;
    for panel in [
        shell::ToolPanel::History,
        shell::ToolPanel::Saved,
        shell::ToolPanel::Diagnostics,
    ] {
        let nav = app.database.document.ui.nav;
        app.select_tool(panel);
        assert_eq!(app.database.ui.sidebar, panel);
        assert_eq!(app.database.document.ui.nav, nav);
        let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
        assert!(output.shapes.len() > 10);
        app.select_tool(shell::ToolPanel::Schema);
        assert_eq!(visible_result(&app).current, ticket);
        assert_eq!(visible_result(&app).rows, rows);
    }
    browse_and_wait(&mut app, &ctx, "example_view");
    assert_eq!(visible_result(&app).rows, rows);
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
    for dark in [false, true] {
        crate::appearance::theme(&ctx, dark);
        for (size, scale, expanded) in [
            (egui::vec2(900.0, 600.0), 1.0, false),
            (egui::vec2(1400.0, 900.0), 1.0, true),
            (egui::vec2(1440.0, 960.0), 1.5, true),
        ] {
            ctx.set_pixels_per_point(scale);
            app.database.workspace.expanded = expanded.then_some(tiling::Kind::Data);
            frame_at(&mut app, &ctx, size, vec![]);
            let output = frame_at(&mut app, &ctx, size, vec![]);
            let pane = pane(&app, tiling::Kind::Data);
            let text_position = |value: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.job.text == value && pane.contains(text.pos) =>
                        {
                            Some(text.pos)
                        }
                        _ => None,
                    })
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
            let expand = app
                .database
                .workspace
                .controls
                .iter()
                .find(|(kind, _, _)| *kind == tiling::Kind::Data)
                .unwrap()
                .2;
            let count = control(&output, "Count matching rows");
            assert!(
                (expand.right() - count.right()).abs() < 1.1,
                "expand {expand:?}, count {count:?}"
            );
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| matches!(&shape.shape,egui::Shape::Rect(rect)
                if (rect.rect.height()-super::grid::HEADER_HEIGHT).abs()<1.1
                    && rect.rect.width()>=pane.width()-9.0 && pane.contains_rect(rect.rect))),
                "full-width column header"
            );
        }
    }
    ctx.set_pixels_per_point(1.0);
    browse_and_wait(&mut app, &ctx, "status_test");
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Count matching rows").center(),
    );
    wait(&mut app, &ctx);
    assert_eq!(app.database.tools.counts.get("status_test"), Some(&205));
    app.database.document.tools.jump_row = 151;
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Go to row").center());
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.browse.as_ref().unwrap().offset, 150);
    assert_eq!(visible_result(&app).rows[0], vec![Value::Integer(151)]);
    close_and_wait(&mut app, &ctx);
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
    browse_and_wait(&mut app, &ctx, "a");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let ticket = visible_result(&app).current;
    let rows = visible_result(&app).rows.clone();
    app.count_table("a".into());
    wait(&mut app, &ctx);
    assert_eq!(app.database.tools.counts.get("a"), Some(&2));
    assert_eq!(visible_result(&app).current, ticket);
    assert_eq!(visible_result(&app).rows, rows);
    assert!(app.has_drafts());
    app.count_table("missing table".into());
    wait(&mut app, &ctx);
    assert!(app.database.status.contains("Count failed"));
    assert!(!app.database.tools.counts.contains_key("missing table"));
    assert_eq!(visible_result(&app).current, ticket);
    assert_eq!(visible_result(&app).rows, rows);
    assert!(app.has_drafts());
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.database.tools.counts.is_empty());
    app.close();
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
                app.database.document.ui.sql_open = sql;
                app.database.document.ui.table_view = if structure {
                    shell::TableView::Structure
                } else {
                    shell::TableView::Data
                };
                app.database.document.ui.inspector_open = details;
                app.database.document.inspect = Some((0, 0));
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
                assert!(app.database.ui.sidebar_rect.width() >= 180.0);
                assert_eq!(app.database.workspace.pane_rects.len(), 5);
                if !structure {
                    assert!(
                        app.database
                            .ui
                            .workspace_rect
                            .contains_rect(app.database.ui.grid_rect),
                        "{width}x{height}: workspace {:?}, grid {:?}, tiles {:?}",
                        app.database.ui.workspace_rect,
                        app.database.ui.grid_rect,
                        app.database.workspace.tiles
                    );
                }
                if width == 1280.0 && !details && !sql && !structure {
                    assert!(
                        app.database.ui.visible_rows >= 9,
                        "{} fully visible rows",
                        app.database.ui.visible_rows
                    );
                }
            }
        }
    }
    ctx.set_pixels_per_point(1.0);
    execute_and_wait(&mut app, &ctx, "BEGIN");
    app.database.document.ui.sql_open = false;
    app.overview();
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(720.0, 480.0), vec![]);
    }
    assert_eq!(app.database.ui.transaction_rects.len(), 2);
    for r in &app.database.ui.transaction_rects {
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
    browse_and_wait(&mut app, &ctx, "a");
    let ticket = visible_result(&app).current;
    let popup = egui::Id::unique("test-menu");
    egui::Popup::open_id(&ctx, popup);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&ctx));
    egui::Popup::open_id(&ctx, popup);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(app.database.ui.palette_open);
    assert!(!egui::Popup::is_any_open(&ctx));
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.database.ui.palette_open);
    app.database.document.ui.sql_open = true;
    app.database.document.ui.filter_open = true;
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
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
        press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!opened(&app), "Escape must close {key:?}'s surface");
    }
    app.database.document.tools.columns.hidden.insert(0);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Text("Show hidden columns".into())],
    );
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert!(!app.database.ui.palette_open);
    assert!(app.database.document.tools.columns.hidden.is_empty());
    assert_eq!(visible_result(&app).current, ticket);
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
    browse_and_wait(&mut app, &ctx, "sample");
    frame(&mut app, &ctx, vec![]);
    ctx.enable_accesskit();
    for (label, sorted, descending) in [
        ("Column number", Some(2), false),
        ("Column number ascending", Some(2), true),
        ("Column number descending", None, false),
    ] {
        let output = frame(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, control(&output, label).center());
        wait(&mut app, &ctx);
        let browse = app.database.document.browse.as_ref().unwrap();
        assert_eq!((browse.sort, browse.descending), (sorted, descending));
    }
    app.database
        .document
        .tools
        .columns
        .visuals
        .insert(1, CellStyle::Status);
    frame(&mut app, &ctx, vec![]);
    assert_retained(&app, 0, 1, Value::Text("a long string".into()));
    app.database.document.tools.columns.move_before(2, 0);
    for (from, target, expected) in [
        (2, 2, vec![2, 0, 1]),
        (9, 0, vec![2, 0, 1]),
        (0, 9, vec![2, 0, 1]),
        (2, 1, vec![0, 2, 1]),
        (2, 0, vec![2, 0, 1]),
    ] {
        app.database
            .document
            .tools
            .columns
            .move_before(from, target);
        assert_eq!(app.database.document.tools.columns.order, expected);
    }
    app.database.document.tools.columns.hidden.insert(1);
    app.database.document.tools.columns.manual.insert(2);
    app.database.document.tools.columns.widths[2] = 217.0;
    frame(&mut app, &ctx, vec![]);
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
            (
                CellStyle::Auto,
                "status",
                Value::Text("blocked".into()),
                true,
            ),
            (
                CellStyle::Status,
                "state",
                Value::Text("pending".into()),
                true,
            ),
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
    for label in ["Fit contents", "Move left", "Move right", "Hide column"] {
        let mut draw = |events| {
            super::test_ui::render(&menu_ctx, egui::vec2(300.0, 240.0), events, |ui| {
                app.database.document.tools.columns.header_menu(ui, 2)
            })
        };
        draw(vec![]);
        let output = draw(vec![]);
        let pos = control(&output, label).center();
        for pressed in [true, false] {
            draw(super::test_ui::pointer(pos, pressed));
        }
    }
    assert!(!app.database.document.tools.columns.manual.contains(&2));
    assert!(app.database.document.tools.columns.refit);
    assert_eq!(app.database.document.tools.columns.order, vec![0, 2, 1]);
    assert!(app.database.document.tools.columns.hidden.contains(&2));
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
    let owner = visible_result(&app).current;
    let first = visible_result(&app).rows.clone();
    app.request_page(false);
    assert_eq!(visible_result(&app).rows, first);
    assert!(app.database.document.tools.pages.pending.is_some());
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows.len(), 200);
    assert_eq!(visible_result(&app).current, owner);
    app.request_page(false);
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    app.select_cell(150, 0, false);
    app.database.document.tools.grid_scroll.y = 3000.0;
    execute_and_wait(&mut app, &ctx, "SELECT 765");
    app.return_to_table();
    wait(&mut app, &ctx);
    for _ in 0..2 {
        frame(&mut app, &ctx, vec![]);
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
    app.request_page(false);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.browse.as_ref().unwrap().offset, 100);
    assert_eq!(visible_result(&app).rows.len(), 300);
    assert_eq!(app.database.document.inspect, Some((100, 1)));
    app.request_page(true);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.browse.as_ref().unwrap().offset, 0);
    assert_retained(&app, 0, 0, Value::Integer(1));
    app.stage_cell(0, 1, Value::Integer(999));
    app.request_page(false);
    assert!(app.database.document.tools.pages.pending.is_none());
    assert!(app.has_drafts());
    app.database.document.drafts = None;
    app.request_page(false);
    execute_and_wait(&mut app, &ctx, "SELECT 987 AS answer");
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(987)]]);
    browse_and_wait(&mut app, &ctx, "sample");
    for _ in 0..5 {
        app.request_page(false);
        wait(&mut app, &ctx);
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
    assert!(app.add_filter(tursobench_core::Filter {
        column: 0,
        op: tursobench_core::FilterOp::Equal,
        value: Value::Integer(1)
    }));
    wait(&mut app, &ctx);
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Integer(1), Value::Integer(1)]],
        "filter queries the entire table, including evicted rows"
    );
    assert_eq!(app.database.document.browse.as_ref().unwrap().offset, 0);
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
    let child = app
        .database
        .document
        .tools
        .inspection
        .as_ref()
        .unwrap()
        .foreign_keys[0]
        .clone();
    let owner = visible_result(&app).current;
    let original = visible_result(&app).rows.clone();
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    wait(&mut app, &ctx);
    let submitted = app.database.sequence;
    app.request_context(0, "child", &child, context::ReadKind::Parent);
    assert_eq!(
        app.database.sequence, submitted,
        "cache hits must not submit another request"
    );
    assert_eq!(visible_result(&app).current, owner);
    assert_eq!(visible_result(&app).rows, original);
    browse_and_wait(&mut app, &ctx, "parent");
    frame(&mut app, &ctx, vec![]);
    app.select_cell(0, 0, false);
    let owner = visible_result(&app).current;
    let original = visible_result(&app).rows.clone();
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
    for size in [egui::vec2(1280.0, 720.0), egui::vec2(3440.0, 1440.0)] {
        frame_at(&mut app, &ctx, size, vec![]);
        let output = frame_at(&mut app, &ctx, size, vec![]);
        let fields = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Fields & values" => {
                    Some(text.pos)
                }
                _ => None,
            })
            .expect("field section heading");
        let relationships = control(&output, "Relationships");
        let value = control(&output, "a · INTEGER · 7");
        assert!(fields.y < value.top());
        assert!(value.bottom() < relationships.top());
        assert!(value.width() > pane(&app, tiling::Kind::Record).width() * 0.9);
        assert_eq!(
            app.database.sequence, sequence,
            "width must not trigger related reads"
        );
    }
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Relationships").center());
    // Opening key navigation still leaves row reads deferred.
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    assert_eq!(app.database.sequence, sequence);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Preview related rows").center(),
    );
    for _ in 0..3 {
        frame(&mut app, &ctx, vec![]);
    }
    wait(&mut app, &ctx);
    let output = frame(&mut app, &ctx, vec![]);
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
    assert_eq!(visible_result(&app).current, owner);
    assert_eq!(visible_result(&app).rows, original);
    assert_eq!(app.database.document.inspect, Some((0, 0)));
    app.request_context(0, "child", &child, context::ReadKind::Count);
    execute_and_wait(&mut app, &ctx, "SELECT 654");
    assert_eq!(visible_result(&app).rows, vec![vec![Value::Integer(654)]]);
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
    browse_and_wait(&mut app, &ctx, "sample");
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
        vec![key_event(
            egui::Key::ArrowRight,
            true,
            egui::Modifiers::SHIFT,
        )],
    );
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 0), (0, 1)]
    );
    let output = send(
        &mut app,
        egui::Modifiers::CTRL,
        vec![key_event(egui::Key::C, true, egui::Modifiers::CTRL)],
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
        vec![key_event(
            egui::Key::ArrowRight,
            true,
            egui::Modifiers::NONE,
        )],
    );
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert!(app.database.document.tools.inline.is_some());
    send(
        &mut app,
        egui::Modifiers::NONE,
        vec![
            egui::Event::Text("draft".into()),
            key_event(egui::Key::Enter, true, egui::Modifiers::NONE),
        ],
    );
    assert!(app.database.document.tools.inline.is_none());
    assert!(app.has_drafts());
    assert_eq!(visible_result(&app).rows[0][2], Value::Text(String::new()));
    // The editable record inspector uses the same focus/submission policy as the grid.
    app.focus_pane(tiling::Kind::Record);
    app.start_inline(0, 1);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.record_inspector(ui, 0));
    output.textures_delta.clear();
    for (input, accepted) in [("invalid", false), ("123", true)] {
        app.database
            .document
            .tools
            .inline
            .as_mut()
            .unwrap()
            .editor
            .input = input.into();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
                ..Default::default()
            },
            |ui| app.record_inspector(ui, 0),
        );
        output.textures_delta.clear();
        assert_eq!(app.database.document.tools.inline.is_none(), accepted);
    }
    assert_staged(&app, 0, 1, Value::Integer(123));
    assert_retained(&app, 0, 1, Value::Integer(9007199254740993));
    app.start_inline(0, 1);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.database.document.tools.inline.is_none());
    app.select_cell(0, 1, false);
    app.select_cell(1, 1, true);
    assert_eq!(
        app.selected_cells().collect::<Vec<_>>(),
        vec![(0, 1), (1, 1)]
    );
    let summary = app.selection_summary();
    assert!(summary.contains("18014398509481988"), "{summary}");
    app.database.document.tools.columns.move_before(2, 1);
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
    browse_and_wait(&mut app, &ctx, "sample");
    app.database.document.tools.filter_focus = true;
    frame(&mut app, &ctx, vec![]);
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Text("n>=2".into()),
            key_event(egui::Key::Enter, true, egui::Modifiers::NONE),
        ],
    );
    wait(&mut app, &ctx);
    assert_eq!(visible_result(&app).rows.len(), 1);
    assert_retained(&app, 0, 0, Value::Integer(2));
    assert_eq!(
        app.database.document.browse.as_ref().unwrap().filters[0].op,
        tursobench_core::FilterOp::GreaterOrEqual
    );
    app.database.document.tools.filter_focus = true;
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::Backspace, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    assert!(
        app.database
            .document
            .browse
            .as_ref()
            .unwrap()
            .filters
            .is_empty()
    );
    assert_eq!(visible_result(&app).rows.len(), 2);
    app.database.document.tools.columns.move_before(2, 0);
    app.select_cell(0, 2, false);
    app.select_cell(0, 0, true);
    assert_eq!(app.selection_tsv().unwrap(), "\"\"\t1");
    app.close();
}

#[test]
fn clipboard_grid_and_record_share_live_rows_without_overlays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clipboard.db");
    crate::test_support::run(crate::local_project_fixtures::create(&path, "clipboard")).unwrap();
    let (ctx, mut app) = connected(&path, false);
    browse_and_wait(&mut app, &ctx, "entries");
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    }
    assert!(!app.database.document.ui.inspector_open);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1920.0, 1080.0), vec![]);
    }
    assert!(!app.database.document.ui.inspector_open);
    assert_eq!(visible_result(&app).columns.len(), 11);
    app.expand_window(app.database.document.id);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(1920.0, 1080.0), vec![]);
    }
    let width: f32 = app
        .database
        .document
        .tools
        .columns
        .widths
        .iter()
        .sum::<f32>()
        + 44.0
        + visible_result(&app).columns.len() as f32
            * ctx.style_of(egui::Theme::Dark).spacing.item_spacing.x;
    assert!(
        width <= app.database.ui.grid_rect.width() + 1.0,
        "columns {width} > grid {}",
        app.database.ui.grid_rect.width()
    );
    assert!(
        app.database.ui.visible_rows >= 30,
        "{} rows",
        app.database.ui.visible_rows
    );
    let data_id = app.database.document.id;
    app.open_record_window(0, 0);
    assert_eq!(app.database.workspace.active, tiling::Kind::Record);
    assert_eq!(app.database.document.id, data_id);
    for _ in 0..2 {
        frame_at(&mut app, &ctx, egui::vec2(2560.0, 1440.0), vec![]);
    }
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
    frame(&mut app, &ctx, vec![]);
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    let pos = output
        .shapes
        .iter()
        .rev()
        .find_map(|s| {
            if let egui::Shape::Text(t) = &s.shape {
                (t.galley.job.text == "sample_value · index"
                    && app.database.ui.sidebar_rect.contains(t.pos))
                .then_some(t.pos + egui::vec2(8.0, 6.0))
            } else {
                None
            }
        })
        .expect("filtered index row");
    let data_id = app.database.document.id;
    let owner = visible_result(&app).current;
    let rows = visible_result(&app).rows.clone();
    click(&mut app, &ctx, pos);
    assert_eq!(
        app.database.document.tools.ddl.as_ref().unwrap().name,
        "sample_value"
    );
    assert_eq!(
        app.database.document.ui.table_view,
        shell::TableView::Structure
    );
    app.refresh_table();
    wait(&mut app, &ctx);
    assert_eq!(
        app.database.document.ui.table_view,
        shell::TableView::Structure
    );
    assert_eq!(app.document_ref(data_id).unwrap().result.current, owner);
    assert_eq!(app.document_ref(data_id).unwrap().result.rows, rows);
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
    browse_and_wait(&mut app, &ctx, "sample");
    app.select_cell(0, 1, false);
    app.stage_cell(0, 1, Value::Text("draft".into()));
    for _ in 0..2 {
        frame(&mut app, &ctx, vec![]);
    }
    let ticket = visible_result(&app).current;
    let sequence = app.database.sequence;
    let focus = app.database.workspace.focused;
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
            let before = app.database.workspace.pane_rects.clone();
            let rect = tiling::dividers(
                &before,
                &app.database.workspace.minimized,
                app.database.workspace.area,
            )
            .into_iter()
            .find(|(d, _)| *d == divider)
            .unwrap()
            .1;
            let original_splits = app.database.workspace.splits;
            click(&mut app, &ctx, rect.center());
            assert_eq!(
                app.database.workspace.splits, original_splits,
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
            let after = &app.database.workspace.pane_rects;
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
        let panes = &app.database.workspace.pane_rects;
        let handle = tiling::dividers(
            panes,
            &app.database.workspace.minimized,
            app.database.workspace.area,
        )[0]
        .1;
        drag(&mut app, handle.center(), egui::vec2(distance, 0.0));
        assert!(
            app.database
                .workspace
                .pane_rects
                .iter()
                .all(|(_, rect)| rect.width() >= 179.9)
        );
        let panes = &app.database.workspace.pane_rects;
        let handle = tiling::dividers(
            panes,
            &app.database.workspace.minimized,
            app.database.workspace.area,
        )[1]
        .1;
        drag(&mut app, handle.center(), egui::vec2(0.0, distance));
        assert!(
            app.database
                .workspace
                .pane_rects
                .iter()
                .all(|(_, rect)| rect.height() >= 95.9)
        );
    }
    let splits = app.database.workspace.splits;
    for fraction in [0.08, 0.92] {
        let before = app.database.ui.sidebar_rect;
        let pos = egui::pos2(before.right(), before.top() + before.height() * fraction);
        drag(&mut app, pos, egui::vec2(30.0, 0.0));
        assert!((app.database.ui.sidebar_rect.width() - before.width() - 30.0).abs() < 1.0);
        assert_eq!(app.database.workspace.splits, splits);
    }
    assert_eq!(visible_result(&app).current, ticket);
    assert_retained(&app, 0, 1, Value::Text("retained".into()));
    assert_staged(&app, 0, 1, Value::Text("draft".into()));
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
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE chunks(id TEXT PRIMARY KEY, part INTEGER, body BLOB)",
            "INSERT INTO chunks VALUES('buffer-1780913432386559807-118',0,x'001122')",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "chunks");
    let data_id = app.database.document.id;
    app.expand_window(data_id);
    frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    assert!(
        app.database
            .document
            .tools
            .columns
            .widths
            .iter()
            .all(|w| *w <= 300.0)
    );
    assert!(app.database.document.tools.columns.widths[1] < 100.0);
    let widths = app.database.document.tools.columns.widths.clone();
    frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
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
    app.database.workspace.splits = tiling::Splits {
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
    assert_eq!(app.database.workspace.splits, tiling::Splits::default());
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
    assert_eq!(app.database.document.id, editor);
    assert!(app.database.document.sql.is_empty());
    assert_eq!(app.database.document.params, "[]");
    assert_eq!(app.database.workspace.expanded, Some(tiling::Kind::Sql));
    wait_until(&mut app, &ctx, |a| {
        !a.executing()
            && a.database
                .documents
                .values()
                .all(|d| !d.needs_load && d.tools.inspection_ticket.is_none())
    });
    assert_eq!(
        app.database.document.id, editor,
        "background reload preserves focus"
    );
    assert_eq!(app.document_ref(data_id).unwrap().result.rows.len(), 1);
    let mut invalid: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    invalid["focused"] = serde_json::json!(9999);
    let invalid: workspace::Snapshot = serde_json::from_value(invalid).unwrap();
    assert!(!invalid.valid());
    app.preferences.workspace_layouts.push(invalid);
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
    frame(&mut app, &ctx, vec![]);
    browse_and_wait(&mut app, &ctx, "a");
    app.open_structure_window();
    wait(&mut app, &ctx);
    app.retile();
    for expanded in [false, true, false] {
        if expanded != app.database.workspace.expanded.is_some() {
            app.expand_window(map);
        }
        for _ in 0..2 {
            frame_at(&mut app, &ctx, egui::vec2(1800.0, 1000.0), vec![]);
        }
        let tile = app
            .database
            .workspace
            .tiles
            .iter()
            .find(|(id, _)| *id == map)
            .unwrap()
            .1;
        assert_eq!(app.database.ui.diagram.table_rects.len(), 3);
        for (_, rect) in &app.database.ui.diagram.table_rects {
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
    let (_dir, ctx, mut app) = fixture(
        &[
            &format!("CREATE TABLE sample(id INTEGER PRIMARY KEY, body TEXT,{columns})"),
            &format!(
                "INSERT INTO sample VALUES(1,'first',{values}),(2,'second',{values}),(3,'third',{values})"
            ),
        ],
        true,
    );
    ctx.enable_accesskit();
    browse_and_wait(&mut app, &ctx, "sample");
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    app.expand_pane(tiling::Kind::Record);
    let sequence = app.database.sequence;
    let ticket = visible_result(&app).current;
    for (label, row, value) in [
        ("Next record", 1, "second"),
        ("Next record", 2, "third"),
        ("Previous record", 1, "second"),
    ] {
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
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
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Record position").center());
    press_key(&mut app, &ctx, egui::Key::A, egui::Modifiers::CTRL);
    frame(&mut app, &ctx, vec![egui::Event::Text("1".into())]);
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    assert_eq!(app.database.sequence, sequence);
    app.open_record_window(1, 0);
    frame(&mut app, &ctx, vec![]);
    app.stage_cell(1, 2, Value::Integer(999));
    app.database.document.tools.columns.hidden.insert(6);
    let ticket = visible_result(&app).current;
    for _ in 0..6 {
        press_key(&mut app, &ctx, egui::Key::ArrowDown, egui::Modifiers::NONE);
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
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert!(app.database.document.tools.inline.is_some());
    frame(&mut app, &ctx, vec![]);
    app.database
        .document
        .tools
        .inline
        .as_mut()
        .unwrap()
        .editor
        .kind = super::results::CellKind::Null;
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert!(app.database.document.tools.inline.is_none());
    assert_staged(&app, 1, 6, Value::Null);
    assert_retained(&app, 1, 6, Value::Integer(4));
    assert_eq!(app.database.sequence, sequence);
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
    browse_and_wait(&mut app, &ctx, "first_table");
    let first_id = app.database.document.id;
    let choose = |app: &mut App, name: &str| {
        for label in ["Choose table", name] {
            frame(app, &ctx, vec![]);
            let output = frame_at(app, &ctx, egui::vec2(1400.0, 900.0), vec![]);
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
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    choose(&mut app, "second_table");
    assert_eq!(app.database.document.id, first_id);
    assert!(app.database.document.has_drafts());
    assert!(app.database.status.contains("Commit or discard drafts"));
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
    browse_and_wait(&mut app, &ctx, "sample");
    let data = app.database.document.id;
    let ticket = visible_result(&app).current;
    app.open_record_window(0, 1);
    app.stage_cell(0, 1, Value::Text("unsaved".into()));
    app.database.document.filter = "body = 'unfinished'".into();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let (_, minimize, _) = *app
        .database
        .workspace
        .controls
        .iter()
        .find(|(k, _, _)| *k == tiling::Kind::Map)
        .unwrap();
    click(&mut app, &ctx, minimize.center());
    assert!(
        app.database
            .workspace
            .minimized
            .contains(&tiling::Kind::Map)
    );
    frame(&mut app, &ctx, vec![]);
    let map = pane(&app, tiling::Kind::Map);
    assert!((map.height() - tiling::BAR).abs() < 0.1);
    click(&mut app, &ctx, map.left_center() + egui::vec2(70.0, 0.0));
    assert!(
        !app.database
            .workspace
            .minimized
            .contains(&tiling::Kind::Map)
    );
    for kind in tiling::Kind::PANES {
        app.minimize_pane(kind);
    }
    app.expand_pane(tiling::Kind::Record);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.workspace.pane_rects.len(), 1);
    assert_eq!(app.database.document.id, data);
    assert!(app.database.document.has_drafts());
    app.expand_pane(tiling::Kind::Record);
    assert_eq!(app.database.workspace.minimized.len(), 4);
    app.retile();
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.workspace.pane_rects.len(), 5);
    assert_eq!(visible_result(&app).current, ticket);
    assert_retained(&app, 0, 1, Value::Text("kept".into()));
    assert_eq!(app.database.document.filter, "body = 'unfinished'");
    assert_eq!(app.database.document.inspect, Some((0, 1)));
    let output = frame(&mut app, &ctx, vec![]);
    let data_rect = pane(&app, tiling::Kind::Data);
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
    assert!(!app.database.document.show_preview);
    click(&mut app, &ctx, commit);
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_retained(&app, 0, 1, Value::Text("unsaved".into()));
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
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        let nodes = &crate::app::test_ui::nodes(&output);
        assert!(
            !nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Run selection / statement at cursor"))
        );
        let before = app.database.sequence;
        click(&mut app, &ctx, control(&output, "Run all").center());
        assert_eq!(
            app.database.sequence, before,
            "choosing a policy must precede execution"
        );
        let output = frame(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, control(&output, policy).center());
        wait(&mut app, &ctx);
        assert_eq!(app.database.document.script_results.len(), 2);
        assert!(
            visible_result(&app)
                .completion
                .as_ref()
                .unwrap()
                .error
                .is_some()
        );
        if atomic {
            assert!(app.database.document.script_message.contains("rolled back"));
        }
        // Selecting an outcome preserves every statement's rows without
        // copying a second payload into the visible result.
        for i in [0, 1, 0, 0] {
            app.database.document.select_script_result(i);
            let result = &visible_result(&app);
            assert_eq!(
                result.rows,
                if i == 0 {
                    vec![vec![Value::Integer(1)]]
                } else {
                    vec![]
                }
            );
            assert_eq!(result.completion.as_ref().unwrap().error.is_some(), i == 1);
        }
        assert_eq!(app.document_ref(editor).unwrap().sql, script);
        execute_and_wait(&mut app, &ctx, "SELECT count(*) FROM script_test");
        assert_eq!(
            visible_result(&app).rows,
            vec![vec![Value::Integer(expected)]]
        );
        close_and_wait(&mut app, &ctx);
    }
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
    browse_and_wait(&mut app, &ctx, "sample");
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
    }
    app.database.workspace.expanded = None;
    for kind in [
        tiling::Kind::Map,
        tiling::Kind::Structure,
        tiling::Kind::Sql,
    ] {
        app.minimize_pane(kind);
    }
    frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    let output = frame_at(&mut app, &ctx, egui::vec2(3440.0, 1440.0), vec![]);
    for label in ["Relationships", "SQL"] {
        control(&output, label);
    }
    assert!(
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|(_, n)| n
                .label()
                .is_some_and(|s| s.starts_with("Restore Structure · ") && s.contains("columns")))
    );
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
    browse_and_wait(&mut app, &ctx, "sample");
    let table = app.database.document.id;
    app.database.document.tools.columns.hidden.insert(0);
    app.open_sql_window(false);
    let editor = app.database.document.id;
    app.database.document.sql = "SELECT 42 AS answer, 'retained' AS text".into();
    app.run_sql();
    wait(&mut app, &ctx);
    let result = app.database.document.id;
    let ticket = visible_result(&app).current;
    assert_eq!(app.database.document.kind, tiling::Kind::Result);
    app.database.document.inspect = Some((0, 0));
    app.database.document.tools.columns.hidden.insert(0);
    app.database.document.tools.columns.manual.insert(0);
    app.focus_pane(tiling::Kind::Sql);
    app.database.workspace.expanded = Some(tiling::Kind::Sql);
    ctx.enable_accesskit();
    let size = egui::vec2(3440.0, 1440.0);
    let press = |app: &mut App, label: &str| {
        frame_at(app, &ctx, size, vec![]);
        let output = frame_at(app, &ctx, size, vec![]);
        let bounds = control(&output, label);
        let pos = egui::pos2(bounds.center().x, bounds.center().y);
        assert!(app.database.workspace.pane_rects[0].1.contains(pos));
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
    assert!(app.database.ui.export_open);
    assert_eq!(app.database.workspace.focused, result);
    assert_eq!(app.database.document.id, result);
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
    assert_eq!(app.database.ui.diagram.camera.zoom, 1.0);
    press(&mut app, "Zoom in");
    assert_eq!(app.database.ui.diagram.camera.zoom, 1.25);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(&s.shape,
        egui::Shape::Text(t) if t.galley.job.text == "125%")));
    press(&mut app, "Zoom out");
    assert_eq!(app.database.ui.diagram.camera.zoom, 1.0);
    press(&mut app, "Zoom out");
    assert_eq!(app.database.ui.diagram.camera.zoom, 0.8);
    press(&mut app, "Reset zoom to 100%");
    assert_eq!(app.database.ui.diagram.camera.zoom, 1.0);
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
    browse_and_wait(&mut app, &ctx, "sample");
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
                app.database.document.ui.inspector_detached = scene == 5;
                app.database.document.ui.inspector_open = scene == 5;
                app.database.document.ui.inspector = shell::DetailMode::Cell;
                app.database.document.inspect = Some((0, 1));
                for _ in 0..3 {
                    frame_at(&mut app, &ctx, size, vec![]);
                }
                let output = frame_at(&mut app, &ctx, size, vec![]);
                let rect = control(&output, label);
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                    "{label} outside {size:?} with font {font_size}: {rect:?}"
                );
                assert_retained(&app, 0, 1, Value::Text("stored".into()));
                assert!(app.has_drafts());
            }
        }
    }
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
    app.request_database(&ctx, second_path.clone());
    app.database.writable = true;
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    let second = app.database.id;
    assert_ne!(first, second);
    execute_and_wait(&mut app, &ctx, "SELECT value FROM t");
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Text("second".into())]]
    );
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    // Sidebar rendering must never exchange parked workspaces or retained buffers.
    let focused = app.database.id;
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert_eq!(app.database.id, focused);
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
    assert_eq!(result.result.rows, vec![vec![Value::Integer(2)]]);
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
    app.stage_cell(0, 1, Value::Text("draft".into()));
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
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    let close = control(&output, &format!("Close database: {first_path}"));
    click(&mut app, &ctx, close.center());
    assert_eq!(app.database.id, first);
    assert!(app.database.confirm_close);
    let session = app.database.session.as_ref().unwrap().id;
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    let keep = control(&output, "Keep open");
    assert!(app.database.ui.sidebar_rect.contains_rect(keep));
    click(&mut app, &ctx, keep.center());
    assert!(!app.database.confirm_close);
    assert_eq!(app.database.id, second);
    assert!(app.has_drafts());
    app.select_workspace(first);
    assert_eq!(app.database.transaction, TransactionState::Active);
    app.request_close();
    assert!(app.database.confirm_close);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.database.closing && !app.database.confirm_close);
    assert_eq!(app.database.session.as_ref().unwrap().id, session);
    // Confirming inline rolls back only the selected session.
    app.request_close();
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    let confirm = control(&output, "Roll back & close");
    assert!(app.database.ui.sidebar_rect.contains_rect(confirm));
    click(&mut app, &ctx, confirm.center());
    wait_closed(&mut app, &ctx);
    app.select_workspace(second);
    assert!(app.has_drafts());
    app.database.document.drafts = None;
    execute_and_wait(&mut app, &ctx, "SELECT value FROM t");
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Text("second".into())]]
    );
    frame(&mut app, &ctx, vec![]);
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
    assert_eq!(
        app.database.document.result.rows,
        vec![vec![Value::Integer(1)]]
    );
    close_and_wait(&mut app, &ctx);
}

#[test]
fn workspace_limit_and_quit_settle_every_live_session() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.preferences.database_limit = Some(4);
    for i in 1..4 {
        let path = dir.path().join(format!("db{i}.db"));
        crate::test_support::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
        app.request_database(&ctx, path);
        inspect_selected(&mut app, &ctx);
        app.open(&ctx);
        wait(&mut app, &ctx);
    }
    assert_eq!(app.parked.len(), 3);
    assert!(!app.new_workspace(&ctx));
    assert!(app.database.status.contains("Database limit 4"));
    app.quit = true;
    app.continue_quit(&ctx);
    wait_until(&mut app, &ctx, |a| {
        a.database.session.is_none() && a.parked.values().all(|d| d.session.is_none())
    });
    assert!(app.parked.values().all(|d| !d.closing));
}

#[test]
fn schema_comparison_routes_inactive_snapshots_and_guards_target_drafts() {
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
    app.request_database(&ctx, target_path);
    app.database.writable = true;
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    let target = app.database.id;
    app.comparison.source = source;
    app.comparison.target = target;
    ctx.enable_accesskit();
    press_key(&mut app, &ctx, egui::Key::F7, egui::Modifiers::NONE);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Capture / refresh both schemas").center(),
    );
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
    frame(&mut app, &ctx, vec![]);
    assert!(app.parked[&source].tools.snapshot.snapshot.is_some());
    app.select_workspace(source);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Modified · table t").center(),
    );
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Target DDL").center());
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Open guarded migration draft in target editor").center(),
    );
    assert_eq!(app.database.id, target);
    assert!(app.database.document.migration.is_some());
    let editor = app.database.document.id;
    app.run_all(false); // Guarded drafts force atomic execution even through this action.
    wait(&mut app, &ctx);
    assert!(
        visible_result(&app)
            .completion
            .as_ref()
            .unwrap()
            .error
            .is_none()
    );
    assert_eq!(app.database.transaction, TransactionState::Autocommit);
    assert!(app.database.document.migration.is_some());
    assert!(app.database.tools.snapshot.stale);
    app.poll_comparison(&ctx);
    assert!(!app.comparison_ready());
    app.switch_document(editor);
    app.database.document.sql = "CREATE TABLE main.stale(x)".into();
    app.run_all(true);
    wait(&mut app, &ctx);
    assert!(
        app.database.status.contains("schema changed"),
        "{}",
        app.database.status
    );
    app.load_sql_window("SELECT id,value FROM t".into());
    assert!(app.database.document.migration.is_none());
    app.run_current(None);
    wait(&mut app, &ctx);
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Integer(1), Value::Null]]
    );
    close_and_wait(&mut app, &ctx);
    app.select_workspace(source);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn sidebar_search_loads_parked_columns_and_respects_scopes_and_keyboard() {
    use super::database_navigation::Scope;
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE first_table(id INTEGER PRIMARY KEY, first_value TEXT)",
            "INSERT INTO first_table VALUES(1,'keep')",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "first_table");
    let first = app.database.id;
    let retained = app.database.document.result.current;
    let second_path = dir.path().join("second.db");
    crate::test_support::create(
        &second_path,
        &["CREATE TABLE second_table(id INTEGER PRIMARY KEY, needle_column TEXT)"],
    );
    app.request_database(&ctx, second_path);
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    let second = app.database.id;
    app.select_workspace(first);
    app.parked.get_mut(&second).unwrap().ui.diagram.data = None;
    app.sidebar_search.query = "needle_column".into();
    app.sidebar_search.scope = Scope::ThisDatabase;
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert!(
        !crate::app::test_ui::nodes(&output)
            .iter()
            .any(|(_, n)| n.label() == Some("second_table.needle_column · column"))
    );
    assert!(app.parked[&second].ui.diagram.ticket.is_none());
    app.sidebar_search.scope = Scope::AllOpen;
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert_eq!(app.database.id, first);
    assert_eq!(app.database.document.result.current, retained);
    wait_until(&mut app, &ctx, |a| {
        a.parked[&second].ui.diagram.data.is_some()
    });
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    control(&output, "second_table.needle_column · column");
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(app.database.id, second);
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("second_table".into(), "needle_column".into()))
    );
    assert!(app.sidebar_search.query.is_empty());
    app.sidebar_search.query = "first_table".into();
    app.sidebar_search.scope = Scope::Databases;
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert!(
        !crate::app::test_ui::nodes(&output)
            .iter()
            .any(|(_, n)| n.label() == Some("first_table · table"))
    );
    app.sidebar_search.query = "second_table".into();
    app.open_palette();
    app.database.ui.palette_query = "second_table".into();
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert!(
        !crate::app::test_ui::nodes(&output)
            .iter()
            .any(|(_, n)| n.label() == Some("second_table · table"))
    );
    app.database.ui.palette_open = false;
    close_and_wait(&mut app, &ctx);
    app.select_workspace(first);
    close_and_wait(&mut app, &ctx);
}

mod sidebar_review;

#[test]
fn compact_record_rows_align_and_separate_null_composite_parent_links() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE parent(a INTEGER,b INTEGER,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,status TEXT,FOREIGN KEY(a,b) REFERENCES parent(a,b))",
            "INSERT INTO parent VALUES(7,8)",
            "INSERT INTO child VALUES(1,7,NULL,'pending'),(2,7,8,'ready')",
        ],
        true,
    );
    ctx.enable_accesskit();
    browse_and_wait(&mut app, &ctx, "child");
    app.open_record_window(0, 0);
    let sequence = app.database.sequence;
    let output = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.record_inspector(ui, 0)
    });
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    let id = texts.iter().find(|t| t.galley.job.text == "id").unwrap();
    let status = texts
        .iter()
        .find(|t| t.galley.job.text == "status")
        .unwrap();
    assert!((status.pos.y - id.pos.y - 72.0).abs() < 1.0);
    assert_eq!(id.pos.x, status.pos.x);
    assert!(!texts.iter().any(|t| t.galley.job.text.contains("Parent:")));
    assert!(
        !super::test_ui::nodes(&output)
            .iter()
            .any(|(_, n)| n.label() == Some("Follow parent")),
        "navigation must not take space from field values"
    );
    let relationships = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.record_relationships(ui, 0)
    });
    let parents: Vec<_> = relationships
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .filter(|(_, n)| n.label() == Some("Follow parent"))
        .collect();
    assert_eq!(parents.len(), 1, "one link per composite relationship");
    assert!(parents.iter().all(|(_, n)| n.is_disabled()));
    assert_eq!(app.database.sequence, sequence);
    assert!(!app.has_drafts());
    let relationships = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.record_relationships(ui, 1)
    });
    let follow = control(&relationships, "Follow parent").center();
    for pressed in [true, false] {
        super::test_ui::render(
            &ctx,
            egui::vec2(396.0, 244.0),
            super::test_ui::pointer(follow, pressed),
            |ui| app.record_relationships(ui, 1),
        );
    }
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.table.as_ref().unwrap().name, "parent");
    assert_eq!(
        visible_result(&app).rows,
        vec![vec![Value::Integer(7), Value::Integer(8)]]
    );
    app.close();
}

#[test]
fn record_structure_selection_value_visuals_and_revert_preserve_stored_values() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY,body TEXT,priority INTEGER,progress INTEGER,data BLOB)",
            "INSERT INTO sample VALUES(1,'original',4,75,X'00FF')",
        ],
        true,
    );
    ctx.enable_accesskit();
    browse_and_wait(&mut app, &ctx, "sample");
    app.open_record_window(0, 0);
    app.stage_cell(0, 1, Value::Text("edited".into()));
    let sequence = app.database.sequence;
    let output = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.record_inspector(ui, 0)
    });
    let pos = control(&output, "Revert body draft").center();
    for pressed in [true, false] {
        super::test_ui::render(
            &ctx,
            egui::vec2(396.0, 244.0),
            super::test_ui::pointer(pos, pressed),
            |ui| app.record_inspector(ui, 0),
        );
    }
    assert!(!app.has_drafts());
    assert_retained(&app, 0, 1, Value::Text("original".into()));
    app.open_cell(0, 3);
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("sample".into(), "progress".into()))
    );
    let output = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.structure_inspector(ui)
    });
    let pos = control(&output, "priority · INTEGER · ").center();
    for pressed in [true, false] {
        super::test_ui::render(
            &ctx,
            egui::vec2(396.0, 244.0),
            super::test_ui::pointer(pos, pressed),
            |ui| app.structure_inspector(ui),
        );
    }
    assert_eq!(app.database.document.inspect, Some((0, 2)));
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("sample".into(), "priority".into()))
    );
    assert!(app.database.document.tools.scroll_to_column);
    assert_eq!(app.database.sequence, sequence);
    let output = super::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
        app.record_inspector(ui, 0)
    });
    assert!(
        output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == "2 bytes"))
    );
    assert_retained(&app, 0, 4, Value::Blob(vec![0, 255]));
    app.close();
}

mod resources;

mod spreadsheet;

mod discoverability;

mod panels;

mod assistant;
