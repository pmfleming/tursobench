use super::{
    App, TransactionState, Value, browse_and_wait, click, close_and_wait, control, egui, fixture,
    frame_at, inspect_selected, press_key, shell, wait, wait_closed, wait_until,
};
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
fn disconnected_drafts_stay_reachable_and_require_a_close_decision() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES(1,'x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.stage_cell(0, 1, Value::Text("draft".into()));
    let origin = app.database.id;
    let path = app.database.path.clone();
    app.database.session = None;
    app.database.ready = false;
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
        let output = frame_at(&mut app, &ctx, SIZE, vec![]);
        click(&mut app, &ctx, control(&output, "Keep open").center());
        assert!(!app.quit && !app.database.confirm_close);
        assert!(app.has_drafts());
    }
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
    click(&mut app, &ctx, x.center());
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
    click(&mut app, &ctx, c.center());
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
#[test]
fn search_keyboard_pages_and_opens_the_highlighted_result() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE aa_one(id INTEGER)",
            "CREATE TABLE aa_two(id INTEGER)",
            "CREATE TABLE aa_three(id INTEGER)",
            "CREATE TABLE aa_four(id INTEGER)",
            "CREATE TABLE aa_five(id INTEGER)",
            "CREATE TABLE aa_six(id INTEGER)",
        ],
        false,
    );
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
            "CREATE TABLE other(id INTEGER PRIMARY KEY)",
        ],
        false,
    );
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
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let settings = control(&output, "Settings");
    click(&mut app, &ctx, settings.center());
    assert!(app.settings.open);
    assert_eq!(app.database.ui.sidebar, shell::ToolPanel::Schema);
}
