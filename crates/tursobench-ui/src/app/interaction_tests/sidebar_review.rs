use super::{
    App, TransactionState, Value, browsing, click, click_control, close_and_wait, control, egui,
    fixture, frame_at, inspect_selected, label_exists, press_key, shell, wait, wait_closed,
    wait_until,
};
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
pub(super) fn second_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("second.db");
    crate::test_support::create(
        &path,
        &["CREATE TABLE second_table(id INTEGER PRIMARY KEY)"],
    );
    path
}
/// A writable table with a staged draft whose session has gone, and its path.
fn disconnected_draft() -> (tempfile::TempDir, egui::Context, App, String) {
    let (dir, ctx, mut app) = browsing("t", "id INTEGER PRIMARY KEY, v TEXT", "(1,'x')", true);
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let path = app.database.path.clone();
    app.database.session = None;
    app.database.ready = false;
    (dir, ctx, app, path)
}
#[test]
fn disconnected_drafts_stay_reachable_and_require_a_close_decision() {
    let (dir, ctx, mut app, path) = disconnected_draft();
    let origin = app.database.id;
    app.request_database(&ctx, second_db(&dir));
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(label_exists(&output, &format!("Select database: {path}")));
    app.cancel_database_review();
    assert_eq!(app.database.id, origin);
    for quitting in [false, true] {
        app.quit = quitting;
        if quitting {
            app.continue_quit(&ctx);
        } else {
            app.request_close();
        }
        assert!(app.database.confirm_close);
        click_control(&mut app, &ctx, "Keep open");
        assert!(!app.quit && !app.database.confirm_close);
        assert!(app.has_drafts());
    }
}

#[test]
fn retained_outcomes_are_navigable_and_drafts_keep_their_file_identity() {
    let (dir, ctx, mut app, path) = disconnected_draft();
    let id = app.database.id;
    app.database.uncertain_edit = true;
    app.database.transaction = TransactionState::Unknown;
    app.request_database(&ctx, second_db(&dir));
    app.cycle_database(1);
    assert_eq!(app.database.id, id);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(!label_exists(&output, "Cancel"));
    click(
        &mut app,
        &ctx,
        control(&output, "Review retained drafts…").center(),
    );
    assert!(app.database.document.show_preview);
    assert!(app.has_drafts());
    assert_eq!(app.database.path, path);
    assert!(app.database.uncertain_edit);
}

pub(super) fn open_second(
    app: &mut App,
    ctx: &egui::Context,
    path: std::path::PathBuf,
) -> (u64, String) {
    app.request_database(ctx, path.clone());
    inspect_selected(app, ctx);
    app.open(ctx);
    wait(app, ctx);
    (app.database.id, path.to_string_lossy().into_owned())
}
#[test]
fn cancelled_review_and_closing_a_background_row_keep_the_active_database() {
    let (d, ctx, mut app) = fixture(&["CREATE TABLE items(id INTEGER PRIMARY KEY)"], false);
    let first = app.database.id;
    let second = second_db(&d);
    app.request_database(&ctx, second.clone());
    inspect_selected(&mut app, &ctx);
    click_control(&mut app, &ctx, "Cancel");
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(
        app.database.id, first,
        "Cancel restores the previous database"
    );
    let (_, path) = open_second(&mut app, &ctx, second);
    app.select_workspace(first);
    click_control(&mut app, &ctx, &format!("Close database: {path}"));
    wait_until(&mut app, &ctx, |a| {
        std::iter::once(&a.database)
            .chain(a.parked.values())
            .filter(|w| w.session.is_some())
            .count()
            == 1
    });
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.database.id, first, "the active database stays");
    assert!(app.database.session.is_some(), "and stays live");
}
#[test]
fn cancelling_a_picker_restores_the_database_and_its_drafts() {
    let (_dir, ctx, mut app) = browsing("t", "id INTEGER PRIMARY KEY, v TEXT", "(1,'x')", true);
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
    let (second, _) = open_second(&mut app, &ctx, second_db(&dir));
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
#[test]
fn search_keyboard_pages_and_opens_the_highlighted_result() {
    let tables = ["one", "two", "three", "four", "five", "six"]
        .map(|name| format!("CREATE TABLE aa_{name}(id INTEGER)"));
    let (dir, ctx, mut app) = fixture(&tables.each_ref().map(String::as_str), false);
    let active = app.database.id;
    app.preferences.discovery.record_open(second_db(&dir), 1);
    app.databases = crate::app::databases::Chooser::new(&app.preferences.discovery);
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(
        app.database.id, active,
        "empty Enter must not open a database"
    );
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    frame_at(&mut app, &ctx, SIZE, vec![egui::Event::Text("aa_".into())]);
    assert_eq!(app.sidebar_search.query, "aa_");
    press_key(&mut app, &ctx, egui::Key::ArrowDown, egui::Modifiers::NONE);
    assert_eq!(
        app.sidebar_search.index, 1,
        "first Down selects the second result"
    );
    app.sidebar_search.query = "aa_".into();
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    app.expand_window(app.database.document.id);
    let expanded = app.database.workspace.expanded;
    press_key(&mut app, &ctx, egui::Key::PageDown, egui::Modifiers::NONE);
    assert_eq!(app.sidebar_search.index, 5);
    press_key(&mut app, &ctx, egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert_eq!(app.sidebar_search.index, 4);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.sidebar_search.query.is_empty());
    assert_eq!(app.database.workspace.expanded, expanded);
    app.sidebar_search.query = "aa_".into();
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    press_key(&mut app, &ctx, egui::Key::ArrowDown, egui::Modifiers::NONE);
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(
        app.database.document.table.as_ref().unwrap().name,
        "aa_four"
    );
    close_and_wait(&mut app, &ctx);
}

#[test]
fn this_database_filters_the_schema_tree_and_opens_a_matching_column() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, needle TEXT, unrelated TEXT)",
            "CREATE TABLE other(id INTEGER PRIMARY KEY, message INTEGER REFERENCES messages(id))",
            "CREATE INDEX msg_index ON messages(needle)",
        ],
        false,
    );
    for (table, related) in [("messages", "other"), ("other", "messages")] {
        assert_eq!(app.related_tables(table), [related.to_owned()].into());
    }
    app.sidebar_search.scope = super::super::database_navigation::Scope::ThisDatabase;
    app.sidebar_search.query = "needle".into();
    app.sidebar_search.focus = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    wait(&mut app, &ctx);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    control(&output, "messages · table");
    control(&output, "needle");
    assert!(!label_exists(&output, "other · table"));
    assert!(!label_exists(&output, "unrelated"));
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(
        app.database.document.tools.highlight_column,
        Some(("messages".into(), "needle".into()))
    );
    // The tree's own rows do the same, and an index shows its definition.
    app.database.document.tools.highlight_column = None;
    click_control(&mut app, &ctx, "needle");
    assert!(app.database.document.tools.highlight_column.is_some());
    app.sidebar_search.query = "msg_index".into();
    frame_at(&mut app, &ctx, SIZE, vec![]);
    click_control(&mut app, &ctx, "msg_index");
    let ddl = app.database.document.tools.ddl.as_ref();
    assert_eq!(ddl.map(|o| o.name.as_str()), Some("msg_index"));
    close_and_wait(&mut app, &ctx);
}
#[test]
fn empty_sidebar_has_no_database_tools_and_keeps_settings_available() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame_at(&mut app, &ctx, SIZE, vec![]);
    ctx.enable_accesskit();
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    // Database tools belong to an open database; without one there is nothing to disable.
    for label in ["Tables", "History", "Saved queries", "Database checks"] {
        assert!(!label_exists(&output, label), "{label}");
    }
    click_control(&mut app, &ctx, "Settings");
    assert!(app.settings.open);
    assert_eq!(app.database.ui.sidebar, shell::ToolPanel::Schema);
}
