//! Search, copy and save value bytes in memory or from the database session.
use super::view::{CHUNK, CLIPBOARD, LOAD, Source, View, Want};
use crate::app::{App, files};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::encode::Format as CopyFormat;
use tursobench_core::EventKind;
use tursobench_db::Operation;

impl App {
    /// Search for the typed bytes or text after the cursor: in memory when
    /// the whole value is held, otherwise in the backing.
    pub(in crate::app) fn find_value(&mut self) {
        let Some(view) = self.value_view() else {
            return;
        };
        let query = view.find.as_deref().unwrap_or_default();
        let needle = needle(query, view.characters);
        if needle.is_empty() || needle.len() > 1024 {
            view.message = "Find 1–1,024 bytes".into();
            return;
        }
        // Searching again from a match finds the next one.
        let start = *view.selection().start();
        view.find_from = match view.found {
            Some(at) if at == start => at + 1,
            _ => view.cursor,
        };
        view.needle = needle;
        match view.whole() {
            Some(bytes) => {
                let start = (view.find_from as usize).min(bytes.len());
                let needle = view.needle.as_slice();
                let hit = bytes[start..]
                    .windows(needle.len())
                    .position(|w| w == needle);
                found(view, hit.map(|i| (start + i) as u64));
            }
            None => view.want = Some(Want::Find),
        }
    }

    /// Copy the selection, or with `selection` false and one byte selected the
    /// whole value, as `format`; a deferred value is loaded first.
    pub(in crate::app) fn copy_value(
        &mut self,
        ctx: &egui::Context,
        format: CopyFormat,
        selection: bool,
    ) {
        let Some(view) = self.value_view() else {
            return;
        };
        let (at, len) = if selection || view.selected() > 1 {
            (*view.selection().start(), view.selected())
        } else {
            (0, view.source.len())
        };
        self.copy_range(ctx, format, at, len);
    }
    /// Copy `len` bytes from `at` as `format`, loading a deferred value first.
    fn copy_range(&mut self, ctx: &egui::Context, format: CopyFormat, at: u64, len: u64) {
        let Some(view) = self.value_view() else {
            return;
        };
        if format.length(len as usize) > CLIPBOARD {
            view.message = "Over the 2 MiB clipboard budget · save to file instead".into();
            return;
        }
        if let Some(bytes) = view.bytes(at, len as usize) {
            ctx.copy_text(format.write(&bytes));
            view.message = format!("Copied {len} bytes as {}", format.name());
            return;
        }
        if view.source.handle().is_some() && view.source.len() <= LOAD {
            view.copy = Some((format, at, len));
            view.want = Some(Want::Load);
        } else {
            view.message = "The value is not held · save to file instead".into();
        }
    }

    /// Ask where to save the value; bytes in memory are written on the
    /// dialog's thread, a deferred value by the session.
    pub(super) fn save_value(&mut self, ctx: &egui::Context, extension: &str) {
        let ctx = ctx.clone();
        let Some(view) = self.database.document.tools.values.view.as_ref() else {
            return;
        };
        let column = self
            .database
            .document
            .result
            .columns
            .get(view.col)
            .map_or("value", |c| c.name.as_str());
        let name = format!("{column}.{extension}");
        let bytes = match &view.source {
            Source::Bytes(bytes) => Some(bytes.clone()),
            Source::Deferred {
                handle: Some(_), ..
            } => None,
            Source::Deferred { handle: None, .. } => {
                return self.value_message("The value is too large to keep, so it cannot be saved");
            }
        };
        let task = files::start_task(ctx, "value-save-dialog", move || {
            files::save_value_dialog(&name, bytes)
        });
        match task {
            Ok(rx) => {
                if let Some(view) = self.value_view() {
                    view.save = Some(rx);
                }
            }
            Err(e) => self.value_message(&format!("Could not open the save dialog: {e}")),
        }
    }
    fn value_message(&mut self, message: &str) {
        if let Some(view) = self.value_view() {
            view.message = message.into();
        }
    }
    pub(super) fn poll_value_save(&mut self) {
        let Some(view) = self.value_view() else {
            return;
        };
        let Some(rx) = &view.save else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result.and_then(|saved| saved),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("The save dialog stopped".into())
            }
        };
        view.save = None;
        match result {
            Ok(Some(path)) => self.value_saved_to(path),
            Ok(None) => {}
            Err(e) => view.message = e,
        }
    }
    /// The dialog chose `path`: a value in memory is already written there;
    /// the session writes a deferred one.
    pub(in crate::app) fn value_saved_to(&mut self, path: std::path::PathBuf) {
        let Some(view) = self.value_view() else {
            return;
        };
        if view.source.handle().is_some() {
            view.save_path = Some(path);
            view.want = Some(Want::Save);
        } else {
            view.message = format!("Saved to {}", path.display());
        }
    }

    /// Send the request the view needs next, one at a time.
    pub(in crate::app) fn drive_value(&mut self) {
        let values = &mut self.database.document.tools.values;
        if values.ticket.is_some() {
            return;
        }
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let Some(handle) = view.source.handle().filter(|_| !view.expired) else {
            return;
        };
        let Some(want) = view
            .want
            .take()
            .or_else(|| view.missing_chunk().map(Want::Chunk))
        else {
            return;
        };
        let operation = match want {
            Want::Chunk(index) => Operation::ReadValue {
                handle,
                offset: index * CHUNK,
                bytes: CHUNK as usize,
            },
            Want::Load => Operation::LoadValue { handle },
            Want::Find => Operation::FindValue {
                handle,
                needle: view.needle.clone(),
                from: view.find_from,
            },
            Want::Save => match view.save_path.take() {
                Some(path) => Operation::SaveValue { handle, path },
                None => return,
            },
        };
        view.pending = Some(want);
        let outcome = self.database.request(operation);
        let message = match outcome {
            Some(Ok(ticket)) => {
                self.database.document.tools.values.ticket = Some(ticket);
                return;
            }
            Some(Err(e)) => e.to_string(),
            None => "The source session is closed".into(),
        };
        if let Some(view) = self.value_view() {
            view.pending = None;
            view.failed(Some(want), message);
        }
    }

    /// The answer to the view's request.
    pub(in crate::app) fn value_event(&mut self, ctx: &egui::Context, kind: EventKind) {
        let values = &mut self.database.document.tools.values;
        values.ticket = None;
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let asked = view.pending.take();
        let ours = view.source.handle();
        let mut copy = None;
        match kind {
            EventKind::ValueChunk {
                handle,
                offset,
                bytes,
                ..
            } if ours == Some(handle) => {
                view.chunks.insert(offset / CHUNK, Arc::from(bytes));
            }
            EventKind::ValueLoaded { handle, bytes } if ours == Some(handle) => {
                view.loaded = Some(Arc::from(bytes));
                copy = view.copy.take();
            }
            EventKind::ValueFound { handle, offset } if ours == Some(handle) => found(view, offset),
            EventKind::ValueSaved(path) => view.message = format!("Saved to {path}"),
            EventKind::Failed(error) => {
                view.failed(asked, error.to_string());
            }
            _ => {}
        }
        if let Some((format, at, len)) = copy {
            self.copy_range(ctx, format, at, len);
        }
    }
}

/// What to search for: text from the characters column or in quotes,
/// otherwise bytes when the query is hex pairs, else text.
fn needle(query: &str, characters: bool) -> Vec<u8> {
    let quoted = query.len() >= 2 && query.starts_with('"') && query.ends_with('"');
    if quoted {
        return query.as_bytes()[1..query.len() - 1].to_vec();
    }
    if characters {
        return query.as_bytes().to_vec();
    }
    tursobench_blob::encode::parse_hex(query).unwrap_or_else(|| query.as_bytes().to_vec())
}

/// Select a match, or say there is none after the cursor.
fn found(view: &mut View, offset: Option<u64>) {
    match offset {
        Some(at) => {
            view.found = Some(at);
            view.select(at, view.needle.len() as u64);
            view.message.clear();
        }
        None => view.message = "No match after the cursor".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::needle;

    #[test]
    fn find_reads_hex_pairs_as_bytes_unless_asked_for_text() {
        assert_eq!(needle("00 ff", false), [0x00, 0xff]);
        assert_eq!(needle("123456", false), [0x12, 0x34, 0x56]);
        // From the characters column, or in quotes, the same digits are text.
        assert_eq!(needle("123456", true), *b"123456");
        assert_eq!(needle("\"cafe\"", false), *b"cafe");
        assert_eq!(needle("seam", false), *b"seam");
        assert_eq!(needle("+f", false), *b"+f");
    }
}
