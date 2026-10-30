use super::*;
use std::time::{Duration, Instant};
fn wait(app: &mut App, ctx: &egui::Context) {
    let start = Instant::now();
    loop {
        app.poll(ctx);
        if app.ready && app.active.is_none() && app.commit_ticket.is_none() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ui| app.render(ui),
    );
    out.textures_delta.clear();
}
#[test]
fn click_inspect_stage_no_write_failed_commit_retains_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.db");
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let c = db.connect().unwrap();
            c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)", ())
                .await
                .unwrap();
            c.execute("INSERT INTO items VALUES (1, 'original'), (2, 'other')", ())
                .await
                .unwrap();
        });
    let ctx = egui::Context::default();
    let mut app = App::new(&ctx);
    app.path = path.to_string_lossy().into_owned();
    app.exclusive = true;
    app.writable = true;
    app.open(&ctx);
    wait(&mut app, &ctx);
    app.browse_table("items".into());
    wait(&mut app, &ctx);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
    frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
    );
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    );
    assert_eq!(app.inspect, Some((0, 0)));
    app.cell_json = "{\"type\":\"Integer\",\"value\":2}".into();
    app.stage_cell(0, 0);
    assert!(app.has_drafts());
    assert!(app.commit_ticket.is_none());
    // Inspecting, rendering and staging produced no SQL request or row mutation.
    assert_eq!(app.result.rows[0][0], Value::Integer(1));
    frame(&mut app, &ctx, vec![]);
    let s = app.session.as_ref().unwrap();
    app.sequence += 1;
    s.submit(Request {
        ticket: s.ticket(app.sequence, 0),
        operation: Operation::Execute {
            sql: "SELECT id FROM items ORDER BY id".into(),
            parameters: vec![],
            budget: Budget::default(),
        },
    })
    .unwrap();
    let mut rows = vec![];
    loop {
        match s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind {
            EventKind::Rows(r) => rows.extend(r),
            EventKind::Finished(c) => {
                assert!(c.error.is_none());
                break;
            }
            _ => {}
        }
    }
    assert_eq!(rows, vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]);
    // Deliberate key collision fails and retains local drafts.
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(app.has_drafts());
    assert!(app.status.contains("drafts retained"));
    app.cell_json = "{\"type\":\"Integer\",\"value\":3}".into();
    app.stage_cell(0, 0);
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(!app.has_drafts());
    assert_eq!(app.result.rows[1][0], Value::Integer(3));
    app.close();
}
