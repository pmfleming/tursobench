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
    // The same button and Ctrl K both close what they opened.
    click(&mut app, &ctx, commands.center());
    assert!(!app.database.ui.palette_open);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(app.database.ui.palette_open);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(!app.database.ui.palette_open);
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

#[test]
fn palette_ends_the_transaction_it_began() {
    use tursobench_core::TransactionState;
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let choose = |app: &mut App, query: &str| {
        app.open_palette();
        app.database.ui.palette_query = query.into();
        frame(app, &ctx, vec![]);
        press_key(app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        wait(app, &ctx);
    };
    for (end, sql) in [("commit", "COMMIT"), ("rollback", "ROLLBACK")] {
        choose(&mut app, "begin");
        assert_eq!(app.database.transaction, TransactionState::Active);
        choose(&mut app, end);
        assert_eq!(
            app.database.transaction,
            TransactionState::Autocommit,
            "{sql}"
        );
    }
    // Outside a transaction ROLLBACK is listed but cannot run.
    let history = app.preferences.query_history.len();
    app.open_palette();
    app.database.ui.palette_query = "rollback".into();
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    wait(&mut app, &ctx);
    assert!(app.database.ui.palette_open);
    assert_eq!(app.preferences.query_history.len(), history);
    app.database.ui.palette_open = false;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn palette_greys_out_commands_that_cannot_run_and_explains_why() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    app.open_palette();
    let mut disabled = |name: &str| {
        app.database.ui.palette_query = name.into();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        crate::app::test_ui::nodes(&output)
            .iter()
            // Rows with a shortcut are named "<label> <shortcut>".
            .find(|(_, n)| n.label().is_some_and(|l| l.starts_with(name)))
            .unwrap_or_else(|| panic!("{name} is not listed"))
            .1
            .is_disabled()
    };
    for name in ["Close database", "New row (staged)", "Copy selected cells"] {
        assert!(disabled(name), "{name}");
    }
    for name in ["Open database", "Settings", "Toggle theme"] {
        assert!(!disabled(name), "{name}");
    }
    // Runnable commands come first, so Enter on an empty query runs one.
    app.database.ui.palette_query.clear();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    // Palette rows are the buttons right of the sidebar, below the search field.
    let rows: Vec<_> = crate::app::test_ui::nodes(&output)
        .iter()
        .filter(|(_, n)| n.role() == egui::accesskit::Role::Button)
        .filter(|(_, n)| n.label().is_some_and(|l| l != "Close window"))
        .filter_map(|(_, n)| n.bounds().map(|b| (b, n.is_disabled())))
        .filter(|(b, _)| b.x0 > 300.0 && b.x1 < 1100.0)
        .collect();
    let lowest_enabled = rows
        .iter()
        .filter(|r| !r.1)
        .map(|r| r.0.y0)
        .fold(0.0, f64::max);
    let highest_disabled = rows
        .iter()
        .filter(|r| r.1)
        .map(|r| r.0.y0)
        .fold(f64::MAX, f64::min);
    assert!(rows.iter().any(|r| !r.1) && rows.iter().any(|r| r.1));
    assert!(lowest_enabled < highest_disabled);
    // Selecting an unavailable command explains it and Enter leaves it alone.
    app.database.ui.palette_query = "close".into();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(
        &s.shape,
        egui::Shape::Text(t) if t.galley.job.text == "No database is open"
    )));
    press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    assert!(app.database.ui.palette_open);
}

#[test]
fn palette_filter_and_review_need_a_table_and_staged_edits() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(1,'a')",
        ],
        true,
    );
    let choose = |app: &mut App, query: &str| {
        app.open_palette();
        app.database.ui.palette_query = query.into();
        frame(app, &ctx, vec![]);
        press_key(app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    };
    // Open but not on a table: neither command may set hidden state.
    choose(&mut app, "filter rows");
    assert!(app.database.ui.palette_open);
    assert!(!app.database.document.ui.filter_open);
    app.database.ui.palette_open = false;
    choose(&mut app, "review staged");
    assert!(app.database.ui.palette_open);
    assert!(!app.database.document.show_preview);
    app.database.ui.palette_open = false;
    browse_and_wait(&mut app, &ctx, "t");
    choose(&mut app, "filter rows");
    assert!(!app.database.ui.palette_open);
    assert!(app.database.document.ui.filter_open);
    close_and_wait(&mut app, &ctx);
}

#[test]
fn palette_names_match_the_app_and_old_words_still_find_them() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    app.open_palette();
    for (query, name) in [
        ("record", "Open record"),
        ("inspector", "Open record"),
        ("relationships", "Relationships"),
        ("map", "Relationships"),
        ("pragma", "Database checks"),
        ("help", "Keyboard and safety reference"),
    ] {
        app.database.ui.palette_query = query.into();
        frame(&mut app, &ctx, vec![]);
        let output = frame(&mut app, &ctx, vec![]);
        let rows: Vec<_> = crate::app::test_ui::nodes(&output)
            .iter()
            .filter_map(|(_, n)| n.bounds().zip(n.label()))
            .filter(|(b, _)| b.x0 > 300.0 && b.x1 < 1100.0 && b.y0 > 150.0)
            .map(|(b, label)| (b.y0 as i64, label.to_owned()))
            .collect();
        let first = rows.iter().min().map(|(_, l)| l.as_str());
        assert!(
            first.is_some_and(|l| l.starts_with(name)),
            "{query}: {rows:?}"
        );
    }
}

#[test]
fn palette_empty_result_points_to_object_search() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    app.open_palette();
    app.database.ui.palette_query = "zzzz".into();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(output.shapes.iter().any(|s| matches!(
        &s.shape,
        egui::Shape::Text(t) if t.galley.job.text.starts_with("No matching commands")
    )));
}

#[test]
fn palette_closes_on_a_click_outside_but_not_inside() {
    let (ctx, mut app) = crate::app::test_ui::app();
    frame(&mut app, &ctx, vec![]);
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    frame(&mut app, &ctx, vec![]);
    // Clicking inside, on the search field, keeps it open.
    let window = ctx
        .memory(|m| m.area_rect(egui::Id::unique("command-palette")))
        .unwrap();
    click(&mut app, &ctx, window.center_top() + egui::vec2(0.0, 70.0));
    assert!(app.database.ui.palette_open);
    click(
        &mut app,
        &ctx,
        egui::pos2(window.right() + 100.0, window.center().y),
    );
    assert!(!app.database.ui.palette_open);
}

#[test]
fn escape_closes_the_palette_before_a_cell_edit_beneath_it() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO t VALUES(1,'a')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    app.start_inline(0, 1);
    assert!(app.database.document.tools.inline.is_some());
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    assert!(app.database.ui.palette_open);
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!app.database.ui.palette_open);
    assert!(app.database.document.tools.inline.is_some());
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.database.document.tools.inline.is_none());
    close_and_wait(&mut app, &ctx);
}

#[test]
fn palette_holds_the_database_and_hands_ctrl_p_to_the_sidebar_search() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let second = dir.path().join("second.db");
    crate::test_support::create(&second, &["CREATE TABLE u(id INTEGER PRIMARY KEY)"]);
    app.request_database(&ctx, second);
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    let id = app.database.id;
    press_key(&mut app, &ctx, egui::Key::K, egui::Modifiers::CTRL);
    for key in [egui::Key::PageDown, egui::Key::PageUp] {
        press_key(&mut app, &ctx, key, egui::Modifiers::CTRL);
        assert_eq!(app.database.id, id, "{key:?}");
        assert!(app.database.ui.palette_open);
    }
    press_key(&mut app, &ctx, egui::Key::P, egui::Modifiers::CTRL);
    assert!(!app.database.ui.palette_open);
    frame(&mut app, &ctx, vec![]);
    assert!(ctx.memory(|m| m.has_focus(egui::Id::unique("sidebar-search"))));
    // Without the palette, the same keys switch database again.
    press_key(&mut app, &ctx, egui::Key::PageDown, egui::Modifiers::CTRL);
    assert_ne!(app.database.id, id);
}
