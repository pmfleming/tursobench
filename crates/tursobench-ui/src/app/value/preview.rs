//! The selected field's preview: 72 points under its line, through a lens.
//! Hex and text use bytes already in memory; a tree comes from the decoder.
//! A large value the lens needs whole is loaded on request.
use super::{
    Decoded, HeldState, Lens, Summary, Target,
    job::Contents,
    job::Work,
    lens::{self, Shown, Size},
    loadable,
};
use crate::app::{
    App,
    icons::{self, Icon},
    presentation,
};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, formats::Format, text, tree};
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
    /// A large value the lens needs whole, not yet in memory.
    Load(Load),
}

/// A large value's place in being loaded for its preview.
pub(in crate::app) enum Load {
    /// It can be loaded: its size, as shown on the button.
    Offer(String),
    Loading,
    /// It cannot be, and why.
    Unavailable(String),
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
        let value = document
            .result
            .displayed_value(document.drafts.as_ref(), row, col)?;
        if matches!(value, Value::Text(_)) {
            return None;
        }
        let values = &mut document.tools.values;
        // A deferred value its preview loaded, read whole.
        let held = loadable(value).and_then(|handle| values.held_bytes(handle));
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: held.is_some(),
        };
        let kind = offer.kind;
        let whole = in_memory(value).or(held);
        let body = match (offer.lens, whole) {
            (Lens::Hex, _) => {
                let head = bytes(value);
                Body::Hex(head[..head.len().min(32 * LINES)].to_vec())
            }
            (Lens::Text, _) if !matches!(kind, Kind::Geometry { .. }) => {
                Body::Text(text_of(value, kind))
            }
            (Lens::Image, Some(bytes)) => Body::Image(values.images.get(
                ctx,
                target,
                Size::Preview,
                || match kind {
                    Kind::Image { format, .. } => Some((bytes, format)),
                    _ => None,
                },
            )),
            (lens, Some(bytes)) => Body::Decoded(
                values
                    .decode(ctx, target, lens, || {
                        whole_work(lens, kind, offer.format, bytes)
                    })
                    .unwrap_or_else(|| Decoded::Failed("no reader for this value".into())),
            ),
            // The first bytes of a deferred JSONB value read as a tree.
            (Lens::Tree, None) if let Some(work) = jsonb_prefix(value, offer.format) => {
                Body::Decoded(
                    values
                        .decode(ctx, target, Lens::Tree, || Some(work))
                        .unwrap_or(Decoded::Pending),
                )
            }
            (_, None) => Body::Load(load_state(value, values)),
        };
        Some(Preview {
            lens: offer.lens,
            offered: offer.lenses,
            body,
        })
    }
}

/// The decode of a whole value for `lens`.
fn whole_work(lens: Lens, kind: Kind, format: Option<Format>, bytes: Arc<[u8]>) -> Option<Work> {
    match lens {
        Lens::Text => Some(Work::Wkt(bytes)),
        Lens::Tree => format.map(|format| Work::Tree(bytes, format)),
        Lens::Vector => Some(Work::Vector(bytes)),
        Lens::Contents => match kind {
            Kind::Compressed(compression) => Some(Work::Contents(bytes, compression)),
            _ => None,
        },
        Lens::Hex | Lens::Image => None,
    }
}

/// Whether a deferred value can be loaded for its preview, is loading, or cannot be.
fn load_state(value: &Value, values: &super::Values) -> Load {
    let Some(handle) = loadable(value) else {
        return Load::Unavailable("Over the 16 MiB a lens reads · use hex or save to file".into());
    };
    match values.held_state(handle) {
        Some(HeldState::Loading | HeldState::Ready(_)) => Load::Loading,
        Some(HeldState::Failed(reason)) => Load::Unavailable(reason.clone()),
        None => Load::Offer(super::summary::byte_size(match value {
            Value::Deferred(cell) => cell.bytes,
            _ => 0,
        })),
    }
}

/// A whole value in memory.
fn in_memory(value: &Value) -> Option<Arc<[u8]>> {
    match value {
        Value::Blob(bytes) => Some(Arc::from(bytes.as_slice())),
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
        let value = document
            .result
            .displayed_value(document.drafts.as_ref(), row, col)?;
        let values = &mut document.tools.values;
        let whole = in_memory(value).or_else(|| values.held_bytes(loadable(value)?))?;
        let Kind::Image { format, .. } = Summary::of(ctx, value)?.kind else {
            return None;
        };
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: false,
        };
        match values
            .images
            .get(ctx, target, Size::Thumb, || Some((whole, format)))
        {
            Shown::Ready(id, size, _) => Some((id, size)),
            _ => None,
        }
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
    let head = bytes(value);
    let head = &head[..head.len().min(4096)];
    match (value, kind) {
        (Value::Deferred(cell), _) if cell.kind == LargeKind::Text => {
            String::from_utf8_lossy(head).into_owned()
        }
        (_, Kind::Text(encoding)) => text::decode(head, encoding),
        _ => String::from_utf8_lossy(head).into_owned(),
    }
}

/// The tree decode of a deferred JSONB value's first bytes.
fn jsonb_prefix(value: &Value, format: Option<Format>) -> Option<Work> {
    match (value, format?) {
        (Value::Deferred(cell), Format::Jsonb) => Some(Work::JsonbPrefix(
            Arc::from(cell.head.as_slice()),
            cell.bytes,
        )),
        _ => None,
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
            Body::Decoded(Decoded::Text(text)) => wrapped(ui, &painter, rect, text, color),
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
            Body::Image(Shown::Pending) | Body::Load(Load::Loading) => {
                let square = egui::Rect::from_min_size(rect.min, egui::Vec2::splat(rect.height()));
                painter.rect_filled(square, 3.0, tokens.raised);
            }
            Body::Image(Shown::Failed(reason)) => wrapped(ui, &painter, rect, reason, tokens.faint),
            // Drawn as controls.
            Body::Load(Load::Offer(_) | Load::Unavailable(_)) => {}
        }
    }
    /// The load button, or why the value cannot be loaded, at the left of
    /// `rect`; returns whether the button was clicked.
    pub fn controls(&self, ui: &mut egui::Ui, rect: egui::Rect, col: usize) -> bool {
        let Body::Load(load) = &self.body else {
            return false;
        };
        let lens = self.lens.name().to_lowercase();
        let slot = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - 14.0),
            egui::vec2(rect.width(), 28.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("preview-load", col))
                .max_rect(slot)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        match load {
            Load::Offer(size) => {
                let name = format!("Load {size} to show the {lens}");
                icons::labelled(&mut child, self.lens.icon(), size, &name).clicked()
            }
            Load::Unavailable(reason) => {
                icons::hint(&mut child, Icon::Warning, 20.0, reason);
                false
            }
            Load::Loading => false,
        }
    }
    /// Whether the preview shows the work of the decode thread.
    pub fn decodes(&self) -> bool {
        matches!(self.body, Body::Decoded(_))
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
            Body::Decoded(Decoded::Text(text)) => text.chars().take(200).collect(),
            Body::Decoded(Decoded::Pending) | Body::Image(Shown::Pending) => "Reading".into(),
            Body::Decoded(Decoded::Failed(reason)) | Body::Image(Shown::Failed(reason)) => {
                reason.clone()
            }
            Body::Image(Shown::Ready(_, _, meta)) => {
                format!("Image, {} × {}, {}", meta.width, meta.height, meta.color)
            }
            Body::Load(Load::Offer(size)) => format!("{size}, not loaded"),
            Body::Load(Load::Loading) => "Loading".into(),
            Body::Load(Load::Unavailable(reason)) => reason.clone(),
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
    let colors = Colors {
        text: color,
        faint,
        pending: color,
    };
    hex_line_marked(ui, offset, digits, chunk, per, colors, &[])
}

/// The colours of a hex line: bytes, offsets and zeros, and staged changes.
#[derive(Clone, Copy)]
pub(in crate::app) struct Colors {
    pub text: egui::Color32,
    pub faint: egui::Color32,
    pub pending: egui::Color32,
}

/// One line of the hex lens, with each byte `changed` from the stored value
/// in the pending colour, in both columns.
pub(in crate::app) fn hex_line_marked(
    ui: &egui::Ui,
    offset: u64,
    digits: usize,
    chunk: &[u8],
    per: usize,
    colors: Colors,
    changed: &[bool],
) -> egui::text::LayoutJob {
    let Colors {
        text: color,
        faint,
        pending,
    } = colors;
    let staged = |i: usize| changed.get(i).copied().unwrap_or(false);
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
            Some(_) if staged(i) => add(&format!("{:02x}", chunk[i]), pending),
            Some(0) => add("00", faint),
            Some(b) => add(&format!("{b:02x}"), color),
            None => add("  ", color),
        }
    }
    add("  ", color);
    for (i, &b) in chunk.iter().enumerate() {
        let shown = if b.is_ascii_graphic() || b == b' ' {
            char::from(b).to_string()
        } else {
            ".".into()
        };
        let tone = match (staged(i), shown.as_str()) {
            (true, _) => pending,
            (false, ".") => faint,
            _ => color,
        };
        add(&shown, tone);
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
