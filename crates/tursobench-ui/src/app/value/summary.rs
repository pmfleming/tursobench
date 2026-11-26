//! One description of a blob or deferred value, shared by record rows, grid
//! cells, tooltips and accessibility labels. It reads only a value's first
//! [`tursobench_blob::HEAD`] bytes and its length, and is cached by them, so
//! painting never decodes a value.
use super::super::icons::Icon;
use eframe::egui;
use std::{collections::HashMap, sync::Arc};
use tursobench_blob::{HEAD, Kind, text::Encoding, uuid};
use tursobench_core::{DeferredCell, LargeKind, Value};

/// How one part of a summary is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Tone {
    /// The value's own words: a format, a size in pixels, an excerpt.
    Plain,
    /// Bytes or an identifier, in the code font.
    Mono,
    /// Supporting detail, such as the size in bytes.
    Faint,
    /// Leading bytes shown after the size.
    FaintMono,
}

/// What a value is, as an icon and a few parts of text.
pub(in crate::app) struct Summary {
    pub kind: Kind,
    pub icon: Icon,
    pub parts: Vec<(String, Tone)>,
    /// What the bytes are, in a sentence, for tooltips.
    pub description: String,
}

/// Short values show their bytes before their size.
const SHORT: usize = 8;

impl Summary {
    /// The summary of a blob or deferred value; `None` for any other value.
    pub fn of(ctx: &egui::Context, value: &Value) -> Option<Arc<Self>> {
        let key = Key::of(value)?;
        let id = egui::Id::unique("value-summaries");
        Some(ctx.data_mut(|data| data.get_temp_mut_or_default::<Cache>(id).get(key, value)))
    }

    /// The parts as one line of text, for accessibility labels and copies of what is shown.
    pub fn text(&self) -> String {
        let parts: Vec<&str> = self.parts.iter().map(|(text, _)| text.as_str()).collect();
        parts.join(" ")
    }

    /// Each part laid out for one line no wider than `width` in all, with
    /// the gap before it. The longest part gives way, ending in an ellipsis,
    /// so a size or format name stays readable. `monospace` sets every part
    /// in the code font, as grid cells are.
    pub fn layout(
        &self,
        ui: &egui::Ui,
        size: f32,
        monospace: bool,
        color: egui::Color32,
        width: f32,
    ) -> Vec<(f32, Arc<egui::Galley>)> {
        let ctx = ui.ctx();
        let faint = crate::appearance::Tokens::of(ctx).faint;
        let gap = size * 0.5;
        let elastic = (0..self.parts.len())
            .max_by_key(|&i| self.parts[i].0.chars().count())
            .unwrap_or(0);
        let job = |i: usize, width: f32| {
            let (text, tone) = &self.parts[i];
            let font = if monospace || matches!(tone, Tone::Mono | Tone::FaintMono) {
                crate::appearance::monospace(ctx, size - 0.5)
            } else {
                crate::appearance::proportional(ctx, size)
            };
            let color = match tone {
                Tone::Plain | Tone::Mono => color,
                Tone::Faint | Tone::FaintMono => faint,
            };
            let mut job = egui::text::LayoutJob::simple_singleline(text.clone(), font, color);
            job.wrap.max_width = width.max(1.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            ui.painter().layout_job(job)
        };
        let mut galleys: Vec<_> = (0..self.parts.len())
            .map(|i| (if i == 0 { 0.0 } else { gap }, job(i, f32::INFINITY)))
            .collect();
        let fixed: f32 = galleys
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != elastic)
            .map(|(_, (gap, galley))| gap + galley.size().x)
            .sum();
        if let Some((gap, galley)) = galleys.get_mut(elastic) {
            *galley = job(elastic, width - fixed - *gap);
        }
        galleys
    }

    /// The icon, or the value's thumbnail, then the parts, vertically centred in `rect`.
    pub fn paint(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        color: egui::Color32,
        monospace: bool,
        thumb: Option<(egui::TextureId, egui::Vec2)>,
    ) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let icon = if matches!(self.icon, Icon::Warning) {
            ui.visuals().warn_fg_color
        } else {
            tokens.faint
        };
        let at = rect.left_center() + egui::vec2(7.0, 0.0);
        match thumb {
            Some((id, size)) => {
                let slot = egui::Rect::from_center_size(at, egui::Vec2::splat(16.0));
                let scale = super::lens::fit(slot, size);
                super::lens::paint_image(ui, slot, id, size, scale, egui::Vec2::ZERO);
            }
            None => super::super::presentation::icon_at(&painter, at, 13.0, self.icon, icon),
        }
        let size = if monospace { 12.0 } else { 12.5 };
        let mut x = rect.left() + 20.0;
        for (gap, galley) in self.layout(ui, size, monospace, color, rect.right() - x) {
            x += gap;
            let width = galley.size().x;
            painter.galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            x += width;
        }
    }

    /// The summary of a value in a column; plain float32 is named a vector
    /// only where the column's name or type says it holds vectors.
    pub fn for_column(ctx: &egui::Context, value: &Value, vector: bool) -> Option<Arc<Self>> {
        let summary = Self::of(ctx, value)?;
        Some(match value {
            Value::Blob(bytes)
                if vector
                    && bytes.len() >= 8
                    && bytes.len().is_multiple_of(4)
                    && matches!(summary.kind, Kind::Unknown | Kind::Digest) =>
            {
                Arc::new(Self::plain_vector(bytes.len() as u64))
            }
            _ => summary,
        })
    }
    /// Plain float32 in a column whose name or type says it holds vectors.
    pub fn plain_vector(len: u64) -> Self {
        let size = byte_size(len);
        Self::new(
            Kind::Unknown,
            Icon::Vector,
            vec![
                (format!("f32 × {}", len / 4), Tone::Plain),
                (size.clone(), Tone::Faint),
            ],
            format!("Plain float32, {size}: the column's name or type says it holds vectors"),
        )
    }
    /// A kind in a word or two, as the contents lens names what it found.
    pub fn describe_kind(kind: Kind) -> String {
        match kind {
            Kind::Text(encoding) => format!("text ({})", encoding.name()),
            Kind::Jsonb => "JSONB".into(),
            Kind::Structured(format) => format.name().into(),
            Kind::Image { format, .. } => format.name().into(),
            Kind::Compressed(compression) => compression.name().into(),
            Kind::Named(named) => named.name.into(),
            Kind::Vector { layout, .. } => format!("{} vector", layout.name()),
            _ => "bytes".into(),
        }
    }
    fn new(kind: Kind, icon: Icon, parts: Vec<(String, Tone)>, description: String) -> Self {
        Self {
            kind,
            icon,
            parts,
            description,
        }
    }
}

/// What a summary depends on: the first bytes, the length and whether the
/// value is text; a deferred value without backing also says so.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    head: Vec<u8>,
    /// A blob's last bytes, which hold a vector's type.
    tail: Vec<u8>,
    len: u64,
    text: bool,
    held: bool,
}
impl Key {
    fn of(value: &Value) -> Option<Self> {
        let (head, tail, len, text, held) = match value {
            Value::Blob(bytes) => (
                &bytes[..bytes.len().min(HEAD)],
                &bytes[bytes.len().saturating_sub(8)..],
                bytes.len() as u64,
                false,
                true,
            ),
            Value::Deferred(cell) => (
                &cell.head[..cell.head.len().min(HEAD)],
                &[][..],
                cell.bytes,
                cell.kind == LargeKind::Text,
                cell.handle.is_some(),
            ),
            _ => return None,
        };
        Some(Self {
            head: head.to_vec(),
            tail: tail.to_vec(),
            len,
            text,
            held,
        })
    }
}

/// Summaries already made, until there are too many to keep.
#[derive(Clone, Default)]
struct Cache(HashMap<Key, Arc<Summary>>);
impl Cache {
    const LIMIT: usize = 4096;
    fn get(&mut self, key: Key, value: &Value) -> Arc<Summary> {
        if let Some(summary) = self.0.get(&key) {
            return summary.clone();
        }
        if self.0.len() >= Self::LIMIT {
            self.0.clear();
        }
        #[cfg(test)]
        MADE.with(|made| made.set(made.get() + 1));
        // Bytes are untrusted: a reader that panics leaves only the size.
        let made = std::panic::catch_unwind(|| match value {
            Value::Deferred(cell) => deferred(cell),
            _ => describe(&key.head, &key.tail, key.len),
        });
        let summary = Arc::new(made.unwrap_or_else(|_| {
            let size = byte_size(key.len);
            Summary::new(
                Kind::Unknown,
                Icon::Bytecode,
                vec![(size.clone(), Tone::Plain)],
                format!("{size} that could not be described"),
            )
        }));
        self.0.insert(key, summary.clone());
        summary
    }
}
#[cfg(test)]
thread_local! {
    /// How many summaries were made rather than found in the cache.
    pub(in crate::app) static MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A size in bytes, then in KB, MB or GB with one decimal.
pub(in crate::app) fn byte_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let (unit, divisor) = if bytes >= 1024 * 1024 * 1024 {
        ("GB", 1024.0 * 1024.0 * 1024.0)
    } else if bytes >= 1024 * 1024 {
        ("MB", 1024.0 * 1024.0)
    } else {
        ("KB", 1024.0)
    };
    format!("{:.1} {unit}", bytes as f64 / divisor)
}

fn hex(bytes: &[u8], spaced: bool) -> String {
    let pairs: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    pairs.join(if spaced { " " } else { "" })
}

/// The first line of text, with any other control characters shown as spaces.
fn excerpt(text: &str, more: bool) -> String {
    let line = text.trim_start().lines().next().unwrap_or_default();
    let mut out: String = line
        .chars()
        .take(HEAD)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if more || line.len() < text.trim().len() {
        out.push('…');
    }
    out
}

fn deferred(cell: &DeferredCell) -> Summary {
    let size = byte_size(cell.bytes);
    if cell.handle.is_none() {
        return Summary::new(
            tursobench_blob::sniff(&cell.head, cell.bytes),
            Icon::Warning,
            vec![
                (size.clone(), Tone::Plain),
                ("over the 64 MiB limit · not held".into(), Tone::Faint),
            ],
            format!("{size}: too large to keep, so only its first bytes can be shown"),
        );
    }
    if cell.kind == LargeKind::Text {
        let valid = match std::str::from_utf8(&cell.head) {
            Ok(text) => text,
            Err(e) => std::str::from_utf8(&cell.head[..e.valid_up_to()]).unwrap_or_default(),
        };
        return Summary::new(
            Kind::Text(Encoding::Utf8),
            Icon::Text,
            vec![
                (excerpt(valid, true), Tone::Plain),
                (size.clone(), Tone::Faint),
            ],
            format!("Text, {size}"),
        );
    }
    describe(&cell.head, &[], cell.bytes)
}

/// The summary of a blob `len` bytes long that starts with `head` and ends with `tail`.
fn describe(head: &[u8], tail: &[u8], len: u64) -> Summary {
    let kind = tursobench_blob::sniff::sniff_tail(head, tail, len);
    let size = byte_size(len);
    let sized = |text: String| vec![(text, Tone::Plain), (size.clone(), Tone::Faint)];
    match kind {
        Kind::Empty => Summary::new(
            kind,
            Icon::Bytecode,
            vec![("empty".into(), Tone::Faint)],
            "An empty blob".into(),
        ),
        Kind::Image {
            format,
            size: pixels,
        } => {
            let name = format.name();
            let (label, description) = match pixels {
                Some((w, h)) => (
                    format!("{name} {w} × {h}"),
                    format!("{name} image, {w} × {h} pixels, {size}"),
                ),
                None => (name.to_owned(), format!("{name} image, {size}")),
            };
            Summary::new(kind, Icon::Image, sized(label), description)
        }
        Kind::Text(encoding) => {
            let partial = (head.len() as u64) < len;
            let text = tursobench_blob::text::decode(head, encoding);
            let mut parts = sized(excerpt(text.trim_end_matches('\u{FFFD}'), partial));
            if encoding != Encoding::Utf8 {
                parts.push((encoding.name().into(), Tone::Faint));
            }
            let description = format!("{} text stored as a blob, {size}", encoding.name());
            Summary::new(kind, Icon::Text, parts, description)
        }
        Kind::Uuid { version } => {
            let bytes: &[u8; 16] = head.try_into().unwrap_or(&[0; 16]);
            Summary::new(
                kind,
                Icon::Hash,
                vec![(uuid::format(bytes, uuid::Layout::Standard), Tone::Mono)],
                format!("UUID version {version}, 16 bytes"),
            )
        }
        Kind::Digest => Summary::new(
            kind,
            Icon::Hash,
            vec![(hex(head, false), Tone::Mono)],
            format!("{size} that are not text: a digest or key"),
        ),
        Kind::Jsonb => {
            let json = tursobench_blob::tree::jsonb_prefix(head, len)
                .map(|tree| tree.json())
                .unwrap_or_default();
            Summary::new(
                kind,
                Icon::Tree,
                vec![(json, Tone::Mono), ("JSONB".into(), Tone::Faint)],
                format!("SQLite JSONB, {size}"),
            )
        }
        Kind::Structured(format) => Summary::new(
            kind,
            Icon::Tree,
            sized(format.name().into()),
            format!("{}, {size}", format.name()),
        ),
        Kind::Vector { layout, dims } => Summary::new(
            kind,
            Icon::Vector,
            sized(format!("{} × {dims}", layout.name())),
            format!("A {} vector of {dims} dimensions, {size}", layout.name()),
        ),
        Kind::Compressed(compression) => Summary::new(
            kind,
            Icon::Contents,
            sized(compression.name().into()),
            format!("{}-compressed data, {size}", compression.name()),
        ),
        Kind::Named(named) => Summary::new(
            kind,
            Icon::Bytecode,
            sized(named.name.into()),
            format!("{}, {size}", named.name),
        ),
        Kind::Unknown if len as usize <= SHORT => Summary::new(
            kind,
            Icon::Bytecode,
            vec![(hex(head, true), Tone::Mono), (size.clone(), Tone::Faint)],
            format!("{size} in no recognised format"),
        ),
        Kind::Unknown => Summary::new(
            kind,
            Icon::Bytecode,
            vec![
                (size.clone(), Tone::Plain),
                (
                    format!("{} …", hex(&head[..head.len().min(16)], true)),
                    Tone::FaintMono,
                ),
            ],
            format!("{size} in no recognised format"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{MADE, Summary, Tone, byte_size};
    use eframe::egui;
    use tursobench_core::{DeferredCell, LargeKind, Value};

    fn parts(value: &Value) -> Vec<(String, Tone)> {
        let ctx = egui::Context::default();
        Summary::of(&ctx, value).unwrap().parts.clone()
    }
    fn texts(value: &Value) -> Vec<String> {
        parts(value).into_iter().map(|(text, _)| text).collect()
    }
    fn deferred(head: &[u8], bytes: u64, kind: LargeKind, handle: Option<u64>) -> Value {
        Value::Deferred(Box::new(DeferredCell {
            handle,
            kind,
            bytes,
            head: head.to_vec(),
        }))
    }

    #[test]
    fn each_kind_of_blob_is_summarised_by_its_first_bytes_and_length() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x40\0\0\0\x30".to_vec();
        png.extend([8, 2, 0, 0, 0]);
        let uuid = 0x3f2a9c1e_7b40_4d1a_9e5c_0a1b2c3d4e5f_u128
            .to_be_bytes()
            .to_vec();
        let cases: Vec<(Value, Vec<&str>)> = vec![
            (Value::Blob(vec![0, 255, 122]), vec!["00 ff 7a", "3 bytes"]),
            (Value::Blob(vec![]), vec!["empty"]),
            (Value::Blob(png.clone()), vec!["PNG 64 × 48", "29 bytes"]),
            (
                Value::Blob(b"\xFF\xD8\xFF\xE0".to_vec()),
                vec!["JPEG", "4 bytes"],
            ),
            (
                Value::Blob("Synthetic note λ\nsecond".into()),
                vec!["Synthetic note λ…", "24 bytes"],
            ),
            (
                Value::Blob(uuid),
                vec!["3f2a9c1e-7b40-4d1a-9e5c-0a1b2c3d4e5f"],
            ),
            (
                Value::Blob(vec![0xAB; 20]),
                vec!["abababababababababababababababababababab"],
            ),
            (
                Value::Blob(vec![0x1f, 0x8b, 0x08, 0, 0]),
                vec!["gzip", "5 bytes"],
            ),
            (Value::Blob(b"%PDF-1.7\n".to_vec()), vec!["PDF", "9 bytes"]),
            (
                Value::Blob(b"\x4c\x17a\x131".to_vec()),
                vec![r#"{"a":1}"#, "JSONB"],
            ),
            (
                Value::Blob((0..=255).collect()),
                vec![
                    "256 bytes",
                    "00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f …",
                ],
            ),
            (
                deferred(&png, 3_200_000, LargeKind::Blob, Some(1)),
                vec!["PNG 64 × 48", "3.1 MB"],
            ),
            (
                deferred("ab λ".as_bytes(), 100_000, LargeKind::Text, Some(1)),
                vec!["ab λ…", "97.7 KB"],
            ),
            (
                deferred(&png, 85_000_000, LargeKind::Blob, None),
                vec!["81.1 MB", "over the 64 MiB limit · not held"],
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(texts(&value), expected, "{value:?}");
        }
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("hi there".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(
            texts(&Value::Blob(utf16)),
            ["hi there", "18 bytes", "UTF-16LE"]
        );
        assert_eq!(parts(&Value::Blob(vec![1, 2]))[0].1, Tone::Mono);
        assert!(Summary::of(&egui::Context::default(), &Value::Text("x".into())).is_none());
        assert_eq!(byte_size(4_404_019), "4.2 MB");
    }

    #[test]
    fn summaries_are_made_once_per_distinct_head_and_length() {
        let ctx = egui::Context::default();
        let before = MADE.with(std::cell::Cell::get);
        for _ in 0..3 {
            for i in 0..50u8 {
                let summary = Summary::of(&ctx, &Value::Blob(vec![0, 255, i])).unwrap();
                assert!(summary.text().starts_with("00 ff"));
            }
        }
        // A long value with the same first 64 and last 8 bytes and length is the same summary.
        let mut long = vec![7u8; 100_000];
        Summary::of(&ctx, &Value::Blob(long.clone()));
        long[50_000] = 8;
        Summary::of(&ctx, &Value::Blob(long));
        assert_eq!(MADE.with(std::cell::Cell::get) - before, 51);
    }
}
