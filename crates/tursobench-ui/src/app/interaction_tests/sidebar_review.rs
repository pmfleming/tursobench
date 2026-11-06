use super::*;
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
fn second_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("second.db");
    crate::test_support::create(
        &path,
        &["CREATE TABLE second_table(id INTEGER PRIMARY KEY)"],
    );
    path
}
fn label_exists(output: &egui::FullOutput, label: &str) -> bool {
    crate::app::test_ui::nodes(output)
        .iter()
        .any(|(_, node)| node.label() == Some(label))
}
#[test]
fn review_disconnected_workspace_with_drafts_stays_reachable() {
    let (d, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES(1,'x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    assert!(app.has_drafts());
    let a = app.database.path.clone();
    // Simulates the post-disconnect state: no session, drafts retained.
    app.database.session = None;
    app.database.ready = false;
    let b = second_db(&d);
    app.request_database(&ctx, b);
    let o = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(
        label_exists(&o, &format!("Select database: {a}")),
        "retained-draft workspace has no row"
    );
}
#[test]
fn review_close_confirm_renders_without_session() {
    let (_d, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES(1,'x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    app.database.session = None;
    app.database.ready = false;
    app.request_close();
    assert!(
        app.database.confirm_close,
        "precondition: confirm requested"
    );
    let o = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(
        label_exists(&o, "Keep open"),
        "confirmation has nowhere to render"
    );
}

#[test]
fn retained_outcomes_are_navigable_and_drafts_keep_their_file_identity() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES(1,'x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let id = app.database.id;
    let path = app.database.path.clone();
    app.database.session = None;
    app.database.ready = false;
    app.database.uncertain_edit = true;
    app.database.transaction = TransactionState::Unknown;
    app.request_database(&ctx, second_db(&dir));
    app.cycle_database(1);
    assert_eq!(app.database.id, id);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(!label_exists(&output, "Cancel"));
    let review = control(&output, "Review retained drafts…");
    click(&mut app, &ctx, review.center());
    assert!(app.database.document.show_preview);
    assert!(app.has_drafts());
    assert_eq!(app.database.path, path);
    assert!(app.database.uncertain_edit);
}

fn open_second(app: &mut App, ctx: &egui::Context, dir: &tempfile::TempDir) -> (u64, String) {
    let path = second_db(dir);
    app.request_database(ctx, path.clone());
    inspect_selected(app, ctx);
    app.open(ctx);
    wait(app, ctx);
    (app.database.id, path.to_string_lossy().into_owned())
}
#[test]
fn review_closing_background_row_keeps_active_database() {
    let (d, ctx, mut app) = fixture(&["CREATE TABLE items(id INTEGER PRIMARY KEY)"], false);
    let first = app.database.id;
    let (_second, path) = open_second(&mut app, &ctx, &d);
    app.select_workspace(first);
    let o = frame_at(&mut app, &ctx, SIZE, vec![]);
    let x = control(&o, &format!("Close database: {path}"));
    for pressed in [true, false] {
        frame_at(
            &mut app,
            &ctx,
            SIZE,
            vec![
                egui::Event::PointerMoved(x.center()),
                pointer_button(x.center(), pressed),
            ],
        );
    }
    wait_until(&mut app, &ctx, |a| {
        std::iter::once(&a.database)
            .chain(a.parked.values())
            .filter(|w| w.session.is_some())
            .count()
            == 1
    });
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(
        app.database.id, first,
        "active database should stay {first}"
    );
    assert!(
        app.database.session.is_some(),
        "active workspace should be a live database"
    );
}
#[test]
fn review_cancel_access_review_returns_to_previous_database() {
    let (d, ctx, mut app) = fixture(&["CREATE TABLE items(id INTEGER PRIMARY KEY)"], false);
    let first = app.database.id;
    let b = second_db(&d);
    app.request_database(&ctx, b);
    inspect_selected(&mut app, &ctx);
    let o = frame_at(&mut app, &ctx, SIZE, vec![]);
    let c = control(&o, "Cancel");
    for pressed in [true, false] {
        frame_at(
            &mut app,
            &ctx,
            SIZE,
            vec![
                egui::Event::PointerMoved(c.center()),
                pointer_button(c.center(), pressed),
            ],
        );
    }
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(
        app.database.id, first,
        "Cancel should restore the previous database"
    );
}
#[test]
fn cancelling_a_picker_restores_the_database_and_its_drafts() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES(1,'x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("local".into()));
    let origin = app.database.id;
    assert!(app.new_workspace(&ctx));
    let pending = app.database.id;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.picker = Some(rx);
    tx.send(Ok(None)).unwrap();
    app.poll_picker();
    assert_eq!(app.database.id, origin);
    assert!(app.has_drafts());
    assert!(!app.parked.contains_key(&pending));
    close_and_wait(&mut app, &ctx);
}
#[test]
fn active_close_chooses_an_open_database_and_last_close_clears_access() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], true);
    let first = app.database.id;
    let (second, _) = open_second(&mut app, &ctx, &dir);
    app.request_sidebar_close(second);
    assert!(!app.database.confirm_close);
    wait_until(&mut app, &ctx, |a| a.database.id == first);
    app.request_sidebar_close(first);
    assert!(!app.database.confirm_close);
    wait_closed(&mut app, &ctx);
    assert!(app.database.path.is_empty());
    assert!(!app.database.writable);
    assert!(app.parked.values().all(|d| !d.retained()));
}
