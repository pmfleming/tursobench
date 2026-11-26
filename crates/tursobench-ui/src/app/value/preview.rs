//! The selected field's preview: 72 points under its line, through a lens.
//! Hex and text use bytes already in memory; a tree comes from the decoder.
use super::{
    Decoded, Lens, Summary, Target,
    job::Contents,
    job::Work,
    lens::{self, Shown, Size},
};
use crate::app::{App, presentation};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, formats::Format, sniff::ImageFormat, text, tree};
use tursobench_core::{LargeKind, Value};

/// How much a selected field grows to show its preview.
pub(in crate::app) const HEIGHT: f32 = 72.0;
/// Rows of hex, text or tree in a preview.
const LINES: usize = 4;

/// What a preview shows.
pub(in crate::app) enum Body {
    /// The first bytes of the value.
    Hex(Vec<u8>),
    Text(String),
    /// A tree, a vector or inflated contents, decoded off the UI thread.
    Decoded(Decoded),
    Image(Shown),
}

/// A selected field's lens, the lenses it could use, and what it shows.
pub(in crate::app) struct Preview {
    pub lens: Lens,
    pub offered: Vec<Lens>,
    pub body: Body,
}

impl App {
    /// The preview of the selected field `col` of `row`, when it holds a blob
    /// or deferred value; a lens that decodes starts or polls its decode.
    pub(in crate::app) fn record_preview(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
    ) -> Option<Preview> {
        let offer = self.value_offer(ctx, row, col)?;
        let document = &mut self.database.document;
        let values = document.result.rows.get(row)?;
        let value = document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col))
            .unwrap_or(values.get(col)?);
        if matches!(value, Value::Text(_)) {
            return None;
        }
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: false,
        };
        let kind = offer.kind;
        let mut decode = |work: Option<Work>| match work {
            Some(work) => document
                .tools
                .values
                .decode(ctx, target, offer.lens, || work),
            None => Decoded::Failed("Open the value to read it".into()),
        };
        let body = match offer.lens {
            Lens::Hex => Body::Hex(bytes(value).iter().copied().take(32 * LINES).collect()),
            Lens::Text => Body::Text(text_of(value, kind)),
            Lens::Tree => Body::Decoded(decode(tree_work(value, offer.format))),
            Lens::Vector => Body::Decoded(decode(in_memory(value).map(Work::Vector))),
            Lens::Contents => Body::Decoded(decode(contents_work(value, kind))),
            Lens::Image => {
                let bytes = || image_bytes(value, kind);
                Body::Image(
                    document
                        .tools
                        .values
                        .images
                        .get(ctx, target, Size::Preview, bytes),
                )
            }
        };
        Some(Preview {
            lens: offer.lens,
            offered: offer.lenses,
            body,
        })
    }
}

/// A whole value in memory.
fn in_memory(value: &Value) -> Option<Arc<[u8]>> {
    match value {
        Value::Blob(bytes) => Some(Arc::from(bytes.as_slice())),
        _ => None,
    }
}
fn contents_work(value: &Value, kind: Kind) -> Option<Work> {
    match kind {
        Kind::Compressed(compression) => in_memory(value).map(|b| Work::Contents(b, compression)),
        _ => None,
    }
}

impl App {
    /// The thumbnail of field `col` of `row` when it holds an image in
    /// memory, once decoded off the UI thread.
    pub(in crate::app) fn record_thumbnail(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        let document = &mut self.database.document;
        let values = document.result.rows.get(row)?;
        let value = document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col))
            .unwrap_or(values.get(col)?);
        let Value::Blob(_) = value else {
            return None;
        };
        let kind = Summary::of(ctx, value)?.kind;
        let Kind::Image { .. } = kind else {
            return None;
        };
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: false,
        };
        let bytes = || image_bytes(value, kind);
        match document
            .tools
            .values
            .images
            .get(ctx, target, Size::Thumb, bytes)
        {
            Shown::Ready(id, size, _) => Some((id, size)),
            _ => None,
        }
    }
}

/// An image's bytes and format, when the whole value is in memory.
pub(in crate::app) fn image_bytes(value: &Value, kind: Kind) -> Option<(Arc<[u8]>, ImageFormat)> {
    match (value, kind) {
        (Value::Blob(bytes), Kind::Image { format, .. }) => {
            Some((Arc::from(bytes.as_slice()), format))
        }
        _ => None,
    }
}

/// The bytes of a value in memory: all of a blob, the head of a deferred value.
fn bytes(value: &Value) -> &[u8] {
    match value {
        Value::Blob(bytes) => bytes,
        Value::Deferred(cell) => &cell.head,
        Value::Text(text) => text.as_bytes(),
        _ => &[],
    }
}

/// Text from the first 4 KiB of a value, in the encoding it was detected in.
fn text_of(value: &Value, kind: Kind) -> String {
    let head = &bytes(value)[..bytes(value).len().min(4096)];
    match (value, kind) {
        (Value::Deferred(cell), _) if cell.kind == LargeKind::Text => {
            String::from_utf8_lossy(head).into_owned()
        }
        (_, Kind::Text(encoding)) => text::decode(head, encoding),
        _ => String::from_utf8_lossy(head).into_owned(),
    }
}

/// The tree decode for a value: whole when in memory; for a deferred
/// JSONB value, its first bytes.
fn tree_work(value: &Value, format: Option<Format>) -> Option<Work> {
    match (value, format?) {
        (Value::Deferred(cell), Format::Jsonb) => Some(Work::JsonbPrefix(
            Arc::from(cell.head.as_slice()),
            cell.bytes,
        )),
        (Value::Deferred(_), _) => None,
        (other, format) => Some(Work::Tree(Arc::from(bytes(other)), format)),
    }
}

impl Preview {
    /// Paint the preview into `rect`; staged values are in the pending colour.
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, staged: bool) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let color = if staged {
            tokens.pending
        } else {
            ui.visuals().text_color()
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        match &self.body {
            Body::Hex(bytes) => hex(ui, &painter, rect, bytes, color),
            Body::Text(text) => wrapped(ui, &painter, rect, text, color),
            Body::Decoded(Decoded::Tree(tree)) => lines(ui, &painter, rect, tree, color),
            Body::Decoded(Decoded::Vector(vector)) => vector_preview(ui, &painter, rect, vector),
            Body::Decoded(Decoded::Contents(contents)) => {
                contents_preview(ui, &painter, rect, contents, color);
            }
            Body::Decoded(Decoded::Pending) => {
                let row = rect.height() / LINES as f32;
                for i in 0..LINES - 1 {
                    let line = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, row * i as f32),
                        egui::vec2(rect.width(), row),
                    );
                    presentation::placeholder_row(&painter, line, i, tokens.raised);
                }
            }
            Body::Decoded(Decoded::Failed(reason)) => {
                let text = format!("Not readable as {}: {reason}", self.lens.name());
                wrapped(ui, &painter, rect, &text, tokens.faint);
            }
            Body::Image(Shown::Ready(id, size, _)) => {
                // Up to the preview's height, at most four times the image's own size.
                let scale = lens::fit(rect, *size).min(4.0);
                let area = egui::Rect::from_min_size(rect.min, *size * scale);
                lens::paint_image(ui, area, *id, *size, scale, egui::Vec2::ZERO);
            }
            Body::Image(Shown::Pending) => {
                let square = egui::Rect::from_min_size(rect.min, egui::Vec2::splat(rect.height()));
                painter.rect_filled(square, 3.0, tokens.raised);
            }
            Body::Image(Shown::Failed(reason)) => wrapped(ui, &painter, rect, reason, tokens.faint),
        }
    }
    /// What the preview shows, for accessibility.
    pub fn describe(&self) -> String {
        match &self.body {
            Body::Hex(bytes) => format!("{} preview of {} bytes", self.lens.name(), bytes.len()),
            Body::Text(text) => text.chars().take(200).collect(),
            Body::Decoded(Decoded::Tree(tree)) => tree.json().chars().take(200).collect(),
            Body::Decoded(Decoded::Vector(vector)) => {
                let (norm, min, max) = vector.stats();
                format!(
                    "{} × {}, norm {norm:.3}, from {min:.3} to {max:.3}",
                    vector.layout.name(),
                    vector.dims
                )
            }
            Body::Decoded(Decoded::Contents(contents)) => {
                format!(
                    "{} contents, {} bytes",
                    contents.compression.name(),
                    contents.len
                )
            }
            Body::Decoded(Decoded::Pending) | Body::Image(Shown::Pending) => "Reading".into(),
            Body::Decoded(Decoded::Failed(reason)) | Body::Image(Shown::Failed(reason)) => {
                reason.clone()
            }
            Body::Image(Shown::Ready(_, _, meta)) => {
                format!("Image, {} × {}, {}", meta.width, meta.height, meta.color)
            }
        }
    }
}

pub(in crate::app) fn font(ui: &egui::Ui) -> egui::FontId {
    crate::appearance::monospace(ui.ctx(), 11.5)
}

/// How many bytes fit on a hex line `width` wide: 32, 16 or 8.
pub(in crate::app) fn bytes_per_line(ui: &egui::Ui, width: f32) -> usize {
    let glyph = ui
        .painter()
        .layout_no_wrap("0".into(), font(ui), egui::Color32::WHITE)
        .size()
        .x;
    // An offset, then each byte as two digits and a space, a gap every 8
    // bytes, and each byte again as a character.
    let chars = |n: usize| 4 + 2 + n * 3 + n / 8 + 1 + n;
    [32, 16]
        .into_iter()
        .find(|&n| chars(n) as f32 * glyph <= width)
        .unwrap_or(8)
}

/// Four lines of offset, bytes and characters; zero bytes are dimmed.
fn hex(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    bytes: &[u8],
    color: egui::Color32,
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let per = bytes_per_line(ui, rect.width());
    let row = rect.height() / LINES as f32;
    for (line, chunk) in bytes.chunks(per).take(LINES).enumerate() {
        let job = hex_line(ui, (line * per) as u64, 4, chunk, per, color, tokens.faint);
        let galley = painter.layout_job(job);
        let at = rect.min + egui::vec2(0.0, row * line as f32 + (row - galley.size().y) / 2.0);
        painter.galley(at, galley, color);
    }
}

/// One line of the hex lens: offset, bytes and characters.
pub(in crate::app) fn hex_line(
    ui: &egui::Ui,
    offset: u64,
    digits: usize,
    chunk: &[u8],
    per: usize,
    color: egui::Color32,
    faint: egui::Color32,
) -> egui::text::LayoutJob {
    let font = font(ui);
    let mut job = egui::text::LayoutJob::default();
    let mut add =
        |text: &str, color| job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
    add(&format!("{offset:0digits$x}  "), faint);
    for i in 0..per {
        let gap = if i > 0 && i % 8 == 0 {
            "  "
        } else if i > 0 {
            " "
        } else {
            ""
        };
        add(gap, color);
        match chunk.get(i) {
            Some(0) => add("00", faint),
            Some(b) => add(&format!("{b:02x}"), color),
            None => add("  ", color),
        }
    }
    add("  ", color);
    for &b in chunk {
        if b.is_ascii_graphic() || b == b' ' {
            add(&char::from(b).to_string(), color);
        } else {
            add(".", faint);
        }
    }
    job
}

/// Text wrapped to the preview's lines.
fn wrapped(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    color: egui::Color32,
) {
    let mut job = egui::text::LayoutJob::simple(
        text.chars().take(4096).collect(),
        crate::appearance::proportional(ui.ctx(), 12.5),
        color,
        rect.width().max(16.0),
    );
    job.wrap.max_rows = LINES;
    job.wrap.break_anywhere = true;
    painter.galley(rect.min, painter.layout_job(job), color);
}

/// The first lines of a tree, printed as JSON in the SQL editor's colours.
fn lines(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    tree: &tree::Tree,
    color: egui::Color32,
) {
    let key = ui.visuals().hyperlink_color;
    let value = crate::appearance::Tokens::of(ui.ctx()).relationship;
    let row = rect.height() / LINES as f32;
    for (i, line) in tree.pretty(LINES).iter().enumerate() {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &"  ".repeat(line.depth),
            0.0,
            egui::TextFormat::simple(font(ui), color),
        );
        for (text, part) in &line.parts {
            let color = match part {
                tree::Part::Key => key,
                tree::Part::Text | tree::Part::Number => value,
                tree::Part::Literal => key,
                tree::Part::Punctuation => color,
            };
            job.append(text, 0.0, egui::TextFormat::simple(font(ui), color));
        }
        job.wrap.max_width = rect.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = painter.layout_job(job);
        let at = rect.min + egui::vec2(0.0, row * i as f32 + (row - galley.size().y) / 2.0);
        painter.galley(at, galley, color);
    }
}

/// A vector's values as bars about a centre line, then its first values.
pub(in crate::app) fn bars(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    values: &[f64],
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let peak = values
        .iter()
        .fold(0f64, |m, v| m.max(v.abs()))
        .max(f64::MIN_POSITIVE);
    let middle = rect.center().y;
    painter.hline(
        rect.x_range(),
        middle,
        egui::Stroke::new(1.0, tokens.line_soft),
    );
    let step = rect.width() / values.len().max(1) as f32;
    let stroke = egui::Stroke::new((step * 0.7).max(1.0), tokens.primary);
    for (i, value) in values.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * step;
        let height = (value / peak) as f32 * rect.height() / 2.0;
        painter.line_segment(
            [egui::pos2(x, middle), egui::pos2(x, middle - height)],
            stroke,
        );
    }
}

fn vector_preview(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    vector: &tursobench_blob::vector::Vector,
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let chart = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), rect.height() - 22.0));
    let count = (rect.width() / 3.0) as usize;
    bars(ui, painter, chart, &vector.buckets(count));
    let numbers = vector.numbers();
    let shown: Vec<String> = numbers.iter().take(12).map(|v| format!("{v:.4}")).collect();
    let more = if numbers.len() > 12 { " …" } else { "" };
    let line = format!("[0]  {}{more}", shown.join("  "));
    let mut job = egui::text::LayoutJob::simple_singleline(line, font(ui), tokens.faint);
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    painter.galley(
        egui::pos2(rect.left(), rect.bottom() - 16.0),
        painter.layout_job(job),
        tokens.faint,
    );
}

/// The first lines of inflated contents: their tree, their text, or their bytes.
fn contents_preview(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    contents: &Contents,
    color: egui::Color32,
) {
    if let Some(tree) = &contents.tree {
        return lines(ui, painter, rect, tree, color);
    }
    if let Some(text) = &contents.text {
        return wrapped(ui, painter, rect, text, color);
    }
    hex(
        ui,
        painter,
        rect,
        &contents.bytes[..contents.bytes.len().min(32 * LINES)],
        color,
    );
}
