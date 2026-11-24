use super::{App, egui, fixture, frame, pane, pointer_button, press_key};
use crate::app::tiling::Kind::{self, Chat, Data, Map, Record, Sql, Structure};
use crate::app::tiling::Layout;

fn columns(app: &App) -> Vec<Vec<Kind>> {
    let layout = &app.database.workspace.layout;
    layout.columns.iter().map(|c| c.tiles.clone()).collect()
}
fn grip(app: &App, kind: Kind) -> egui::Pos2 {
    let grips = &app.database.workspace.grips;
    grips.iter().find(|(k, _)| *k == kind).unwrap().1.center()
}
/// Press on the tile's grip, move to `to` in two steps, then release unless `hold`.
fn drag(app: &mut App, ctx: &egui::Context, kind: Kind, to: egui::Pos2, hold: bool) {
    drag_from(app, ctx, grip(app, kind), to, hold);
}
fn drag_from(app: &mut App, ctx: &egui::Context, from: egui::Pos2, to: egui::Pos2, hold: bool) {
    frame(app, ctx, crate::app::test_ui::pointer(from, true));
    frame(
        app,
        ctx,
        vec![egui::Event::PointerMoved(from + (to - from) / 2.0)],
    );
    frame(app, ctx, vec![egui::Event::PointerMoved(to)]);
    frame(app, ctx, vec![]);
    if !hold {
        frame(app, ctx, vec![pointer_button(to, false)]);
        frame(app, ctx, vec![]);
    }
}

#[test]
fn tiles_move_between_columns_by_grip_and_keys_and_each_database_keeps_its_own_layout() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    app.set_assistant_shown(true);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.workspace.layout, Layout::default());
    assert_eq!(app.database.workspace.grips.len(), 6);

    // Dropped on another column, the tile joins it at the slot under the pointer.
    let map = pane(&app, Map);
    let slot = egui::pos2(map.center().x, map.bottom() - 10.0);
    drag(&mut app, &ctx, Structure, slot, false);
    assert_eq!(
        columns(&app),
        [vec![Map, Structure, Data], vec![Record, Sql], vec![Chat]]
    );
    assert_eq!(app.database.workspace.active, Structure);
    let (above, moved, below) = (pane(&app, Map), pane(&app, Structure), pane(&app, Data));
    assert!(above.bottom() < moved.top() && moved.bottom() < below.top());
    assert_eq!(moved.x_range(), above.x_range());

    // Dropped past the last column, it starts a new column on the right.
    let area = app.database.workspace.area;
    let past = egui::pos2(area.right() - 2.0, area.center().y);
    drag(&mut app, &ctx, Sql, past, false);
    assert_eq!(
        columns(&app),
        [
            vec![Map, Structure, Data],
            vec![Record],
            vec![Chat],
            vec![Sql]
        ]
    );
    assert!((pane(&app, Sql).right() - area.right()).abs() < 0.5);
    assert!(pane(&app, Chat).right() < pane(&app, Sql).left());

    // Between two columns no column starts: the tile joins the nearer one.
    let gap = egui::pos2(pane(&app, Record).right() + 3.0, area.bottom() - 20.0);
    drag(&mut app, &ctx, Sql, gap, false);
    assert_eq!(
        columns(&app),
        [vec![Map, Structure, Data], vec![Record, Sql], vec![Chat]],
        "the column Sql left is gone"
    );

    // Escape during a drag leaves the tile where it was.
    let before = columns(&app);
    let target = pane(&app, Map).center();
    drag(&mut app, &ctx, Chat, target, true);
    assert!(
        app.database
            .workspace
            .drag
            .is_some_and(|d| d.target.is_some())
    );
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    frame(&mut app, &ctx, vec![pointer_button(target, false)]);
    frame(&mut app, &ctx, vec![]);
    assert!(app.database.workspace.drag.is_none());
    assert_eq!(columns(&app), before);

    // Alt Shift arrows move the focused tile. Left and right join the
    // neighbouring column; emptying a column removes it.
    const MOVE: egui::Modifiers = egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT);
    app.focus_pane(Chat);
    press_key(&mut app, &ctx, egui::Key::ArrowLeft, MOVE);
    assert_eq!(
        columns(&app),
        [vec![Map, Structure, Data], vec![Chat, Record, Sql]]
    );
    press_key(&mut app, &ctx, egui::Key::ArrowDown, MOVE);
    assert_eq!(columns(&app)[1], [Record, Chat, Sql]);
    // Right from the last column starts a new one; there is none to the left of the first.
    press_key(&mut app, &ctx, egui::Key::ArrowRight, MOVE);
    assert_eq!(columns(&app)[2], [Chat]);
    press_key(&mut app, &ctx, egui::Key::ArrowRight, MOVE);
    assert_eq!(columns(&app).len(), 3, "already alone in the last column");
    app.focus_pane(Map);
    press_key(&mut app, &ctx, egui::Key::ArrowLeft, MOVE);
    press_key(&mut app, &ctx, egui::Key::ArrowUp, MOVE);
    assert_eq!(columns(&app)[0], [Map, Structure, Data]);

    // The arrangement is saved with this database and comes back with it.
    let arranged = app.database.workspace.layout.clone();
    assert_ne!(arranged, Layout::default());
    app.database.workspace.layout = Layout::default();
    app.restore_workspace();
    assert_eq!(app.database.workspace.layout, arranged);

    // Reset returns this database to the default layout.
    app.reset_layout();
    assert_eq!(app.database.workspace.layout, Layout::default());
    app.database.workspace.layout = arranged;
    app.restore_workspace();
    assert_eq!(app.database.workspace.layout, Layout::default());
    frame(&mut app, &ctx, vec![]);
    assert_eq!(app.database.workspace.pane_rects.len(), 6);
    app.close();
}

#[test]
fn the_assistant_is_a_tile_that_minimizes_folds_and_stacks_like_the_others() {
    let (_dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    // Hidden by default: alone in its column, it is the line at the right edge.
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    assert!(!app.assistant_shown());
    assert_eq!(app.database.workspace.pane_rects.len(), 5);
    // Its line drags left or right; the column moves whole and stays a line.
    let lanes = app.workspace_frame().columns;
    let (line, middle) = (lanes[2].rect.center(), lanes[1].rect.left() + 10.0);
    drag_from(&mut app, &ctx, line, egui::pos2(middle, line.y), false);
    assert_eq!(
        columns(&app),
        [vec![Map, Data], vec![Chat], vec![Record, Structure, Sql]]
    );
    assert!(!app.assistant_shown());
    let lanes = app.workspace_frame().columns;
    let (line, end) = (lanes[1].rect.center(), lanes[2].rect.right() - 5.0);
    drag_from(&mut app, &ctx, line, egui::pos2(end, line.y), false);
    assert_eq!(app.database.workspace.layout, Layout::default());
    assert!(!app.assistant_shown());
    // Restoring every tile leaves the assistant as it is.
    app.minimize_pane(Map);
    assert!(app.tiles_restorable());
    app.retile();
    assert!(!app.tiles_restorable() && !app.assistant_shown());
    // Stacked under other tiles and hidden, it is a header like theirs.
    app.focus_pane(Chat);
    assert!(app.assistant_shown());
    press_key(
        &mut app,
        &ctx,
        egui::Key::ArrowLeft,
        egui::Modifiers::ALT.plus(egui::Modifiers::SHIFT),
    );
    assert_eq!(columns(&app)[1], [Chat, Record, Structure, Sql]);
    app.set_assistant_shown(false);
    frame(&mut app, &ctx, vec![]);
    let bar = pane(&app, Chat);
    assert!((bar.height() - crate::app::tiling::BAR).abs() < 0.1);
    assert_eq!(bar.x_range(), pane(&app, Record).x_range());
    // Ctrl I brings it back in place.
    press_key(&mut app, &ctx, egui::Key::I, egui::Modifiers::CTRL);
    frame(&mut app, &ctx, vec![]);
    assert!(app.assistant_shown());
    assert!(pane(&app, Chat).height() > 96.0);
    assert_eq!(app.database.ui.assistant_rect, pane(&app, Chat));
    app.close();
}
