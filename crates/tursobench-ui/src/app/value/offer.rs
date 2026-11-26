//! What a field's value is and which lenses read it: the one place the
//! preview and the value view both ask.
use super::{Lens, Summary, Target, Weak, vector_hint};
use crate::app::App;
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, formats::Format, text::Encoding};
use tursobench_core::Value;

/// A field's value, its kind, the lenses offered and the one in use.
pub(in crate::app) struct Offer {
    pub kind: Kind,
    pub lenses: Vec<Lens>,
    pub lens: Lens,
    /// The structured format the tree lens reads, if any.
    pub format: Option<Format>,
}

impl App {
    /// The value field `col` of `row` shows: its draft, else the stored value.
    pub(in crate::app) fn field_value(&self, row: usize, col: usize) -> Option<&Value> {
        let document = &self.database.document;
        let values = document.result.rows.get(row)?;
        document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col))
            .or_else(|| values.get(col))
    }

    /// The lenses for field `col` of `row`; a blob with no signature is
    /// parsed off the UI thread to learn whether a tree reads it.
    pub(in crate::app) fn value_offer(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
    ) -> Option<Offer> {
        let document = &self.database.document;
        let header = document.column_header(col);
        let hint = vector_hint(&header.column.name, header.declared_type());
        let value = self.field_value(row, col)?;
        let mut kind = match value {
            Value::Text(_) => Kind::Text(Encoding::Utf8),
            other => Summary::of(ctx, other)?.kind,
        };
        // In a vector column, plain float32 outweighs weak signatures.
        let hinted = hint && matches!(value, Value::Blob(b) if super::summary::plain_vector(b));
        if hinted {
            kind = Kind::Unknown;
        }
        let unnamed = matches!(kind, Kind::Unknown | Kind::Digest);
        let bytes = match value {
            Value::Blob(bytes) if unnamed => Some(Arc::<[u8]>::from(bytes.as_slice())),
            _ => None,
        };
        let plain_vector = hinted
            || bytes
                .as_ref()
                .is_some_and(|b| b.len() >= 8 && b.len().is_multiple_of(4));
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: false,
        };
        let chosen = document.tools.columns.lens(col);
        let possible = match bytes {
            Some(bytes) => self
                .database
                .document
                .tools
                .values
                .probe(ctx, target, || bytes),
            None => None,
        };
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
