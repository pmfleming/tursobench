use super::*;

#[test]
fn count_and_memory_limits_admit_more_than_four_and_preserve_existing_work() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let first = app.database.id;
    app.preferences.database_limit = Some(7);
    app.preferences.memory_budget_mib = Some(7 * crate::resources::DATABASE_RESERVATION_MIB);
    for index in 1..6 {
        let path = dir.path().join(format!("db{index}.db"));
        crate::test_support::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
        app.request_database(&ctx, path);
        inspect_selected(&mut app, &ctx);
        app.open(&ctx);
        wait(&mut app, &ctx);
    }
    assert_eq!(
        app.parked.values().filter(|d| d.session.is_some()).count() + 1,
        6
    );
    let recent = app.database.id;
    assert!(app.new_workspace(&ctx));
    app.preferences.memory_budget_mib = Some(6 * crate::resources::DATABASE_RESERVATION_MIB);
    app.set_database_path(dir.path().join("db1.db").to_str().unwrap().into());
    // Duplicate selection still works while admission is blocked.
    app.open(&ctx);
    assert!(app.database.session.is_some());
    app.preferences.database_limit = Some(2);
    assert!(!app.new_workspace(&ctx));
    assert!(app.database.status.contains("Database limit 2"));
    assert_eq!(
        std::iter::once(&app.database)
            .chain(app.parked.values())
            .filter(|d| d.session.is_some())
            .count(),
        6
    );
    // Changing access reuses an existing slot even after reducing the limit.
    app.select_workspace(recent);
    assert!(app.databases.inspection.ready(&app.database.path));
    app.database.pending_access = Some(true);
    app.close();
    wait(&mut app, &ctx);
    assert!(app.database.writable && app.database.ready);
    assert_eq!(
        std::iter::once(&app.database)
            .chain(app.parked.values())
            .filter(|d| d.session.is_some())
            .count(),
        6
    );
    app.select_workspace(first);
    assert_eq!(app.database.id, first);
    app.quit = true;
    app.continue_quit(&ctx);
    wait_until(&mut app, &ctx, |a| {
        a.database.session.is_none() && a.parked.values().all(|d| d.session.is_none())
    });
}

#[test]
fn a_pending_or_cli_open_cannot_bypass_a_reduced_budget() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    let first = app.database.id;
    let path = dir.path().join("pending.db");
    crate::test_support::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
    app.preferences.memory_budget_mib = Some(1024);
    app.request_database(&ctx, path.clone());
    inspect_selected(&mut app, &ctx);
    app.preferences.memory_budget_mib = Some(512);
    app.open(&ctx);
    assert!(app.database.session.is_none());
    assert!(app.database.status.contains("Database limit 1"));
    app.cancel_database_review();
    assert_eq!(app.database.id, first);
    app.close();
    wait_closed(&mut app, &ctx);
    app.memory.available_mib = Some(1024);
    app.preferences.memory_budget_mib = None;
    app.set_database_path(path.to_str().unwrap().into());
    app.open(&ctx);
    assert!(app.database.session.is_none());
    assert!(app.database.status.contains("Database limit 0"));
    let output = frame_at(&mut app, &ctx, egui::vec2(1280.0, 720.0), vec![]);
    assert!(
        output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == "0/0"))
    );
    app.preferences.memory_budget_mib = Some(1024);
    inspect_selected(&mut app, &ctx);
    app.open(&ctx);
    wait(&mut app, &ctx);
    close_and_wait(&mut app, &ctx);
}
