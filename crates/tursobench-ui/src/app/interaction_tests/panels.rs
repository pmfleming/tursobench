use super::super::{ribbon::Action, tiling::Kind};
use super::{
    App, browsing, click, control, egui, fixture, frame, frame_at, label_exists, press_key, shell,
    wait,
};
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
/// Click the control named `name` in the frame `output` drew.
fn click_named(app: &mut App, ctx: &egui::Context, output: &egui::FullOutput, name: &str) {
    click(app, ctx, control(output, name).center());
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
    click_named(&mut app, &ctx, &output, "Sidebar");
    assert!(!app.preferences.panels.sidebar);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click_named(&mut app, &ctx, &output, "Assistant");
    assert!(app.preferences.panels.assistant && !app.preferences.panels.sidebar);
    press_key(&mut app, &ctx, egui::Key::I, egui::Modifiers::CTRL);
    assert!(!app.preferences.panels.assistant);
    press_key(&mut app, &ctx, egui::Key::B, egui::Modifiers::CTRL);
    assert!(app.preferences.panels.sidebar);
    press_key(&mut app, &ctx, egui::Key::B, egui::Modifiers::CTRL);
    // Searching needs the sidebar, so Ctrl P reveals it.
    press_key(&mut app, &ctx, egui::Key::P, egui::Modifiers::CTRL);
    assert!(app.preferences.panels.sidebar);
    // So do the ribbon's Find and Save query.
    for (action, panel) in [
        (Action::Find, shell::ToolPanel::Schema),
        (Action::SaveSql, shell::ToolPanel::Saved),
    ] {
        app.preferences.panels.sidebar = false;
        app.dispatch_action(action, &ctx);
        assert!(app.preferences.panels.sidebar && app.database.ui.sidebar == panel);
    }
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
    click_named(&mut app, &ctx, &output, "Column: Record, Structure and SQL");
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
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.set_assistant_shown(true);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let side = app.database.ui.assistant_rect;
    let tiles = app.database.ui.workspace_rect;
    assert!(label_exists(&output, "Minimize to header"));
    click_named(&mut app, &ctx, &output, "Expand chat to workspace");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(app.assistant_expanded());
    // The chat takes the tiles' place; the sidebar and status bar stay.
    let chat = app.database.ui.assistant_rect;
    assert!(chat.left() <= tiles.left() + 1.0 && chat.right() >= side.right() - 1.0);
    assert!(app.database.ui.sidebar_rect.right() <= chat.left());
    assert!(label_exists(&output, "Restore workspace"));
    assert_eq!(app.database.workspace.pane_rects.len(), 1);
    // Ctrl Alt Enter restores it, as it does a tile.
    let restore = egui::Modifiers::CTRL | egui::Modifiers::ALT;
    press_key(&mut app, &ctx, egui::Key::Enter, restore);
    assert!(!app.assistant_expanded());
    assert!(app.database.workspace.expanded.is_none());
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.database.ui.assistant_rect, side);
    // A tile asking for attention, such as proposed SQL, shows the tiles again.
    click_named(&mut app, &ctx, &output, "Expand chat to workspace");
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

#[test]
fn ribbon_refreshes_its_pane_discards_deliveries_and_acts_on_the_inspected_record() {
    let (_dir, ctx, mut app) = browsing(
        "t",
        "id INTEGER PRIMARY KEY, v TEXT",
        "(1,'x'),(2,'y')",
        true,
    );
    app.dispatch_action(Action::Refresh, &ctx);
    let reload = app.database.document.active;
    assert!(reload.is_some(), "Refresh reloads the rows");
    // Discarding suppresses delivery; the statement itself still finishes.
    app.dispatch_action(Action::Discard, &ctx);
    assert_eq!(app.database.document.discard_ticket, reload);
    wait(&mut app, &ctx);
    assert!(app.database.document.result.rows.is_empty());
    // F5 reloads the open table, and the schema from the map.
    press_key(&mut app, &ctx, egui::Key::F5, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert_eq!(app.database.document.result.rows.len(), 2);
    app.database.document.ui.nav = shell::Nav::Overview;
    press_key(&mut app, &ctx, egui::Key::F5, egui::Modifiers::NONE);
    assert!(app.database.catalog_ticket.is_some() && app.database.ui.diagram.ticket.is_some());
    wait(&mut app, &ctx);
    app.database.document.ui.nav = shell::Nav::Table;
    for (kind, inspects) in [(Kind::Map, false), (Kind::Structure, true)] {
        app.focus_pane(kind);
        app.dispatch_action(Action::Refresh, &ctx);
        assert!(app.database.catalog_ticket.is_some(), "{kind:?}");
        assert_eq!(
            app.database.document.tools.inspection_ticket.is_some(),
            inspects
        );
        wait(&mut app, &ctx);
    }
    app.focus_pane(Kind::Data);
    app.dispatch_action(Action::CopyDdl, &ctx);
    app.database.document.inspect = Some((1, 1));
    app.dispatch_action(Action::CopyRecord, &ctx);
    let commands = frame(&mut app, &ctx, vec![]).platform_output.commands;
    use egui::OutputCommand::CopyText;
    let [CopyText(ddl), CopyText(record)] = &commands[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(ddl, "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)");
    assert!(record.ends_with("\"2\"\t\"y\"\r\n"), "{record}");
    app.dispatch_action(Action::CloneRecord, &ctx);
    assert!(
        app.database.document.show_insert,
        "a clone is only prepared"
    );
    app.database.document.show_insert = false;
    app.dispatch_action(Action::DeleteRecord, &ctx);
    app.dispatch_action(Action::Review, &ctx);
    assert!(app.database.document.has_drafts() && app.database.document.show_preview);
}
