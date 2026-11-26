//! Editing a blob in the value view: typed hex or characters, over or
//! between the bytes, a replacement from a file, or NULL. Every edit is
//! staged through `stage_cell` and a blob stays a blob.
use super::view::Source;
use crate::app::{App, files};
use eframe::egui;
use tursobench_core::{EDIT_BYTES, Value};

/// What the value view's editing can change, and why not when it cannot.
pub(in crate::app) fn refusal(
    editable: bool,
    value: Option<&Value>,
    source: &Source,
) -> Option<&'static str> {
    if !editable {
        return Some("Editing needs writable access to a browsed table with a key");
    }
    match (value, source) {
        (Some(Value::Blob(_)), Source::Bytes(_)) => None,
        (Some(Value::Deferred(_)), _) => Some("A value too large to hold cannot be staged"),
        _ => Some("Only blobs are edited here; text is edited in the field"),
    }
}

/// `bytes` with `typed` hexadecimal digits or characters applied at the
/// cursor: over the bytes there, or between them in `insert` mode. Returns
/// the new bytes, cursor and whether a hex byte is half typed.
pub(in crate::app) fn type_into(
    bytes: &[u8],
    cursor: usize,
    half: bool,
    typed: &str,
    characters: bool,
    insert: bool,
) -> (Vec<u8>, usize, bool) {
    let mut out = bytes.to_vec();
    let (mut at, mut half) = (cursor.min(out.len()), half);
    if characters {
        for byte in typed.bytes() {
            if insert || at >= out.len() {
                out.insert(at, byte);
            } else {
                out[at] = byte;
            }
            at += 1;
        }
        return (out, at, false);
    }
    for digit in typed.chars().filter_map(|c| c.to_digit(16)) {
        let digit = digit as u8;
        if half {
            out[at] = out[at] & 0xf0 | digit;
            at += 1;
            half = false;
        } else {
            if insert || at >= out.len() {
                out.insert(at, 0);
            }
            out[at] = digit << 4 | out[at] & 0x0f;
            half = true;
        }
    }
    (out, at, half)
}

impl App {
    /// Why the open value cannot be edited, if it cannot.
    pub(in crate::app) fn value_refusal(&self) -> Option<&'static str> {
        let view = self.database.document.tools.values.view.as_ref()?;
        let value = self.field_value(view.row, view.col);
        refusal(self.can_edit(), value, &view.source)
    }

    /// Stage `bytes` as the open field's new blob, refused past the edit limit.
    pub(in crate::app) fn stage_value_bytes(&mut self, bytes: Vec<u8>) {
        let Some(view) = self.database.document.tools.values.view.as_mut() else {
            return;
        };
        let value = Value::Blob(bytes);
        if value.bytes() > EDIT_BYTES {
            let size = super::summary::byte_size(value.bytes() as u64);
            view.message =
                format!("{size} is over the 64 KiB a staged blob may hold · nothing was staged");
            return;
        }
        let (row, col) = (view.row, view.col);
        view.message.clear();
        self.stage_cell(row, col, value);
    }

    /// Stage NULL for the open field, closing the value.
    pub(in crate::app) fn stage_value_null(&mut self) {
        let Some(view) = self.database.document.tools.values.view.as_ref() else {
            return;
        };
        let (row, col) = (view.row, view.col);
        self.stage_cell(row, col, Value::Null);
        self.close_value();
    }

    /// Apply typed hex digits or characters, and Delete and Backspace,
    /// when the value can be edited; returns whether keys were used.
    pub(in crate::app) fn value_edit_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.value_refusal().is_some() {
            return false;
        }
        let typed: String = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        });
        let insert_key = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Insert));
        let delete = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        let Some(view) = self.database.document.tools.values.view.as_mut() else {
            return false;
        };
        if insert_key {
            view.insert = !view.insert;
        }
        let Some(bytes) = view.whole() else {
            return false;
        };
        if delete {
            self.delete_value_selection();
            return true;
        }
        let next = if !typed.is_empty() {
            Some(type_into(
                &bytes,
                view.cursor as usize,
                view.half,
                &typed,
                view.characters,
                view.insert,
            ))
        } else {
            None
        };
        let Some((out, cursor, half)) = next else {
            return insert_key;
        };
        view.half = half;
        view.edit_cursor = Some(cursor as u64);
        self.stage_value_bytes(out);
        true
    }

    /// Stage the open blob without its selected bytes.
    pub(in crate::app) fn delete_value_selection(&mut self) {
        let Some(view) = self.database.document.tools.values.view.as_mut() else {
            return;
        };
        let Some(bytes) = view.whole().filter(|b| !b.is_empty()) else {
            return;
        };
        let range = view.selection();
        let (start, end) = (
            *range.start() as usize,
            (*range.end() as usize).min(bytes.len() - 1),
        );
        let mut out = bytes.to_vec();
        out.drain(start..=end);
        view.edit_cursor = Some(start as u64);
        self.stage_value_bytes(out);
    }

    /// Choose a file whose bytes replace the open value, read on the
    /// dialog's thread and refused past the edit limit.
    pub(in crate::app) fn replace_value_from_file(&mut self, ctx: &egui::Context) {
        let task = files::start_task(ctx.clone(), "value-load-dialog", || {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Replace value from file")
                .pick_file()
            else {
                return Ok(None);
            };
            files::read_at_most(&path, EDIT_BYTES).map(Some)
        });
        let Some(view) = self.database.document.tools.values.view.as_mut() else {
            return;
        };
        match task {
            Ok(rx) => view.load = Some(rx),
            Err(e) => view.message = format!("Could not open the file dialog: {e}"),
        }
    }

    /// Stage the bytes of a chosen file, once read.
    pub(in crate::app) fn poll_value_load(&mut self) {
        let Some(view) = self.database.document.tools.values.view.as_mut() else {
            return;
        };
        let Some(rx) = &view.load else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result.and_then(|read| read),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("The file dialog stopped".into())
            }
        };
        view.load = None;
        match result {
            Ok(Some(bytes)) => self.stage_value_bytes(bytes),
            Ok(None) => {}
            Err(e) => view.message = e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{refusal, type_into};
    use crate::app::value::view::Source;
    use std::sync::Arc;
    use tursobench_core::Value;

    #[test]
    fn typing_overwrites_or_inserts_whole_bytes_a_digit_at_a_time() {
        let bytes = [0x00, 0xff, 0x7a];
        assert_eq!(
            type_into(&bytes, 1, false, "a", false, false),
            (vec![0x00, 0xaf, 0x7a], 1, true)
        );
        assert_eq!(
            type_into(&bytes, 1, true, "b", false, false),
            (vec![0x00, 0xfb, 0x7a], 2, false)
        );
        assert_eq!(
            type_into(&bytes, 1, false, "12", false, true),
            (vec![0x00, 0x12, 0xff, 0x7a], 2, false)
        );
        assert_eq!(
            type_into(&bytes, 3, false, "4", false, false),
            (vec![0x00, 0xff, 0x7a, 0x40], 3, true)
        );
        assert_eq!(
            type_into(&bytes, 0, false, "xyz!", false, false).0,
            bytes.to_vec()
        );
        assert_eq!(
            type_into(&bytes, 2, false, "hi", true, false),
            (vec![0x00, 0xff, b'h', b'i'], 4, false)
        );
        assert_eq!(
            type_into(&bytes, 0, false, "A", true, true),
            (vec![b'A', 0x00, 0xff, 0x7a], 1, false)
        );
    }

    #[test]
    fn only_blobs_held_in_memory_are_edited() {
        let bytes = Source::Bytes(Arc::from(&[1u8][..]));
        let blob = Value::Blob(vec![1]);
        assert_eq!(refusal(true, Some(&blob), &bytes), None);
        assert!(refusal(false, Some(&blob), &bytes).is_some());
        assert!(refusal(true, Some(&Value::Text("a".into())), &bytes).is_some());
    }
}
