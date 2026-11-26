//! Blob and deferred values: what they are, in the words every view shares,
//! and the lenses that read them in the record tile.
mod hex;
mod inspect;
mod job;
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
}
impl Lens {
    pub const ALL: [Self; 3] = [Self::Hex, Self::Text, Self::Tree];
    pub fn icon(self) -> Icon {
        match self {
            Self::Hex => Icon::Bytecode,
            Self::Text => Icon::Text,
            Self::Tree => Icon::Tree,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Text => "Text",
            Self::Tree => "Tree",
        }
    }
    /// The lenses that suit a kind of value, the automatic one first.
    pub fn offered(kind: Kind) -> &'static [Self] {
        match kind {
            Kind::Text(_) => &[Self::Text, Self::Hex],
            Kind::Jsonb => &[Self::Tree, Self::Hex],
            _ => &[Self::Hex],
        }
    }
    /// The column's chosen lens when it suits `kind`, otherwise the automatic one.
    pub fn resolve(kind: Kind, chosen: Option<Self>) -> Self {
        let offered = Self::offered(kind);
        chosen
            .filter(|lens| offered.contains(lens))
            .unwrap_or(offered[0])
    }
}

/// What a decode has produced for the value it was asked about.
#[derive(Clone)]
pub(super) enum Decoded {
    Pending,
    Tree(Arc<Tree>),
    Failed(String),
}

/// Which value a decode belongs to: the result, its drafts, and the cell.
#[derive(Clone, Copy, PartialEq, Eq)]
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
    decode: Option<(Target, u64, Decoded)>,
    /// The field open in the value view, and its request in flight.
    pub view: Option<view::View>,
    pub ticket: Option<tursobench_core::Ticket>,
}
impl Values {
    /// The decoded tree of the value at `target`: started on first request,
    /// replacing (and cancelling) the decode of any other value.
    pub fn tree(
        &mut self,
        ctx: &eframe::egui::Context,
        target: Target,
        bytes: impl FnOnce() -> job::Work,
    ) -> Decoded {
        match &mut self.decode {
            Some((current, id, decoded)) if *current == target => {
                if let Some(job::Output::Tree(output)) = self.decoder.poll(*id) {
                    *decoded = output.map_or_else(Decoded::Failed, Decoded::Tree);
                }
                decoded.clone()
            }
            _ => {
                let id = self.decoder.submit(ctx, bytes());
                self.decode = Some((target, id, Decoded::Pending));
                Decoded::Pending
            }
        }
    }
    /// Stop decoding: nothing is selected that needs it.
    pub fn idle(&mut self) {
        if self.decode.take().is_some() {
            self.decoder.cancel();
        }
    }
}
