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
