use super::*;

#[test]
fn sidebar_foot_opens_help_and_commands_without_a_database() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, vec![]);
    let help = control(&output, "Keyboard and safety reference");
    let commands = control(&output, "Commands");
    assert!(commands.right() <= help.left(), "{commands:?} {help:?}");
    click(&mut app, &ctx, help.center());
    assert!(app.database.tools.help);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.database.tools.help);
    click(&mut app, &ctx, commands.center());
    assert!(app.database.ui.palette_open);
    assert!(app.database.session.is_none());
}

#[test]
fn headers_name_every_sort_level_in_engine_order() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE sample(id INTEGER PRIMARY KEY, label TEXT, number INTEGER)",
            "INSERT INTO sample VALUES(1,'a',42)",
        ],
        false,
    );
    browse_and_wait(&mut app, &ctx, "sample");
    frame(&mut app, &ctx, vec![]);
    ctx.enable_accesskit();
    let browse = app.database.document.browse.as_mut().unwrap();
    browse.sort = Some(2);
    browse.descending = true;
    let output = frame(&mut app, &ctx, vec![]);
    control(&output, "Column number descending");
    // A repeated primary column is ignored, exactly as the engine request ignores it.
    let browse = app.database.document.browse.as_mut().unwrap();
    browse.options.sorts = vec![(2, false), (1, false)];
    let output = frame(&mut app, &ctx, vec![]);
    control(&output, "Column number descending, sort level 1");
    control(&output, "Column label ascending, sort level 2");
    control(&output, "Column id PK1");
    close_and_wait(&mut app, &ctx);
}

#[test]
fn database_tools_sit_under_the_open_database_and_application_controls_at_the_foot() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    frame(&mut app, &ctx, vec![]);
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, vec![]);
    let row = control(&output, &format!("Select database: {}", app.database.path));
    let settings = control(&output, "Settings");
    let sidebar = app.database.ui.sidebar_rect;
    let mut previous = sidebar.left();
    for name in ["Tables", "History", "Saved queries", "Database checks"] {
        let tool = control(&output, name);
        assert!(sidebar.contains_rect(tool), "{name}: {tool:?}");
        assert!(tool.top() >= row.bottom(), "{name}: {tool:?} {row:?}");
        assert!(tool.bottom() <= row.bottom() + 48.0, "{name}: {tool:?}");
        assert!(tool.bottom() < settings.top(), "{name}: {tool:?}");
        assert!(tool.left() >= previous, "{name}: {tool:?}");
        previous = tool.right();
    }
    assert!(sidebar.contains_rect(settings));
    let history = control(&output, "History");
    click(&mut app, &ctx, history.center());
    assert_eq!(app.database.ui.sidebar, shell::ToolPanel::History);
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Tables").center());
    assert_eq!(app.database.ui.sidebar, shell::ToolPanel::Schema);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn palette_runs_the_best_match_not_the_first_listed() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    for (query, opened) in [
        ("set", (|a: &App| a.settings.open) as fn(&App) -> bool),
        ("help", |a: &App| a.database.tools.help),
    ] {
        app.open_palette();
        app.database.ui.palette_query = query.into();
        frame(&mut app, &ctx, vec![]);
        press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(opened(&app), "{query}");
        assert!(
            !app.database.document.ui.sql_open,
            "{query} ran the SQL editor"
        );
        app.settings.open = false;
        app.database.tools.help = false;
    }
}

#[test]
fn palette_copy_with_nothing_selected_leaves_the_clipboard_alone() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    app.execute_command(&ctx, super::super::commands::Command::CopySelection);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(
        !output
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(_)))
    );
    assert_eq!(app.database.status, "Select cells to copy");
}
