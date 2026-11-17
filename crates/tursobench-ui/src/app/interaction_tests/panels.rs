use super::{click, control, egui, fixture, frame_at, press_key};
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
fn label_exists(output: &egui::FullOutput, label: &str) -> bool {
    crate::app::test_ui::nodes(output)
        .iter()
        .any(|(_, node)| node.label() == Some(label))
}

#[test]
fn sidebar_and_assistant_hide_independently_and_the_workspace_reclaims_their_width() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let mut widths = vec![];
    for (sidebar, assistant) in [(true, false), (true, true), (false, true), (false, false)] {
        app.preferences.panels.sidebar = sidebar;
        app.preferences.panels.assistant = assistant;
        let output = frame_at(&mut app, &ctx, SIZE, vec![]);
        let output = if output.platform_output.accesskit_update.is_none() {
            frame_at(&mut app, &ctx, SIZE, vec![])
        } else {
            output
        };
        assert_eq!(
            app.database.ui.sidebar_rect != egui::Rect::NOTHING,
            sidebar,
            "sidebar {sidebar} assistant {assistant}"
        );
        assert_eq!(
            app.database.ui.assistant_rect != egui::Rect::NOTHING,
            assistant
        );
        // The status-bar button opens and closes it; a closed panel leaves a line.
        assert_eq!(label_exists(&output, "Show assistant"), !assistant);
        // The status-bar toggles stay reachable whatever is hidden.
        assert!(label_exists(&output, "Sidebar"));
        assert!(label_exists(&output, "Assistant"));
        let workspace = app.database.ui.workspace_rect;
        if sidebar {
            assert!(app.database.ui.sidebar_rect.right() <= workspace.left());
        }
        if assistant {
            assert!(workspace.right() <= app.database.ui.assistant_rect.left());
        }
        widths.push(workspace.width());
    }
    // Shown panels take width from the workspace; collapsing both leaves two thin lines.
    assert!(widths[1] < widths[0] && widths[0] < widths[3] && widths[2] < widths[3]);
    assert!(widths[3] >= SIZE.x - 60.0);
    app.close();
}

#[test]
fn toggles_shortcuts_and_search_bring_panels_back() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(&mut app, &ctx, control(&output, "Sidebar").center());
    assert!(!app.preferences.panels.sidebar);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(&mut app, &ctx, control(&output, "Assistant").center());
    assert!(app.preferences.panels.assistant && !app.preferences.panels.sidebar);
    press_key(&mut app, &ctx, egui::Key::I, egui::Modifiers::CTRL);
    assert!(!app.preferences.panels.assistant);
    press_key(&mut app, &ctx, egui::Key::B, egui::Modifiers::CTRL);
    assert!(app.preferences.panels.sidebar);
    press_key(&mut app, &ctx, egui::Key::B, egui::Modifiers::CTRL);
    // Searching needs the sidebar, so Ctrl P reveals it.
    press_key(&mut app, &ctx, egui::Key::P, egui::Modifiers::CTRL);
    assert!(app.preferences.panels.sidebar);
    // A close confirmation lives in the sidebar and must stay visible.
    app.preferences.panels.sidebar = false;
    app.database.confirm_close = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(app.database.ui.sidebar_rect != egui::Rect::NOTHING);
    app.database.confirm_close = false;
    app.close();
}

#[test]
fn collapsed_areas_leave_lines_that_bring_them_back() {
    use crate::app::tiling::Kind;
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.preferences.panels.sidebar = false;
    app.preferences.panels.assistant = false;
    app.database
        .workspace
        .minimized
        .extend([Kind::Record, Kind::Structure, Kind::Sql]);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let mut output = frame_at(&mut app, &ctx, SIZE, vec![]);
    // The folded column gives its width to the left tiles.
    let data = app.database.workspace.pane_rects[1].1;
    assert!(data.width() > app.database.workspace.area.width() - 40.0);
    for name in [
        "Show databases",
        "Show assistant",
        "Show Record, Structure and SQL",
    ] {
        click(&mut app, &ctx, control(&output, name).center());
        output = frame_at(&mut app, &ctx, SIZE, vec![]);
        assert!(!label_exists(&output, name), "{name} stays a line");
    }
    assert!(app.preferences.panels.sidebar && app.preferences.panels.assistant);
    assert!(app.database.workspace.minimized.is_empty());
    app.close();
}

#[test]
fn the_tiles_toggle_folds_the_column_and_reopens_what_was_open() {
    use crate::app::tiling::Kind;
    use std::collections::BTreeSet;
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.database
        .workspace
        .minimized
        .extend([Kind::Record, Kind::Structure]);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Record, Structure and SQL").center(),
    );
    assert!(app.tiles_folded());
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(label_exists(&output, "Show Record, Structure and SQL"));
    // Ctrl L unfolds it with SQL open and the others still minimized.
    press_key(&mut app, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    assert_eq!(
        app.database.workspace.minimized,
        BTreeSet::from([Kind::Record, Kind::Structure])
    );
    app.close();
}

#[test]
fn the_chat_expands_to_the_workspace_like_a_tile() {
    use crate::app::tiling::Kind;
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.preferences.panels.assistant = true;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let side = app.database.ui.assistant_rect;
    let tiles = app.database.ui.workspace_rect;
    assert!(label_exists(&output, "Minimize to header"));
    click(
        &mut app,
        &ctx,
        control(&output, "Expand chat to workspace").center(),
    );
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(app.assistant_expanded());
    // The chat takes the tiles' place; the sidebar and status bar stay.
    let chat = app.database.ui.assistant_rect;
    assert!(chat.left() <= tiles.left() + 1.0 && chat.right() >= side.right() - 1.0);
    assert!(app.database.ui.sidebar_rect.right() <= chat.left());
    assert!(label_exists(&output, "Restore workspace"));
    assert!(!label_exists(&output, "Minimize to header"));
    // Ctrl Alt Enter restores it, as it does a tile.
    press_key(
        &mut app,
        &ctx,
        egui::Key::Enter,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
    );
    assert!(!app.assistant_expanded());
    assert!(app.database.workspace.expanded.is_none());
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.database.ui.assistant_rect, side);
    // A tile asking for attention, such as proposed SQL, shows the tiles again.
    click(
        &mut app,
        &ctx,
        control(&output, "Expand chat to workspace").center(),
    );
    assert!(app.assistant_expanded());
    app.focus_pane(Kind::Sql);
    assert!(!app.assistant_expanded());
    // Hiding the chat ends the expansion.
    app.toggle_assistant_expanded();
    app.toggle_assistant();
    assert!(!app.assistant_expanded() && !app.database.ui.assistant_expanded);
    app.close();
}
