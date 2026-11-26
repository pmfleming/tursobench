//! Blob and deferred values: what they are, in the words every view shares,
//! and the lenses that read them in the record tile.
mod edit;
mod hex;
mod inspect;
mod job;
mod lens;
mod offer;
mod preview;
mod summary;
mod tile;
mod view;

use super::icons::Icon;
use std::sync::Arc;
use tursobench_blob::{Kind, tree::Tree};

pub(super) use preview::{HEIGHT as PREVIEW_HEIGHT, Preview};
#[cfg(test)]
pub(super) use summary::MADE as SUMMARIES_MADE;
pub(super) use summary::Summary;
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
    pub possible: Option<tursobench_blob::formats::Format>,
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

/// A document's value decoding: the selected value's decode and its thread.
#[derive(Default)]
pub(super) struct Values {
    decoder: job::Decoder,
    decode: Option<((Target, Lens), u64, Decoded)>,
    /// Formats found by parsing values without a signature.
    prober: job::Decoder,
    probes: Vec<(
        Target,
        u64,
        Option<Option<tursobench_blob::formats::Format>>,
    )>,
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
        work: impl FnOnce() -> job::Work,
    ) -> Decoded {
        match &mut self.decode {
            Some((current, id, decoded)) if *current == (target, lens) => {
                if let Some(output) = self.decoder.poll(*id) {
                    *decoded = match output {
                        job::Output::Tree(Ok(tree)) => Decoded::Tree(tree),
                        job::Output::Vector(Ok(vector)) => Decoded::Vector(vector),
                        job::Output::Contents(Ok(contents)) => Decoded::Contents(contents),
                        job::Output::Text(Ok(text)) => Decoded::Text(text),
                        job::Output::Text(Err(e)) => Decoded::Failed(e),
                        job::Output::Tree(Err(e))
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
                let id = self.decoder.submit(ctx, work());
                self.decode = Some(((target, lens), id, Decoded::Pending));
                Decoded::Pending
            }
        }
    }
    /// A format with no signature that reads the whole value at `target`,
    /// found off the UI thread; `None` until known, or when there is none.
    pub fn probe(
        &mut self,
        ctx: &eframe::egui::Context,
        target: Target,
        bytes: impl FnOnce() -> Arc<[u8]>,
    ) -> Option<tursobench_blob::formats::Format> {
        for (id, output) in self.prober.poll_all() {
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
        let id = self.prober.queue(ctx, job::Work::Probe(bytes()));
        self.probes.push((target, id, None));
        None
    }
    /// Stop decoding: nothing is selected that needs it.
    pub fn idle(&mut self) {
        if self.decode.take().is_some() {
            self.decoder.cancel();
        }
    }
}
