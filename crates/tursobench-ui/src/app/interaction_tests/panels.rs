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
    let mut tiles = vec![];
    for (sidebar, assistant) in [(true, false), (true, true), (false, true), (false, false)] {
        app.preferences.panels.sidebar = sidebar;
        app.set_assistant_shown(assistant);
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
        // The status-bar button opens and closes it; hidden, it leaves a line.
        assert_eq!(label_exists(&output, "Show Assistant"), !assistant);
        // The status-bar toggles stay reachable whatever is hidden.
        assert!(label_exists(&output, "Sidebar"));
        assert!(label_exists(&output, "Assistant"));
        let workspace = app.database.ui.workspace_rect;
        if sidebar {
            assert!(app.database.ui.sidebar_rect.right() <= workspace.left());
        }
        // The assistant is a tile in the workspace's last column.
        let data = app.database.workspace.pane_rects[1].1;
        if assistant {
            let chat = app.database.ui.assistant_rect;
            assert!(workspace.contains_rect(chat) && data.right() < chat.left());
            assert!((chat.right() - workspace.right()).abs() < 0.5);
        }
        widths.push(workspace.width());
        tiles.push(data.width());
    }
    // The sidebar takes width from the workspace, the assistant from the other
    // tiles; collapsing both leaves two thin lines.
    assert!(widths[0] == widths[1] && widths[2] == widths[3] && widths[0] < widths[3]);
    assert!(tiles[1] < tiles[0] && tiles[0] < tiles[3] && tiles[2] < tiles[3]);
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
    app.set_assistant_shown(false);
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
        "Show Assistant",
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
    app.focus_pane(Kind::Sql);
    app.database
        .workspace
        .minimized
        .extend([Kind::Record, Kind::Structure]);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    // The status bar's picture of the columns folds the one clicked.
    click(
        &mut app,
        &ctx,
        control(&output, "Column: Record, Structure and SQL").center(),
    );
    let workspace = &app.database.workspace;
    assert!(workspace.layout.all_minimized(1, &workspace.minimized));
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(label_exists(&output, "Show Record, Structure and SQL"));
    // Ctrl L reopens the focused tile's column with SQL open and the others
    // still minimized. The assistant stays hidden.
    press_key(&mut app, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    assert_eq!(
        app.database.workspace.minimized,
        BTreeSet::from([Kind::Record, Kind::Structure, Kind::Chat])
    );
    // Pressed again it folds that column, whichever column that is.
    app.focus_pane(Kind::Map);
    press_key(&mut app, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(label_exists(&output, "Show Relationships and Table data"));
    app.close();
}

#[test]
fn the_chat_expands_to_the_workspace_like_a_tile() {
    use crate::app::tiling::Kind;
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.set_assistant_shown(true);
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
    assert_eq!(app.database.workspace.pane_rects.len(), 1);
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
    assert!(!app.assistant_expanded() && app.database.workspace.expanded.is_none());
    assert!(!app.assistant_shown());
    app.close();
}
