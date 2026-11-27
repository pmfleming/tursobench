//! Blob and deferred values: what they are, in the words every view shares,
//! and the lenses that read them in the record tile.
mod edit;
mod hex;
mod inspect;
mod io;
mod job;
mod lens;
mod preview;
mod summary;
mod tile;
mod view;

use super::{App, icons::Icon};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, formats::Format, text::Encoding, tree::Tree};
use tursobench_core::Value;

pub(super) use preview::{HEIGHT as PREVIEW_HEIGHT, Preview};
#[cfg(test)]
pub(super) use summary::MADE as SUMMARIES_MADE;
pub(super) use summary::Summary;
#[cfg(test)]
pub(super) use summary::byte_size;
#[cfg(test)]
pub(super) use view::{CHUNK, View, Want};

/// A way of reading a value's bytes. Choosing one never changes the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lens {
    Hex,
    Text,
    Tree,
    Image,
    Vector,
    Contents,
}
impl Lens {
    pub const ALL: [Self; 6] = [
        Self::Hex,
        Self::Text,
        Self::Tree,
        Self::Image,
        Self::Vector,
        Self::Contents,
    ];
    pub fn icon(self) -> Icon {
        match self {
            Self::Hex => Icon::Bytecode,
            Self::Text => Icon::Text,
            Self::Tree => Icon::Tree,
            Self::Image => Icon::Image,
            Self::Vector => Icon::Vector,
            Self::Contents => Icon::Contents,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Text => "Text",
            Self::Tree => "Tree",
            Self::Image => "Image",
            Self::Vector => "Vector",
            Self::Contents => "Contents",
        }
    }
    /// The lenses that suit a value, the automatic one first. Weak evidence
    /// only adds a lens: plain float32 leads only with the column's hint,
    /// and a format found by parsing without a signature never leads.
    pub fn offered(kind: Kind, weak: Weak) -> Vec<Self> {
        let mut lenses = match kind {
            Kind::Text(_) | Kind::Geometry { .. } => vec![Self::Text, Self::Hex],
            Kind::Jsonb | Kind::Structured(_) => vec![Self::Tree, Self::Hex],
            Kind::Image { .. } => vec![Self::Image, Self::Hex],
            Kind::Vector { .. } => vec![Self::Vector, Self::Hex],
            Kind::Compressed(_) => vec![Self::Contents, Self::Hex],
            _ => vec![Self::Hex],
        };
        if weak.plain_vector && !lenses.contains(&Self::Vector) {
            let at = if weak.hint { 0 } else { lenses.len() };
            lenses.insert(at, Self::Vector);
        }
        if weak.possible.is_some() && !lenses.contains(&Self::Tree) {
            lenses.push(Self::Tree);
        }
        lenses
    }
    /// The column's chosen lens when it suits the value, otherwise the automatic one.
    pub fn resolve(offered: &[Self], chosen: Option<Self>) -> Self {
        chosen
            .filter(|lens| offered.contains(lens))
            .unwrap_or(offered[0])
    }
}

/// Weak evidence about a value: lenses it adds, never chooses on its own.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Weak {
    /// The bytes could be plain float32.
    pub plain_vector: bool,
    /// The column's name or type says it holds vectors.
    pub hint: bool,
    /// A format with no signature reads the whole value.
    pub possible: Option<Format>,
}

/// Whether a column's name or declared type says it holds vectors.
pub(super) fn vector_hint(name: &str, declared: Option<&str>) -> bool {
    let name = name.to_ascii_lowercase();
    let declared = declared.unwrap_or_default().to_ascii_uppercase();
    ["embedding", "vector", "vec"]
        .iter()
        .any(|w| name.contains(w))
        || ["VECTOR", "F32_BLOB", "F64_BLOB", "F8_BLOB", "F1BIT_BLOB"]
            .iter()
            .any(|t| declared.contains(t))
}

/// A field's value, its kind, the lenses offered and the one in use.
pub(in crate::app) struct Offer {
    pub kind: Kind,
    pub lenses: Vec<Lens>,
    pub lens: Lens,
    /// The structured format the tree lens reads, if any.
    pub format: Option<Format>,
}

impl App {
    fn value_view(&mut self) -> Option<&mut view::View> {
        self.database.document.tools.values.view.as_mut()
    }

    /// The value field `col` of `row` shows: its draft, else the stored value.
    pub(in crate::app) fn field_value(&self, row: usize, col: usize) -> Option<&Value> {
        let document = &self.database.document;
        document
            .result
            .displayed_value(document.drafts.as_ref(), row, col)
    }

    /// The lenses for field `col` of `row`; a blob with no signature is
    /// parsed off the UI thread to learn whether a tree reads it.
    pub(in crate::app) fn value_offer(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
    ) -> Option<Offer> {
        let document = &mut self.database.document;
        let header = document.column_header(col);
        let hint = vector_hint(&header.column.name, header.declared_type());
        let value = document
            .result
            .displayed_value(document.drafts.as_ref(), row, col)?;
        let mut kind = match value {
            Value::Text(_) => Kind::Text(Encoding::Utf8),
            other => Summary::of(ctx, other)?.kind,
        };
        // In a vector column, plain float32 outweighs weak signatures.
        let hinted = hint && matches!(value, Value::Blob(b) if summary::plain_vector(b));
        if hinted {
            kind = Kind::Unknown;
        }
        let unnamed = matches!(kind, Kind::Unknown | Kind::Digest);
        let bytes = match value {
            Value::Blob(bytes) if unnamed => Some(bytes.as_slice()),
            _ => None,
        };
        let plain_vector =
            hinted || bytes.is_some_and(|b| b.len() >= 8 && b.len().is_multiple_of(4));
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: false,
        };
        let chosen = document.tools.columns.lens(col);
        let possible = bytes.and_then(|bytes| {
            document
                .tools
                .values
                .probe(ctx, target, || Arc::from(bytes))
        });
        let format = match kind {
            Kind::Jsonb => Some(Format::Jsonb),
            Kind::Structured(format) => Some(format),
            _ => possible,
        };
        let lenses = Lens::offered(
            kind,
            Weak {
                plain_vector,
                hint,
                possible,
            },
        );
        let lens = Lens::resolve(&lenses, chosen);
        Some(Offer {
            kind,
            lenses,
            lens,
            format,
        })
    }
}

/// What a decode has produced for the value it was asked about.
#[derive(Clone)]
pub(super) enum Decoded {
    Pending,
    Tree(Arc<Tree>),
    Vector(Arc<tursobench_blob::vector::Vector>),
    Contents(Arc<job::Contents>),
    /// Text a decoder wrote, such as a geometry's WKT.
    Text(Arc<str>),
    Failed(String),
}

/// Which value a decode belongs to: the result, its drafts, and the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub result: Option<tursobench_core::Ticket>,
    pub drafts: u64,
    pub row: usize,
    pub col: usize,
    /// A decode of the whole value, rather than the preview's.
    pub whole: bool,
}

/// A deferred value read whole so the selected field's preview can show it.
pub(super) struct Held {
    pub handle: u64,
    pub state: HeldState,
}
pub(super) enum HeldState {
    Loading,
    Ready(Arc<[u8]>),
    Failed(String),
}

/// A document's value decoding: the selected value's decode and its thread.
#[derive(Default)]
pub(super) struct Values {
    /// The selected field's deferred value, loaded whole on request; at most
    /// one is held, so at most 16 MiB.
    pub held: Option<Held>,
    pub held_ticket: Option<tursobench_core::Ticket>,
    decoder: job::Decoder,
    decode: Option<((Target, Lens), u64, Decoded)>,
    /// Formats found by parsing values without a signature.
    prober: job::Decoder,
    probes: Vec<(Target, u64, Option<Option<Format>>)>,
    /// Images decoded for rows, previews and the value view.
    pub images: lens::Images,
    /// The field open in the value view, and its request in flight.
    pub view: Option<view::View>,
    pub ticket: Option<tursobench_core::Ticket>,
}
impl Values {
    /// The decode of the value at `target` for `lens`: started on first
    /// request, replacing (and cancelling) any other decode.
    pub fn decode(
        &mut self,
        ctx: &eframe::egui::Context,
        target: Target,
        lens: Lens,
        work: impl FnOnce() -> Option<job::Work>,
    ) -> Option<Decoded> {
        Some(match &mut self.decode {
            Some((current, id, decoded)) if *current == (target, lens) => {
                if let Some(output) = self.decoder.poll(*id) {
                    *decoded = match output {
                        job::Output::Tree(Ok(tree)) => Decoded::Tree(tree),
                        job::Output::Vector(Ok(vector)) => Decoded::Vector(vector),
                        job::Output::Contents(Ok(contents)) => Decoded::Contents(contents),
                        job::Output::Text(Ok(text)) => Decoded::Text(text),
                        job::Output::Text(Err(e))
                        | job::Output::Tree(Err(e))
                        | job::Output::Vector(Err(e))
                        | job::Output::Contents(Err(e)) => Decoded::Failed(e),
                        job::Output::Image(_) | job::Output::Probe(_) => {
                            Decoded::Failed("Unexpected result".into())
                        }
                    };
                }
                decoded.clone()
            }
            _ => {
                let Some(id) = self.decoder.submit(ctx, work()?) else {
                    return Some(Decoded::Failed("Could not start the value decoder".into()));
                };
                self.decode = Some(((target, lens), id, Decoded::Pending));
                Decoded::Pending
            }
        })
    }
    /// A format with no signature that reads the whole value at `target`,
    /// found off the UI thread; `None` until known, or when there is none.
    pub fn probe(
        &mut self,
        ctx: &eframe::egui::Context,
        target: Target,
        bytes: impl FnOnce() -> Arc<[u8]>,
    ) -> Option<Format> {
        while let Some((id, output)) = self.prober.next() {
            if let (Some(entry), job::Output::Probe(found)) =
                (self.probes.iter_mut().find(|e| e.1 == id), output)
            {
                entry.2 = Some(found);
            }
        }
        if let Some(entry) = self.probes.iter().find(|e| e.0 == target) {
            return entry.2.flatten();
        }
        if self.probes.len() >= 64 {
            self.probes.remove(0);
        }
        let id = self.prober.queue(ctx, job::Work::Probe(bytes()))?;
        self.probes.push((target, id, None));
        None
    }
    /// Stop decoding: nothing is selected that needs it.
    pub fn idle(&mut self) {
        if self.decode.take().is_some() {
            self.decoder.cancel_queued();
        }
    }
    /// The held value with `handle`, once read.
    pub fn held_bytes(&self, handle: u64) -> Option<Arc<[u8]>> {
        match &self.held {
            Some(Held {
                handle: held,
                state: HeldState::Ready(bytes),
            }) if *held == handle => Some(bytes.clone()),
            _ => None,
        }
    }
    /// How the held value with `handle` stands, if it is the one held.
    pub fn held_state(&self, handle: u64) -> Option<&HeldState> {
        self.held
            .as_ref()
            .filter(|held| held.handle == handle)
            .map(|held| &held.state)
    }
    /// Keep the held value only while it is the selected field's.
    pub fn keep_held(&mut self, handle: Option<u64>) {
        if self.held.as_ref().is_some_and(|h| Some(h.handle) != handle) {
            self.forget_held();
        }
    }
    pub fn forget_held(&mut self) {
        self.held = None;
        self.held_ticket = None;
    }
}

/// The backing handle of a deferred value that can be loaded whole.
pub(super) fn loadable(value: &Value) -> Option<u64> {
    match value {
        Value::Deferred(cell) if cell.bytes <= view::LOAD => cell.handle,
        _ => None,
    }
}

impl App {
    /// Read the whole of field `col` of `row`, a deferred value, so its
    /// preview can show it. Does nothing while it is held or on its way.
    pub(in crate::app) fn load_field(&mut self, row: usize, col: usize) {
        let Some(handle) = self.field_value(row, col).and_then(loadable) else {
            return;
        };
        let values = &mut self.database.document.tools.values;
        if matches!(
            values.held_state(handle),
            Some(HeldState::Loading | HeldState::Ready(_))
        ) {
            return;
        }
        values.held = Some(Held {
            handle,
            state: HeldState::Loading,
        });
        let outcome = self
            .database
            .request(tursobench_db::Operation::LoadValue { handle });
        let values = &mut self.database.document.tools.values;
        match outcome {
            Some(Ok(ticket)) => values.held_ticket = Some(ticket),
            failed => {
                let reason = match failed {
                    Some(Err(e)) => e.to_string(),
                    _ => "The source session is closed".into(),
                };
                if let Some(held) = values.held.as_mut() {
                    held.state = HeldState::Failed(reason);
                }
            }
        }
    }
    /// The answer to a preview's load; an open value view of the same
    /// value takes the bytes too.
    pub(in crate::app) fn held_event(&mut self, kind: tursobench_core::EventKind) {
        use tursobench_core::EventKind;
        let values = &mut self.database.document.tools.values;
        values.held_ticket = None;
        let Some(held) = values.held.as_mut() else {
            return;
        };
        match kind {
            EventKind::ValueLoaded { handle, bytes } if handle == held.handle => {
                let bytes: Arc<[u8]> = Arc::from(bytes);
                held.state = HeldState::Ready(bytes.clone());
                if let Some(view) = values.view.as_mut()
                    && view.source.handle() == Some(handle)
                    && view.loaded.is_none()
                {
                    view.loaded = Some(bytes);
                }
            }
            EventKind::Failed(error) => {
                let message = error.to_string();
                held.state = HeldState::Failed(if message.contains("expired") {
                    "The value is no longer held · refresh to read it again".into()
                } else {
                    message
                });
            }
            _ => {}
        }
    }
}
