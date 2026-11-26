//! The value view: one field filling the record tile. A deferred value is
//! paged from its backing in 64 KiB chunks; the view keeps the latest 16.
use super::Lens;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use tursobench_blob::{Order, encode::Format};
use tursobench_core::{LargeKind, Value};

/// How much of a value one request reads.
pub(in crate::app) const CHUNK: u64 = 64 * 1024;
/// How many chunks the view keeps.
const KEPT: usize = 16;
/// The most a copy may put on the clipboard.
pub(in crate::app) const CLIPBOARD: usize = 2 * 1024 * 1024;
/// The most a lens loads whole.
pub(in crate::app) const LOAD: u64 = 16 * 1024 * 1024;

/// Where the view's bytes come from.
pub(in crate::app) enum Source {
    /// A value held in memory: a blob or text.
    Bytes(Arc<[u8]>),
    /// A value in the session's backing: its handle, if it has one, and size.
    Deferred {
        handle: Option<u64>,
        len: u64,
        head: Arc<[u8]>,
    },
}
impl Source {
    pub fn of(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Blob(bytes) => Self::Bytes(Arc::from(bytes.as_slice())),
            Value::Text(text) => Self::Bytes(Arc::from(text.as_bytes())),
            Value::Deferred(cell) => Self::Deferred {
                handle: cell.handle,
                len: cell.bytes,
                head: Arc::from(cell.head.as_slice()),
            },
            _ => return None,
        })
    }
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::Deferred { len, .. } => *len,
        }
    }
    pub fn handle(&self) -> Option<u64> {
        match self {
            Self::Deferred { handle, .. } => *handle,
            Self::Bytes(_) => None,
        }
    }
}

/// Chunks of a deferred value, the least recently used dropped first.
#[derive(Default)]
pub(in crate::app) struct Chunks {
    map: HashMap<u64, Arc<[u8]>>,
    order: VecDeque<u64>,
}
impl Chunks {
    pub fn insert(&mut self, index: u64, bytes: Arc<[u8]>) {
        self.touch(index);
        self.map.insert(index, bytes);
        while self.order.len() > KEPT {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
    fn touch(&mut self, index: u64) {
        self.order.retain(|&i| i != index);
        self.order.push_back(index);
    }
    pub fn get(&self, index: u64) -> Option<&Arc<[u8]>> {
        self.map.get(&index)
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// What the request in flight asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Want {
    Chunk(u64),
    Load,
    Find,
    Save,
}

/// A chosen file's bytes, or why they could not be read.
pub(in crate::app) type Loaded = Result<Result<Option<Vec<u8>>, String>, String>;

/// A field open in the value view.
pub(in crate::app) struct View {
    pub row: usize,
    pub col: usize,
    /// The result and drafts the source was taken from.
    pub from: (Option<tursobench_core::Ticket>, u64),
    pub source: Source,
    pub text: bool,
    pub cursor: u64,
    pub anchor: u64,
    /// The first line shown.
    pub top: u64,
    /// Bytes per line and lines shown, as last painted.
    pub per: usize,
    pub lines: usize,
    /// Whether the cursor is in the characters column rather than the bytes.
    pub characters: bool,
    pub order: Order,
    pub chunks: Chunks,
    /// The whole value, once loaded for a lens or a copy.
    pub loaded: Option<Arc<[u8]>>,
    pub find: Option<String>,
    /// Where the last search matched, so the next one starts after it.
    pub found: Option<u64>,
    pub goto: Option<String>,
    pub message: String,
    pub expired: bool,
    /// The request in flight, and what the view wants next.
    pub pending: Option<Want>,
    pub want: Option<Want>,
    pub needle: Vec<u8>,
    pub save_path: Option<std::path::PathBuf>,
    /// A copy waiting for its bytes to load: the format, first byte and length.
    pub copy: Option<(Format, u64, u64)>,
    /// Where the next search starts.
    pub find_from: u64,
    pub save: Option<std::sync::mpsc::Receiver<Result<super::super::files::SaveResult, String>>>,
    pub lens: Option<Lens>,
    /// The image lens's scale of its texture, `None` to fit, and its pan.
    pub zoom: Option<f32>,
    pub pan: eframe::egui::Vec2,
    /// The image shown, as last painted: what it is, its texture size and scale.
    pub image: Option<(super::lens::Meta, eframe::egui::Vec2, f32)>,
    /// What the contents lens found, for the footer.
    pub note: String,
    /// Typing inserts bytes rather than replacing them.
    pub insert: bool,
    /// A hex byte has had its first digit typed.
    pub half: bool,
    /// Where the cursor goes once a staged edit is shown.
    pub edit_cursor: Option<u64>,
    /// A file chosen to replace the value, being read.
    pub load: Option<std::sync::mpsc::Receiver<Loaded>>,
}
impl View {
    pub fn new(
        row: usize,
        col: usize,
        value: &Value,
        from: (Option<tursobench_core::Ticket>, u64),
    ) -> Option<Self> {
        let text = matches!(value, Value::Text(_))
            || matches!(value, Value::Deferred(cell) if cell.kind == LargeKind::Text);
        let source = Source::of(value)?;
        let message = match &source {
            Source::Deferred { handle: None, .. } => {
                "Only the first 64 bytes are held: the value is over the 64 MiB limit".into()
            }
            _ => String::new(),
        };
        Some(Self {
            row,
            col,
            from,
            source,
            text,
            cursor: 0,
            anchor: 0,
            top: 0,
            per: 16,
            lines: 1,
            characters: false,
            order: Order::Little,
            chunks: Chunks::default(),
            loaded: None,
            find: None,
            found: None,
            goto: None,
            message,
            expired: false,
            pending: None,
            want: None,
            needle: Vec::new(),
            save_path: None,
            copy: None,
            find_from: 0,
            save: None,
            lens: None,
            zoom: None,
            pan: eframe::egui::Vec2::ZERO,
            image: None,
            note: String::new(),
            insert: false,
            half: false,
            edit_cursor: None,
            load: None,
        })
    }

    /// The byte at `at`, if it is in memory.
    pub fn byte(&self, at: u64) -> Option<u8> {
        if let Some(loaded) = &self.loaded {
            return loaded.get(at as usize).copied();
        }
        match &self.source {
            Source::Bytes(bytes) => bytes.get(at as usize).copied(),
            Source::Deferred { head, .. } if at < head.len() as u64 => Some(head[at as usize]),
            Source::Deferred { .. } => {
                let chunk = self.chunks.get(at / CHUNK)?;
                chunk.get((at % CHUNK) as usize).copied()
            }
        }
    }
    /// `len` bytes from `at`, if all are in memory.
    pub fn bytes(&self, at: u64, len: usize) -> Option<Vec<u8>> {
        let end = at.saturating_add(len as u64).min(self.source.len());
        (at..end).map(|i| self.byte(i)).collect()
    }
    /// The whole value, if it is in memory.
    pub fn whole(&self) -> Option<Arc<[u8]>> {
        match (&self.source, &self.loaded) {
            (Source::Bytes(bytes), _) => Some(bytes.clone()),
            (_, Some(loaded)) => Some(loaded.clone()),
            _ => None,
        }
    }

    /// The selected bytes, first to last.
    pub fn selection(&self) -> std::ops::RangeInclusive<u64> {
        self.anchor.min(self.cursor)..=self.anchor.max(self.cursor)
    }
    pub fn selected(&self) -> u64 {
        if self.source.len() == 0 {
            0
        } else {
            self.anchor.abs_diff(self.cursor) + 1
        }
    }
    fn last(&self) -> u64 {
        self.source.len().saturating_sub(1)
    }
    fn total_lines(&self) -> u64 {
        self.source.len().div_ceil(self.per.max(1) as u64).max(1)
    }
    /// The first line shown, kept so the last lines fill the view.
    pub fn scroll_to_line(&mut self, line: i64) {
        let most = self.total_lines().saturating_sub(self.lines.max(1) as u64);
        self.top = (line.max(0) as u64).min(most);
    }
    /// Move the cursor to `at`, extending the selection with `extend`, and
    /// scroll so the cursor shows.
    pub fn move_to(&mut self, at: u64, extend: bool) {
        self.cursor = at.min(self.last());
        if !extend {
            self.anchor = self.cursor;
        }
        let per = self.per.max(1) as u64;
        let line = self.cursor / per;
        let lines = self.lines.max(1) as u64;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + lines {
            self.top = line + 1 - lines;
        }
    }
    /// Select `len` bytes from `at`.
    pub fn select(&mut self, at: u64, len: u64) {
        self.move_to(at, false);
        self.move_to(at + len.saturating_sub(1), true);
    }
    /// Apply a navigation key; returns whether it was one.
    pub fn key(&mut self, key: eframe::egui::Key, shift: bool, ctrl: bool) -> bool {
        use eframe::egui::Key;
        let per = self.per.max(1) as u64;
        let page = per * self.lines.max(1) as u64;
        let at = self.cursor;
        let to = match key {
            Key::ArrowLeft => at.saturating_sub(1),
            Key::ArrowRight => at + 1,
            Key::ArrowUp => at.saturating_sub(per),
            Key::ArrowDown => at.saturating_add(per).min(self.last()),
            Key::PageUp => at.saturating_sub(page),
            Key::PageDown => at.saturating_add(page).min(self.last()),
            Key::Home if ctrl => 0,
            Key::End if ctrl => self.last(),
            Key::Home => at - at % per,
            Key::End => at - at % per + per - 1,
            Key::Tab => {
                self.characters = !self.characters;
                return true;
            }
            _ => return false,
        };
        self.move_to(to, shift);
        true
    }
    /// The chunk to read next for the lines shown, if one is missing.
    pub fn missing_chunk(&self) -> Option<u64> {
        let Source::Deferred {
            handle: Some(_),
            len,
            ..
        } = self.source
        else {
            return None;
        };
        if self.loaded.is_some() {
            return None;
        }
        let per = self.per.max(1) as u64;
        let start = self.top * per;
        let end = (start + per * self.lines.max(1) as u64).min(len);
        (start / CHUNK..=end.saturating_sub(1) / CHUNK)
            .find(|&i| i * CHUNK < len && self.chunks.get(i).is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::{CHUNK, Chunks, Source, View};
    use eframe::egui::Key;
    use std::sync::Arc;
    use tursobench_core::{DeferredCell, LargeKind, Value};

    fn view(len: usize) -> View {
        let mut view = View::new(
            0,
            0,
            &Value::Blob((0..len).map(|i| i as u8).collect()),
            (None, 0),
        )
        .unwrap();
        view.per = 16;
        view.lines = 4;
        view
    }

    #[test]
    fn chunks_keep_the_sixteen_most_recently_used() {
        let mut chunks = Chunks::default();
        for i in 0..20 {
            chunks.insert(i, Arc::from(&[i as u8][..]));
            // Reading chunk 0 again keeps it.
            chunks.insert(0, Arc::from(&[0u8][..]));
        }
        assert_eq!(chunks.len(), 16);
        assert!(chunks.get(0).is_some());
        assert!(chunks.get(1).is_none());
        assert!(chunks.get(19).is_some());
    }

    #[test]
    fn every_navigation_key_moves_and_extends_within_the_value() {
        let mut v = view(1000);
        let press = |v: &mut View, key, shift, ctrl| {
            assert!(v.key(key, shift, ctrl));
            (v.cursor, v.anchor, v.top)
        };
        assert_eq!(press(&mut v, Key::ArrowRight, false, false), (1, 1, 0));
        assert_eq!(press(&mut v, Key::ArrowLeft, false, false), (0, 0, 0));
        assert_eq!(press(&mut v, Key::ArrowLeft, false, false), (0, 0, 0));
        assert_eq!(press(&mut v, Key::ArrowDown, true, false), (16, 0, 0));
        assert_eq!(v.selected(), 17);
        assert_eq!(press(&mut v, Key::ArrowUp, true, false), (0, 0, 0));
        assert_eq!(press(&mut v, Key::PageDown, false, false), (64, 64, 1));
        assert_eq!(press(&mut v, Key::PageUp, false, false), (0, 0, 0));
        assert_eq!(press(&mut v, Key::End, false, false), (15, 15, 0));
        assert_eq!(press(&mut v, Key::Home, false, false), (0, 0, 0));
        assert_eq!(press(&mut v, Key::End, false, true), (999, 999, 59));
        assert_eq!(press(&mut v, Key::ArrowRight, false, false), (999, 999, 59));
        assert_eq!(press(&mut v, Key::ArrowDown, false, false), (999, 999, 59));
        assert_eq!(press(&mut v, Key::Home, true, true), (0, 999, 0));
        assert!(!v.characters);
        press(&mut v, Key::Tab, false, false);
        assert!(v.characters);
        assert!(!v.key(Key::A, false, false));
        v.select(20, 4);
        assert_eq!((v.selection(), v.selected()), (20..=23, 4));
    }

    #[test]
    fn deferred_values_read_the_head_then_ask_for_missing_chunks() {
        let cell = DeferredCell {
            handle: Some(3),
            kind: LargeKind::Blob,
            bytes: 3 * CHUNK + 5,
            head: vec![9; 64],
        };
        let mut v = View::new(0, 0, &Value::Deferred(Box::new(cell)), (None, 0)).unwrap();
        v.per = 16;
        v.lines = 4;
        assert_eq!(v.byte(63), Some(9));
        assert_eq!(v.byte(64), None);
        assert_eq!(v.missing_chunk(), Some(0));
        v.chunks.insert(0, Arc::from(vec![1u8; CHUNK as usize]));
        assert_eq!((v.byte(64), v.missing_chunk()), (Some(1), None));
        // The last lines straddle the end of chunk 2 and the short chunk 3.
        v.move_to(3 * CHUNK + 4, false);
        assert_eq!(v.missing_chunk(), Some(2));
        v.chunks.insert(2, Arc::from(vec![2u8; CHUNK as usize]));
        assert_eq!(v.missing_chunk(), Some(3));
        assert_eq!(v.bytes(3 * CHUNK - 2, 4), None);
        assert!(matches!(v.source, Source::Deferred { len, .. } if len == 3 * CHUNK + 5));
    }
}
