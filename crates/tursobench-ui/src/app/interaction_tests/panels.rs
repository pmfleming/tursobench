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
        assert_eq!(label_exists(&output, "Hide assistant"), assistant);
        // The status-bar toggles stay reachable whatever is hidden.
        assert!(label_exists(&output, "Sidebar"));
        assert!(label_exists(&output, "Assistant"));
        let workspace = app.database.ui.workspace_rect;
        if sidebar {
            assert!(app.database.ui.sidebar_rect.right() <= workspace.left());
        }
        widths.push(workspace.width());
    }
    // Shown panels take width from the workspace; hiding both gives it all back.
    assert!(widths[1] < widths[0] && widths[0] < widths[3] && widths[2] < widths[3]);
    assert!(widths[3] >= SIZE.x - 20.0);
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
