use super::*;
use crate::app::{
    results::CellKind,
    spreadsheet::{Mapping, Sheet},
};

fn select(app: &mut App, a: (usize, usize), b: (usize, usize)) {
    app.select_cell(a.0, a.1, false);
    app.select_cell(b.0, b.1, true);
}
fn edits(app: &App) -> Vec<tursobench_core::Edit> {
    app.database
        .document
        .drafts
        .as_ref()
        .map_or_else(Vec::new, |d| d.edits.clone())
}

#[test]
fn spreadsheet_paste_respects_visible_order_types_and_atomic_undo() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT, n INTEGER, hidden TEXT)",
            "INSERT INTO t VALUES(1,'one',10,'secret'),(2,'two',20,'secret2')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    app.database.document.tools.columns.order = vec![0, 2, 3, 1];
    app.database.document.tools.columns.hidden.insert(3);
    select(&mut app, (0, 2), (0, 2));
    app.stage_cell(0, 1, Value::Text("earlier".into()));
    let before = edits(&app);
    assert!(app.paste_cells("9007199254740993\t\"a\tb\"\r\n42\t\n"));
    assert_staged(&app, 0, 2, Value::Integer(9007199254740993));
    assert_staged(&app, 0, 1, Value::Text("a\tb".into()));
    assert_staged(&app, 1, 2, Value::Integer(42));
    assert_staged(&app, 1, 1, Value::Text(String::new()));
    assert_retained(&app, 0, 3, Value::Text("secret".into()));
    app.undo_grid_edit();
    assert_eq!(edits(&app), before);
    app.undo_grid_edit();
    assert!(!app.has_drafts());
    assert!(!app.can_undo_grid_edit());
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_invalid_pastes_and_fill_leave_all_drafts_untouched() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, n INTEGER, label TEXT)",
            "INSERT INTO t VALUES(1,10,'one'),(2,20,'two')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    app.stage_cell(0, 2, Value::Text("previous".into()));
    select(&mut app, (0, 1), (0, 1));
    let before = edits(&app);
    for text in [
        "11\ngarbage",
        "11\n22\n33",
        "11\tx\textra",
        "11\tx\n22",
        "\"unterminated",
    ] {
        assert!(!app.paste_cells(text), "{text}");
        assert_eq!(edits(&app), before);
    }
    select(&mut app, (0, 1), (1, 2));
    assert!(!app.paste_cells("11\tx"));
    assert_eq!(edits(&app), before);
    assert!(!app.fill_selection(Value::Real(f64::INFINITY)));
    assert_eq!(edits(&app), before);
    app.database.uncertain_edit = true;
    assert!(!app.paste_cells("11\tx\n22\ty"));
    assert!(!app.fill_selection(Value::Null));
    app.undo_grid_edit();
    assert_eq!(edits(&app), before);
    app.database.uncertain_edit = false;
    app.database.document.drafts = None;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_mapped_inserts_defaults_nulls_and_atomic_database_commit() {
    let (_dir, ctx, mut app) = fixture(
        &["CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT, n INTEGER DEFAULT 7, payload BLOB)"],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    assert!(app.can_edit(), "{}", app.database.status);
    let sheet = Sheet::parse("alpha\t00ff\n\\N\t\n\"\\N\"\t0102").unwrap();
    let mapping = [
        Mapping {
            column: Some(1),
            kind: CellKind::Text,
        },
        Mapping {
            column: Some(3),
            kind: CellKind::Blob,
        },
    ];
    let prepared = app.clipboard_inserts(&sheet, &mapping).unwrap();
    assert!(app.stage_spreadsheet(prepared));
    assert_eq!(edits(&app).len(), 3);
    assert!(visible_result(&app).rows.is_empty());
    app.undo_grid_edit();
    assert!(!app.has_drafts());
    let duplicate = [mapping[0], mapping[0]];
    assert!(app.clipboard_inserts(&sheet, &duplicate).is_err());
    let prepared = app.clipboard_inserts(&sheet, &mapping).unwrap();
    assert!(app.stage_spreadsheet(prepared));
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(!app.has_drafts(), "{}", app.database.status);
    assert!(!app.can_undo_grid_edit());
    assert_eq!(visible_result(&app).rows.len(), 3);
    assert_retained(&app, 0, 1, Value::Text("alpha".into()));
    assert_retained(&app, 0, 2, Value::Integer(7));
    assert_retained(&app, 0, 3, Value::Blob(vec![0, 255]));
    assert_retained(&app, 1, 1, Value::Null);
    assert_retained(&app, 1, 3, Value::Text(String::new()));
    assert_retained(&app, 2, 1, Value::Text("\\N".into()));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_copy_paste_roundtrip_preserves_empty_null_blob_and_literal_marker() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a TEXT, b TEXT, c BLOB, d TEXT)",
            "INSERT INTO t VALUES(1,'',NULL,X'00ff','\\N'),(2,'other','other',X'01','other')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    select(&mut app, (0, 1), (0, 4));
    let text = app.selection_tsv().unwrap();
    assert_eq!(text, "\"\"\t\\N\t00ff\t\"\\N\"");
    select(&mut app, (1, 1), (1, 1));
    assert!(app.paste_cells(&text));
    for (c, value) in [
        (1, Value::Text(String::new())),
        (2, Value::Null),
        (3, Value::Blob(vec![0, 255])),
        (4, Value::Text("\\N".into())),
    ] {
        assert_staged(&app, 1, c, value);
    }
    app.database.document.drafts = None;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_native_clipboard_fill_and_undo_are_grid_scoped() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT)",
            "INSERT INTO t VALUES(1,'one'),(2,'two')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let pos = app.first_cell_rect.unwrap().center();
    click(&mut app, &ctx, pos);
    select(&mut app, (0, 1), (0, 1));
    frame(&mut app, &ctx, vec![egui::Event::Paste("native".into())]);
    assert_staged(&app, 0, 1, Value::Text("native".into()));
    select(&mut app, (0, 1), (1, 1));
    press_key(&mut app, &ctx, egui::Key::Backspace, egui::Modifiers::NONE);
    assert_staged(&app, 0, 1, Value::Null);
    assert_staged(&app, 1, 1, Value::Null);
    press_key(&mut app, &ctx, egui::Key::Z, egui::Modifiers::CTRL);
    assert_staged(&app, 0, 1, Value::Text("native".into()));
    assert_eq!(edits(&app).len(), 1);
    // Pasting into an inline text editor must not be intercepted as a rectangle.
    app.start_inline(0, 1);
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![egui::Event::Paste("inline".into())]);
    assert!(
        app.database
            .document
            .tools
            .inline
            .as_ref()
            .unwrap()
            .editor
            .input
            .contains("inline")
    );
    assert_staged(&app, 0, 1, Value::Text("native".into()));
    press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    app.database.document.drafts = None;
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_dialogs_stage_mapped_rows_and_typed_fill_without_writing() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT, n INTEGER DEFAULT 7)",
            "INSERT INTO t VALUES(1,'one',10),(2,'two',20)",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    app.open_paste(true, "three\n\\N".into());
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Preview clipboard").center(),
    );
    let Some(crate::app::spreadsheet::Dialog::Paste(paste)) =
        &mut app.database.document.tools.spreadsheet.dialog
    else {
        panic!("paste dialog")
    };
    paste.mapping[0] = Mapping {
        column: Some(1),
        kind: CellKind::Text,
    };
    // Let the window settle after the preview expands its contents.
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Stage entire paste").center(),
    );
    assert!(
        app.database.document.tools.spreadsheet.dialog.is_none(),
        "{}; document {:?}",
        app.database.status,
        app.database.document.kind
    );
    assert_eq!(edits(&app).len(), 2);
    assert_eq!(
        visible_result(&app).rows.len(),
        2,
        "staging never reruns SQL"
    );
    app.undo_grid_edit();
    assert!(!app.has_drafts());
    select(&mut app, (0, 1), (1, 1));
    app.open_fill();
    let Some(crate::app::spreadsheet::Dialog::Fill(editor)) =
        &mut app.database.document.tools.spreadsheet.dialog
    else {
        panic!("fill dialog")
    };
    editor.kind = CellKind::Text;
    editor.input = "all filled".into();
    frame(&mut app, &ctx, vec![]);
    let output = frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx, control(&output, "Stage fill").center());
    assert_staged(&app, 0, 1, Value::Text("all filled".into()));
    assert_staged(&app, 1, 1, Value::Text("all filled".into()));
    app.revert_grid_draft(0, Some(1));
    app.undo_grid_edit();
    assert_staged(&app, 0, 1, Value::Text("all filled".into()));
    app.undo_grid_edit();
    assert!(!app.has_drafts());
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_guards_batch_limits_result_identity_and_database_rollback() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT UNIQUE)",
            "INSERT INTO t VALUES(1,'one'),(2,'two')",
            "CREATE TABLE keyless(label TEXT)",
            "INSERT INTO keyless VALUES('x')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    select(&mut app, (0, 1), (0, 1));
    assert!(app.paste_cells("same\nsame"));
    let before = edits(&app);
    assert!(!app.stage_spreadsheet(vec![
        tursobench_core::Edit::Insert {
            values: vec![None, Some(Value::Text("x".into()))]
        };
        1001
    ]));
    assert_eq!(edits(&app), before);
    app.commit_drafts();
    wait(&mut app, &ctx);
    assert!(
        app.database.status.contains("No edits committed"),
        "{}",
        app.database.status
    );
    assert_eq!(edits(&app), before);
    app.undo_grid_edit();
    assert!(!app.has_drafts());
    app.reload();
    wait(&mut app, &ctx);
    assert_retained(&app, 0, 1, Value::Text("one".into()));
    assert_retained(&app, 1, 1, Value::Text("two".into()));
    select(&mut app, (0, 1), (0, 1));
    app.fill_selection(Value::Null);
    app.undo_grid_edit();
    app.open_paste(true, "pending".into());
    browse_and_wait(&mut app, &ctx, "keyless");
    assert!(!app.can_undo_grid_edit());
    assert!(app.database.document.tools.spreadsheet.dialog.is_none());
    assert!(!app.paste_cells("blocked"));
    execute_and_wait(&mut app, &ctx, "SELECT * FROM t");
    assert!(!app.paste_cells("blocked"));
    close_and_wait(&mut app, &ctx);
}

#[test]
fn spreadsheet_dialog_keeps_stage_action_visible_with_long_preview_on_small_window() {
    let (_dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT, n INTEGER)",
            "INSERT INTO t VALUES(1,'one',10)",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "t");
    frame(&mut app, &ctx, vec![]);
    app.open_paste(true, "3\tthree\t30\n4\tfour\t40\n5\tfive\t50".into());
    let Some(crate::app::spreadsheet::Dialog::Paste(mut paste)) =
        app.database.document.tools.spreadsheet.dialog.take()
    else {
        panic!("paste dialog")
    };
    app.preview_clipboard(&mut paste);
    app.database.document.tools.spreadsheet.dialog =
        Some(crate::app::spreadsheet::Dialog::Paste(paste));
    let size = egui::vec2(720.0, 480.0);
    frame_at(&mut app, &ctx, size, vec![]);
    frame_at(&mut app, &ctx, size, vec![]);
    let output = frame_at(&mut app, &ctx, size, vec![]);
    let button = control(&output, "Stage entire paste");
    assert!(
        egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(button),
        "{button:?}"
    );
    for pressed in [true, false] {
        frame_at(
            &mut app,
            &ctx,
            size,
            vec![
                egui::Event::PointerMoved(button.center()),
                pointer_button(button.center(), pressed),
            ],
        );
    }
    assert_eq!(edits(&app).len(), 3, "{}", app.database.status);
    app.undo_grid_edit();
    close_and_wait(&mut app, &ctx);
}
