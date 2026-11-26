//! The record tile's value view, through the real worker.
use super::{
    App, browse_and_wait, connected, draw, egui, execute_and_wait, frame, key_event, wait_until,
};
use crate::app::value::{CHUNK, View};
use tursobench_blob::encode::Format;
use tursobench_core::{EventKind, Failure, Value};

/// A table whose `data` is over the 64 KiB a page keeps in memory, with
/// "seam" written across its first chunk boundary, beside a short blob.
fn deferred() -> (tempfile::TempDir, egui::Context, App, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("values.db");
    let mut big = vec![0u8; 3 * CHUNK as usize];
    big[CHUNK as usize - 2..CHUNK as usize + 2].copy_from_slice(b"seam");
    big[0..4].copy_from_slice(&[0x00, 0x66, 0x6a, 0x73]);
    let bytes = big.clone();
    crate::test_support::run(async move {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await?;
        let c = db.connect()?;
        c.execute(
            "CREATE TABLE v(id INTEGER PRIMARY KEY, data BLOB, small BLOB)",
            (),
        )
        .await?;
        let params = turso::params![big, vec![0u8, 255, 122]];
        c.execute("INSERT INTO v VALUES(1, ?1, ?2)", params).await?;
        Ok::<_, turso::Error>(())
    })
    .unwrap();
    let (ctx, mut app) = connected(&dir.path().join("values.db"), true);
    browse_and_wait(&mut app, &ctx, "v");
    (dir, ctx, app, bytes)
}

fn view(app: &App) -> &View {
    app.database
        .document
        .tools
        .values
        .view
        .as_ref()
        .expect("value open")
}
fn view_mut(app: &mut App) -> &mut View {
    app.database
        .document
        .tools
        .values
        .view
        .as_mut()
        .expect("value open")
}
/// Draw and poll until `done`, as the app would between frames.
fn settle_value(app: &mut App, ctx: &egui::Context, done: impl Fn(&App) -> bool) {
    for _ in 0..500 {
        draw(app, ctx);
        if done(app) {
            return;
        }
        wait_until(app, ctx, |a| {
            a.database.document.tools.values.ticket.is_none()
        });
    }
    panic!("the value view never settled: {}", view(app).message);
}
/// Take keyboard focus from the sidebar search, as clicking the tile would.
fn unfocus(app: &mut App, ctx: &egui::Context) {
    app.sidebar_search.focus = false;
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}
fn copied(output: &egui::FullOutput) -> Vec<String> {
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_deferred_value_pages_finds_copies_and_saves_by_handle() {
    let (dir, ctx, mut app, bytes) = deferred();
    unfocus(&mut app, &ctx);
    assert!(matches!(
        &app.database.document.result.rows[0][1],
        Value::Deferred(_)
    ));
    app.open_value(0, 1);
    // Only the head is in memory; the first chunk is read for the lines shown.
    settle_value(&mut app, &ctx, |a| view(a).byte(100).is_some());
    let output = draw(&mut app, &ctx);
    let lines: Vec<_> = super::texts(&output)
        .map(|t| t.galley.job.text.clone())
        .collect();
    assert!(
        lines.iter().any(|t| t.starts_with("000000  00 66 6a 73")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|t| t == "1 936 352 768"), "{lines:?}");
    // A search finds bytes split across the boundary between two chunks.
    view_mut(&mut app).find = Some("seam".into());
    app.find_value();
    settle_value(&mut app, &ctx, |a| view(a).found.is_some());
    assert_eq!(view(&app).selection(), CHUNK - 2..=CHUNK + 1);
    settle_value(&mut app, &ctx, |a| view(a).bytes(CHUNK - 2, 4).is_some());
    // Ctrl C copies the selection as shown; the characters column copies text.
    // (With the find input open, Ctrl C copies from it instead.)
    view_mut(&mut app).find = None;
    unfocus(&mut app, &ctx);
    let output = frame(&mut app, &ctx, vec![egui::Event::Copy]);
    assert_eq!(copied(&output), ["73 65 61 6d"]);
    view_mut(&mut app).characters = true;
    let output = frame(&mut app, &ctx, vec![egui::Event::Copy]);
    assert_eq!(copied(&output), ["seam"]);
    // Copying the whole value as Base64 loads it first.
    view_mut(&mut app).move_to(0, false);
    view_mut(&mut app).message.clear();
    app.copy_value(&ctx, Format::Base64, false);
    settle_value(&mut app, &ctx, |a| view(a).message.starts_with("Copied"));
    assert_eq!(
        view(&app).message,
        format!("Copied {} bytes as Base64", bytes.len())
    );
    // Saving writes the exact bytes through the session.
    let path = dir.path().join("saved.bin");
    app.value_saved_to(path.clone());
    settle_value(&mut app, &ctx, |a| view(a).message.starts_with("Saved"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // An expired backing is reported, and nothing more is asked of it.
    let expired = Failure::input("Value backing expired or belongs to another session");
    app.value_event(&ctx, EventKind::Failed(expired));
    assert!(view(&app).expired);
    let sequence = app.database.sequence;
    view_mut(&mut app).want = Some(crate::app::value::Want::Load);
    app.drive_value();
    assert_eq!(app.database.sequence, sequence);
    app.close();
}

#[test]
fn values_in_memory_open_with_enter_and_close_with_escape() {
    let (_dir, ctx, mut app, _) = deferred();
    app.open_record_window(0, 2);
    draw(&mut app, &ctx);
    unfocus(&mut app, &ctx);
    let sequence = app.database.sequence;
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!((view(&app).row, view(&app).col), (0, 2));
    let output = draw(&mut app, &ctx);
    let lines: Vec<_> = super::texts(&output)
        .map(|t| t.galley.job.text.clone())
        .collect();
    assert!(
        lines.iter().any(|t| t.starts_with("0000  00 ff 7a")),
        "{lines:?}"
    );
    assert_eq!(
        app.database.sequence, sequence,
        "bytes in memory need no request"
    );
    // Ctrl F opens find; Esc closes it, then the value.
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::F, true, egui::Modifiers::COMMAND)],
    );
    assert!(view(&app).find.is_some());
    view_mut(&mut app).find = Some("ff 7a".into());
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!(view(&app).selection(), 1..=2);
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Escape, true, egui::Modifiers::NONE)],
    );
    assert!(view(&app).find.is_none());
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Escape, true, egui::Modifiers::NONE)],
    );
    assert!(!app.value_open());
    // Ctrl G goes to an offset typed in the footer.
    app.open_value(0, 2);
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::G, true, egui::Modifiers::COMMAND)],
    );
    view_mut(&mut app).goto = Some("0x2".into());
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!(view(&app).cursor, 2);
    // Arrows move the cursor while the value is open.
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::ArrowLeft, true, egui::Modifiers::NONE)],
    );
    assert_eq!(view(&app).cursor, 1);
    app.close();
}

#[test]
fn a_value_without_backing_or_session_says_so() {
    let (ctx, mut app) = crate::app::test_ui::app();
    let cell = |handle| {
        Value::Deferred(Box::new(tursobench_core::DeferredCell {
            handle,
            kind: tursobench_core::LargeKind::Text,
            bytes: 40_000_000,
            head: b"head".to_vec(),
        }))
    };
    let document = &mut app.database.document;
    document.result.columns = vec![tursobench_core::Column {
        name: "body".into(),
        declared_type: None,
    }];
    document.result.rows = vec![vec![cell(None)], vec![cell(Some(7))]];
    app.open_value(0, 0);
    assert!(view(&app).message.contains("64 MiB"));
    app.open_value(1, 0);
    app.drive_value();
    assert_eq!(view(&app).message, "The source session is closed");
    // Chunks for another value are ignored.
    let chunk = EventKind::ValueChunk {
        handle: 8,
        offset: 0,
        bytes: vec![1; 10],
        total: 40_000_000,
    };
    app.value_event(&ctx, chunk);
    assert_eq!(view(&app).byte(4), None);
    app.value_event(&ctx, EventKind::ValueSaved("/tmp/value".into()));
    assert_eq!(view(&app).message, "Saved to /tmp/value");
}

#[test]
fn vectors_trees_without_a_signature_and_compressed_contents_find_their_lenses() {
    use crate::app::value::Lens;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lenses.db");
    let floats: Vec<u8> = (0..16)
        .flat_map(|i| (i as f32 / 10.0).to_le_bytes())
        .collect();
    let mut gzip = vec![0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0xff];
    // A stored (uncompressed) deflate block holding "hello", then its CRC and size.
    gzip.extend([0x01, 0x05, 0x00, 0xfa, 0xff]);
    gzip.extend(b"hello");
    gzip.extend(0x3610_a686_u32.to_le_bytes());
    gzip.extend(5u32.to_le_bytes());
    // MessagePack {"a":1}: no signature, so a tree is only offered.
    let msgpack = vec![0x81, 0xa1, b'a', 0x01];
    let blobs = (floats.clone(), floats, gzip, msgpack);
    crate::test_support::run(async move {
        let db = turso::Builder::new_local(path.to_str().unwrap()).build().await?;
        let c = db.connect()?;
        c.execute("CREATE TABLE l(id INTEGER PRIMARY KEY, embedding BLOB, raw BLOB, payload BLOB, session BLOB)", ())
            .await?;
        let params = turso::params![blobs.0, blobs.1, blobs.2, blobs.3];
        c.execute("INSERT INTO l VALUES(1, ?1, ?2, ?3, ?4)", params).await?;
        Ok::<_, turso::Error>(())
    })
    .unwrap();
    let (ctx, mut app) = connected(&dir.path().join("lenses.db"), true);
    browse_and_wait(&mut app, &ctx, "l");
    let offer = |app: &mut App, col| {
        let mut found = None;
        for _ in 0..500 {
            let offer = app.value_offer(&ctx, 0, col).unwrap();
            found = Some((offer.lens, offer.lenses.clone()));
            if col != 4 || offer.lenses.contains(&Lens::Tree) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        found.unwrap()
    };
    // The same float32 bytes lead with the vector lens only where the column says so.
    assert_eq!(
        offer(&mut app, 1),
        (Lens::Vector, vec![Lens::Vector, Lens::Hex])
    );
    assert_eq!(
        offer(&mut app, 2),
        (Lens::Hex, vec![Lens::Hex, Lens::Vector])
    );
    assert_eq!(
        offer(&mut app, 3),
        (Lens::Contents, vec![Lens::Contents, Lens::Hex])
    );
    assert_eq!(offer(&mut app, 4), (Lens::Hex, vec![Lens::Hex, Lens::Tree]));
    // The contents lens inflates and reads the text inside.
    app.open_value(0, 3);
    let mut shown = Vec::new();
    for _ in 0..500 {
        let output = draw(&mut app, &ctx);
        shown = super::texts(&output)
            .map(|t| t.galley.job.text.clone())
            .collect();
        if shown.iter().any(|t| t == "hello") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(shown.iter().any(|t| t == "hello"), "{shown:?}");
    app.close();
}

#[test]
fn hex_edits_stage_a_blob_that_stays_a_blob_and_write_only_on_commit() {
    // Rows holding a deferred value cannot be edited: commits compare whole originals.
    let (_dir, ctx, mut app) = super::fixture(
        &[
            "CREATE TABLE v(id INTEGER PRIMARY KEY, n INTEGER, small BLOB)",
            "INSERT INTO v VALUES(1, 7, X'00FF7A')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "v");
    unfocus(&mut app, &ctx);
    app.open_value(0, 2);
    draw(&mut app, &ctx);
    let sequence = app.database.sequence;
    // Two hex digits overwrite the byte under the cursor; nothing is written.
    frame(&mut app, &ctx, vec![egui::Event::Text("ab".into())]);
    let staged = |app: &App| app.field_value(0, 2).cloned();
    assert_eq!(staged(&app), Some(Value::Blob(vec![0xab, 0xff, 0x7a])));
    assert_eq!(app.database.sequence, sequence, "staging writes nothing");
    draw(&mut app, &ctx);
    assert_eq!(
        view(&app).cursor,
        1,
        "the cursor moves on past the typed byte"
    );
    // Insert mode puts typed characters between the bytes.
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Insert, true, egui::Modifiers::NONE)],
    );
    view_mut(&mut app).characters = true;
    frame(&mut app, &ctx, vec![egui::Event::Text("Z".into())]);
    assert_eq!(
        staged(&app),
        Some(Value::Blob(vec![0xab, b'Z', 0xff, 0x7a]))
    );
    // Changed bytes are drawn in the pending colour.
    let output = draw(&mut app, &ctx);
    let pending = crate::appearance::Tokens::of(&ctx).pending;
    let marked = super::texts(&output).any(|t| {
        t.galley.job.text.starts_with("0000")
            && t.galley
                .job
                .sections
                .iter()
                .any(|s| s.format.color == pending)
    });
    assert!(marked);
    // A replacement past the edit limit is refused with the reason.
    app.stage_value_bytes(vec![0; 70_000]);
    assert!(
        view(&app).message.contains("over the 64 KiB"),
        "{}",
        view(&app).message
    );
    assert_eq!(
        staged(&app),
        Some(Value::Blob(vec![0xab, b'Z', 0xff, 0x7a]))
    );
    // Reverting restores the stored bytes.
    app.revert_grid_draft(0, Some(2));
    draw(&mut app, &ctx);
    assert_eq!(staged(&app), Some(Value::Blob(vec![0x00, 0xff, 0x7a])));
    // Deleting the selection, then committing, stores a blob.
    view_mut(&mut app).select(1, 2);
    app.delete_value_selection();
    assert_eq!(staged(&app), Some(Value::Blob(vec![0x00])));
    app.commit_drafts();
    super::wait(&mut app, &ctx);
    super::execute_and_wait(&mut app, &ctx, "SELECT typeof(small), hex(small) FROM v");
    let rows = &app.database.document.result.rows;
    assert_eq!(
        rows[0],
        [Value::Text("blob".into()), Value::Text("00".into())]
    );
    app.close();
}

#[test]
fn a_file_replaces_a_value_only_within_the_edit_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (small, large) = (dir.path().join("small.bin"), dir.path().join("large.bin"));
    std::fs::write(&small, [1, 2, 3]).unwrap();
    std::fs::write(&large, vec![0; 70_000]).unwrap();
    let limit = tursobench_core::EDIT_BYTES;
    assert_eq!(
        crate::app::files::read_at_most(&small, limit).unwrap(),
        [1, 2, 3]
    );
    let refused = crate::app::files::read_at_most(&large, limit).unwrap_err();
    assert!(refused.contains("over the 64 KiB"), "{refused}");
}

#[test]
fn edit_keys_act_only_in_the_hex_lens_and_keep_its_place() {
    use crate::app::value::Lens;
    let wide = format!("X'{}'", "ab".repeat(400));
    let (_dir, ctx, mut app) = super::fixture(
        &[
            "CREATE TABLE v(id INTEGER PRIMARY KEY, small BLOB, note BLOB, empty BLOB, wide BLOB)",
            &format!(
                "INSERT INTO v VALUES(1, X'00FF7A', CAST('plain text stored as a blob' AS BLOB), X'', {wide})"
            ),
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "v");
    unfocus(&mut app, &ctx);
    let staged = |app: &App, col| app.field_value(0, col).cloned();
    // Typing or Delete in the text lens stages nothing.
    assert_eq!(app.value_offer(&ctx, 0, 2).unwrap().lens, Lens::Text);
    app.open_value(0, 2);
    draw(&mut app, &ctx);
    frame(&mut app, &ctx, vec![egui::Event::Text("a".into())]);
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Delete, true, egui::Modifiers::NONE)],
    );
    assert!(
        !app.database.document.has_drafts(),
        "the text lens is read only"
    );
    app.close_value();
    // A half-typed byte deleted, then another digit: a new byte, no panic.
    app.open_value(0, 3);
    draw(&mut app, &ctx);
    frame(&mut app, &ctx, vec![egui::Event::Text("a".into())]);
    assert_eq!(staged(&app, 3), Some(Value::Blob(vec![0xa0])));
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Delete, true, egui::Modifiers::NONE)],
    );
    assert_eq!(staged(&app, 3), Some(Value::Blob(vec![])));
    frame(&mut app, &ctx, vec![egui::Event::Text("b".into())]);
    assert_eq!(staged(&app, 3), Some(Value::Blob(vec![0xb0])));
    app.close_value();
    // A staged keystroke keeps the lines shown: the cursor's line stays put.
    app.open_value(0, 4);
    draw(&mut app, &ctx);
    let (per, lines) = (view(&app).per as u64, view(&app).lines as u64);
    assert!(lines > 1, "the test needs more than one line shown");
    view_mut(&mut app).move_to(per * (lines - 1), false);
    frame(&mut app, &ctx, vec![egui::Event::Text("1".into())]);
    assert!(app.database.document.has_drafts());
    assert_eq!(
        (view(&app).top, view(&app).cursor),
        (0, per * (lines - 1)),
        "the edited line stays where it was"
    );
    assert!(view(&app).half);
    // A blob staged as NULL is edited as NULL: Enter opens the field's editor.
    app.revert_grid_draft(0, None);
    app.close_value();
    app.open_value(0, 1);
    app.stage_value_null();
    assert!(!app.value_open());
    assert_eq!(staged(&app, 1), Some(Value::Null));
    app.start_inline(0, 1);
    assert!(app.database.document.tools.inline.is_some());
    app.close();
}

#[test]
fn a_value_from_query_results_opens_in_the_detached_inspector() {
    let (_dir, ctx, mut app) = super::fixture(&["CREATE TABLE t(x)"], false);
    execute_and_wait(&mut app, &ctx, "SELECT X'00FF7A' AS b");
    let kind = app.database.document.kind;
    assert_ne!(kind, crate::app::tiling::Kind::Data, "a query result");
    // As clicking the result's grid would, focus the result.
    app.database.workspace.focused = app.database.document.id;
    unfocus(&mut app, &ctx);
    app.open_value(0, 0);
    let ui = &app.database.document.ui;
    assert!(app.value_in_dialog() && ui.inspector_open && ui.inspector_detached);
    // A window's first frame only measures it.
    draw(&mut app, &ctx);
    let output = draw(&mut app, &ctx);
    let lines: Vec<_> = super::texts(&output)
        .map(|t| t.galley.job.text.clone())
        .collect();
    assert!(
        lines.iter().any(|t| t.starts_with("0000  00 ff 7a")),
        "{lines:?}"
    );
    // Escape closes the value, leaving the inspector open.
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Escape, true, egui::Modifiers::NONE)],
    );
    assert!(!app.value_open() && app.database.document.ui.inspector_open);
    app.close();
}

#[test]
fn typing_appends_keeps_the_hex_lens_and_stages_only_real_changes() {
    use crate::app::value::Lens;
    let (_dir, ctx, mut app) = super::fixture(
        &[
            "CREATE TABLE v(id INTEGER PRIMARY KEY, a BLOB, b BLOB, c BLOB)",
            "INSERT INTO v VALUES(1, X'', X'', X'00FF')",
        ],
        true,
    );
    browse_and_wait(&mut app, &ctx, "v");
    unfocus(&mut app, &ctx);
    let staged = |app: &App, col| app.field_value(0, col).cloned();
    let type_keys = |app: &mut App, keys: &str| {
        for key in keys.chars() {
            frame(app, &ctx, vec![egui::Event::Text(key.to_string())]);
        }
    };
    // One key a frame, as typed: each byte is added after the last. After
    // "4" the value is "@", which reads as text, but the lens stays hex.
    app.open_value(0, 1);
    draw(&mut app, &ctx);
    type_keys(&mut app, "4");
    draw(&mut app, &ctx);
    assert_eq!(app.value_lens_now(&ctx), Some(Lens::Hex));
    type_keys(&mut app, "1cafe");
    assert_eq!(staged(&app, 1), Some(Value::Blob(vec![0x41, 0xca, 0xfe])));
    draw(&mut app, &ctx);
    assert_eq!(view(&app).cursor, 3, "the cursor waits past the last byte");
    app.close_value();
    // Insert mode adds bytes in the order typed.
    app.open_value(0, 2);
    draw(&mut app, &ctx);
    frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::Insert, true, egui::Modifiers::NONE)],
    );
    type_keys(&mut app, "a0b1c2");
    assert_eq!(staged(&app, 2), Some(Value::Blob(vec![0xa0, 0xb1, 0xc2])));
    app.revert_grid_draft(0, None);
    app.close_value();
    // Keys that are not hex digits stage nothing, and typing the stored
    // bytes back leaves no draft.
    app.open_value(0, 3);
    draw(&mut app, &ctx);
    type_keys(&mut app, "g ");
    assert!(!app.database.document.has_drafts(), "no byte changed");
    type_keys(&mut app, "11");
    assert_eq!(staged(&app, 3), Some(Value::Blob(vec![0x11, 0xff])));
    draw(&mut app, &ctx);
    view_mut(&mut app).move_to(0, false);
    type_keys(&mut app, "00");
    assert!(
        !app.database.document.has_drafts(),
        "the stored bytes again revert the draft"
    );
    app.close();
}
