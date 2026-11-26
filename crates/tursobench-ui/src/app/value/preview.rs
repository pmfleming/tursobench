//! The selected field's preview: 72 points under its line, through a lens.
//! Hex and text use bytes already in memory; a tree comes from the decoder.
use super::{Decoded, Lens, Summary, Target, job::Work};
use crate::app::{App, presentation};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, text, tree};
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
    Tree(Decoded),
}

/// A selected field's lens, the lenses it could use, and what it shows.
pub(in crate::app) struct Preview {
    pub lens: Lens,
    pub offered: &'static [Lens],
    pub body: Body,
}

impl App {
    /// The preview of the selected field `col` of `row`, when it holds a blob
    /// or deferred value; reading a tree starts or polls its decode.
    pub(in crate::app) fn record_preview(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        col: usize,
    ) -> Option<Preview> {
        let document = &mut self.database.document;
        let values = document.result.rows.get(row)?;
        let value = document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col))
            .unwrap_or(values.get(col)?);
        let kind = Summary::of(ctx, value)?.kind;
        let lens = Lens::resolve(kind, document.tools.columns.lens(col));
        let body = match lens {
            Lens::Hex => Body::Hex(bytes(value).iter().copied().take(32 * LINES).collect()),
            Lens::Text => Body::Text(text_of(value, kind)),
            Lens::Tree => {
                let target = Target {
                    result: document.result.current,
                    drafts: document.draft_revision,
                    row,
                    col,
                    whole: false,
                };
                let work = || tree_work(value);
                Body::Tree(document.tools.values.tree(ctx, target, work))
            }
        };
        Some(Preview {
            lens,
            offered: Lens::offered(kind),
            body,
        })
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

fn tree_work(value: &Value) -> Work {
    match value {
        Value::Deferred(cell) => Work::JsonbPrefix(Arc::from(cell.head.as_slice()), cell.bytes),
        other => Work::Jsonb(Arc::from(bytes(other))),
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
            Body::Tree(Decoded::Tree(tree)) => lines(ui, &painter, rect, tree, color),
            Body::Tree(Decoded::Pending) => {
                let row = rect.height() / LINES as f32;
                for i in 0..LINES - 1 {
                    let line = egui::Rect::from_min_size(
                        rect.min + egui::vec2(0.0, row * i as f32),
                        egui::vec2(rect.width(), row),
                    );
                    presentation::placeholder_row(&painter, line, i, tokens.raised);
                }
            }
            Body::Tree(Decoded::Failed(reason)) => {
                let text = format!("Not readable as {}: {reason}", self.lens.name());
                wrapped(ui, &painter, rect, &text, tokens.faint);
            }
        }
    }
    /// What the preview shows, for accessibility.
    pub fn describe(&self) -> String {
        match &self.body {
            Body::Hex(bytes) => format!("{} preview of {} bytes", self.lens.name(), bytes.len()),
            Body::Text(text) => text.chars().take(200).collect(),
            Body::Tree(Decoded::Tree(tree)) => tree.json().chars().take(200).collect(),
            Body::Tree(Decoded::Pending) => "Reading".into(),
            Body::Tree(Decoded::Failed(reason)) => reason.clone(),
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
