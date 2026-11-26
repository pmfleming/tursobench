//! Structured values as a flat, bounded tree, read from SQLite JSONB
//! (<https://sqlite.org/jsonb.html>).
//!
//! Text is kept as the body of a JSON string, escapes included, so the tree
//! prints back as the JSON the engine's `json()` would return.
use std::sync::atomic::{AtomicBool, Ordering};

/// How much of a value a decoder may read into a tree.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub nodes: usize,
    pub depth: usize,
    /// Bytes of key and text kept, in total.
    pub text: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            nodes: 10_000,
            depth: 64,
            text: 1024 * 1024,
        }
    }
}

/// One value in the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Null,
    Bool(bool),
    /// A number as JSON text.
    Number(String),
    /// The body of a JSON string, escapes included.
    Text(String),
    /// Bytes: up to 16 as hexadecimal, and how many there are.
    Bytes(String, usize),
    /// An array of this many elements.
    Array(usize),
    /// An object of this many members.
    Object(usize),
}

/// A value, its key within an object, and where its subtree ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub depth: usize,
    /// The member name, as the body of a JSON string.
    pub key: Option<String>,
    pub item: Item,
    /// The index after this node's last descendant.
    pub end: usize,
    /// An array or object whose reading stopped before its end.
    pub cut: bool,
}

/// Values in document order, each before its children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// False when a limit or the end of the input stopped the reading.
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Cancelled,
}

/// The bytes as SQLite JSONB: one element that exactly fills them.
pub fn jsonb(bytes: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Tree, Error> {
    let mut reader = Reader {
        bytes,
        limits,
        cancel,
        nodes: Vec::new(),
        text: 0,
        partial: false,
        stopped: false,
    };
    let end = reader.element(0, bytes.len(), 0, None)?;
    if end != bytes.len() {
        return Err(Error::Invalid("Bytes follow the JSONB element"));
    }
    Ok(Tree {
        complete: !reader.stopped,
        nodes: reader.nodes,
    })
}

/// The leading bytes of a value `len` bytes long, read as far as they go:
/// `None` unless they start a JSONB array or object that exactly fills the
/// value and is valid up to where the bytes end.
pub fn jsonb_prefix(head: &[u8], len: u64) -> Option<Tree> {
    let (kind, header, size) = header(head, 0)?;
    if !matches!(kind, ARRAY | OBJECT) || header as u64 + size != len {
        return None;
    }
    let limits = Limits::default();
    let cancel = AtomicBool::new(false);
    let mut reader = Reader {
        bytes: head,
        limits: &limits,
        cancel: &cancel,
        nodes: Vec::new(),
        text: 0,
        partial: (head.len() as u64) < len,
        stopped: false,
    };
    reader.element(0, head.len(), 0, None).ok()?;
    Some(Tree {
        complete: !reader.stopped && head.len() as u64 == len,
        nodes: reader.nodes,
    })
}

const NULL: u8 = 0;
const TRUE: u8 = 1;
const FALSE: u8 = 2;
const INT: u8 = 3;
const INT5: u8 = 4;
const FLOAT: u8 = 5;
const FLOAT5: u8 = 6;
const TEXT: u8 = 7;
const TEXTJ: u8 = 8;
const TEXT5: u8 = 9;
const TEXTRAW: u8 = 10;
const ARRAY: u8 = 11;
const OBJECT: u8 = 12;

/// An element's type, header length and payload size, read at `at`.
fn header(bytes: &[u8], at: usize) -> Option<(u8, usize, u64)> {
    let first = *bytes.get(at)?;
    let extra = match first >> 4 {
        code @ 0..=11 => return Some((first & 0x0F, 1, u64::from(code))),
        12 => 1,
        13 => 2,
        14 => 4,
        _ => 8,
    };
    let size = bytes
        .get(at + 1..at + 1 + extra)?
        .iter()
        .fold(0u64, |n, &b| n << 8 | u64::from(b));
    Some((first & 0x0F, 1 + extra, size))
}

struct Reader<'a> {
    bytes: &'a [u8],
    limits: &'a Limits,
    cancel: &'a AtomicBool,
    nodes: Vec<Node>,
    text: usize,
    /// The bytes may end inside an element; stop there instead of failing.
    partial: bool,
    stopped: bool,
}
impl Reader<'_> {
    /// Read the element at `at`, which must end by `end`; returns where it ends.
    fn element(
        &mut self,
        at: usize,
        end: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let Some((kind, header, size)) = header(self.bytes, at) else {
            return self.cut(end, "Truncated JSONB header");
        };
        let start = at + header;
        let stop = usize::try_from(size)
            .ok()
            .and_then(|size| start.checked_add(size))
            .filter(|&stop| stop <= end);
        let Some(stop) = stop else {
            if !self.partial || end != self.bytes.len() {
                return Err(Error::Invalid("JSONB element overruns its container"));
            }
            // The bytes end inside this element: read a container as far as they go.
            let index = self.nodes.len();
            if matches!(kind, ARRAY | OBJECT) && start <= end {
                self.container(kind, start, end, depth, key)?;
                self.nodes[index].cut = true;
            }
            self.stopped = true;
            return Ok(end);
        };
        if self.nodes.len() >= self.limits.nodes || depth > self.limits.depth {
            self.stopped = true;
            return Ok(stop);
        }
        let payload = &self.bytes[start..stop];
        let item = match kind {
            NULL | TRUE | FALSE if !payload.is_empty() => {
                return Err(Error::Invalid("JSONB literal with a payload"));
            }
            NULL => Item::Null,
            TRUE => Item::Bool(true),
            FALSE => Item::Bool(false),
            INT | INT5 | FLOAT | FLOAT5 => Item::Number(number(kind, payload)?),
            TEXT..=TEXTRAW => Item::Text(self.text(kind, payload)?),
            ARRAY | OBJECT => {
                self.container(kind, start, stop, depth, key)?;
                return Ok(stop);
            }
            _ => return Err(Error::Invalid("Reserved JSONB element type")),
        };
        self.nodes.push(Node {
            depth,
            key,
            item,
            end: self.nodes.len() + 1,
            cut: false,
        });
        Ok(stop)
    }

    /// Without `partial`, a truncated element is invalid; with it, reading stops.
    fn cut(&mut self, end: usize, reason: &'static str) -> Result<usize, Error> {
        if self.partial {
            self.stopped = true;
            Ok(end)
        } else {
            Err(Error::Invalid(reason))
        }
    }

    /// An array's elements, or an object's alternating names and values,
    /// filling `start..stop` exactly.
    fn container(
        &mut self,
        kind: u8,
        start: usize,
        stop: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<(), Error> {
        let index = self.nodes.len();
        let item = if kind == ARRAY {
            Item::Array(0)
        } else {
            Item::Object(0)
        };
        self.nodes.push(Node {
            depth,
            key,
            item,
            end: index + 1,
            cut: false,
        });
        let mut at = start;
        let mut count = 0;
        while at < stop && !self.stopped {
            let name = if kind == OBJECT {
                let (name, next) = self.name(at, stop)?;
                at = next;
                if at >= stop {
                    // A name with no value is only possible where the bytes ran out.
                    self.cut(stop, "JSONB object member without a value")?;
                    break;
                }
                Some(name)
            } else {
                None
            };
            at = self.element(at, stop, depth + 1, name)?;
            count += 1;
        }
        self.nodes[index].end = self.nodes.len();
        self.nodes[index].cut = self.stopped;
        self.nodes[index].item = if kind == ARRAY {
            Item::Array(count)
        } else {
            Item::Object(count)
        };
        Ok(())
    }

    /// A member name: a text element at `at`.
    fn name(&mut self, at: usize, stop: usize) -> Result<(String, usize), Error> {
        let Some((kind, header, size)) = header(self.bytes, at) else {
            return self
                .cut(stop, "Truncated JSONB name")
                .map(|end| (String::new(), end));
        };
        if !(TEXT..=TEXTRAW).contains(&kind) {
            return Err(Error::Invalid("JSONB object name is not text"));
        }
        let end = (at + header)
            .checked_add(usize::try_from(size).unwrap_or(usize::MAX))
            .filter(|&end| end <= stop);
        match end {
            Some(end) => Ok((self.text(kind, &self.bytes[at + header..end])?, end)),
            None => self
                .cut(stop, "JSONB name overruns its object")
                .map(|end| (String::new(), end)),
        }
    }

    /// Text as the body of a JSON string, within the text budget.
    fn text(&mut self, kind: u8, payload: &[u8]) -> Result<String, Error> {
        let raw =
            std::str::from_utf8(payload).map_err(|_| Error::Invalid("JSONB text is not UTF-8"))?;
        self.text += raw.len();
        if self.text > self.limits.text {
            self.stopped = true;
        }
        Ok(match kind {
            TEXT | TEXTJ => raw.to_owned(),
            TEXT5 => json5_text(raw),
            _ => escape(raw),
        })
    }
}

/// A JSONB number as canonical JSON: hexadecimal integers in decimal and
/// JSON5 reals with their missing digits.
fn number(kind: u8, payload: &[u8]) -> Result<String, Error> {
    let text = std::str::from_utf8(payload)
        .ok()
        .filter(|t| !t.is_empty() && t.is_ascii())
        .ok_or(Error::Invalid("JSONB number is not ASCII"))?;
    Ok(match kind {
        INT5 => {
            let (sign, digits) = match text.as_bytes()[0] {
                b'-' => ("-", &text[1..]),
                b'+' => ("", &text[1..]),
                _ => ("", text),
            };
            match digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))
            {
                Some(hex) => u128::from_str_radix(hex, 16)
                    .map(|n| format!("{sign}{n}"))
                    .map_err(|_| Error::Invalid("Invalid JSON5 hexadecimal"))?,
                None => format!("{sign}{digits}"),
            }
        }
        FLOAT5 => json5_real(text),
        _ => text.to_owned(),
    })
}

/// A JSON5 real (`.5`, `5.`, `+1`, `Infinity`, `NaN`) as JSON, as SQLite writes it.
fn json5_real(text: &str) -> String {
    let (sign, body) = match text.as_bytes()[0] {
        b'-' => ("-", &text[1..]),
        b'+' => ("", &text[1..]),
        _ => ("", text),
    };
    if body.eq_ignore_ascii_case("infinity") || body.eq_ignore_ascii_case("inf") {
        return format!("{sign}9.0e999");
    }
    if body.eq_ignore_ascii_case("nan") {
        return "null".into();
    }
    let mut out = String::from(sign);
    if body.starts_with('.') {
        out.push('0');
    }
    for (i, c) in body.char_indices() {
        out.push(c);
        let next = body[i + 1..].chars().next();
        if c == '.' && !next.is_some_and(|n| n.is_ascii_digit()) {
            out.push('0');
        }
    }
    out
}

/// JSON5 string escapes rewritten as JSON ones.
fn json5_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\'') => out.push('\''),
            Some('v') => out.push_str("\\u000b"),
            Some('0') => out.push_str("\\u0000"),
            Some('x') => {
                out.push_str("\\u00");
                out.extend(chars.by_ref().take(2));
            }
            // A backslash before a line ending continues the string.
            Some('\r') => {
                chars.next_if_eq(&'\n');
            }
            Some('\n' | '\u{2028}' | '\u{2029}') => {}
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Raw text escaped as the body of a JSON string.
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// One part of a printed line, and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Punctuation,
    Key,
    Text,
    Number,
    Literal,
}

impl Tree {
    /// The tree as compact JSON, `…` marking where an incomplete tree stops.
    pub fn json(&self) -> String {
        let mut out = String::new();
        for line in self.lines(usize::MAX, false) {
            for (text, _) in line.parts {
                out.push_str(&text);
            }
        }
        out
    }

    /// The tree as JSON printed two spaces to a level, at most `limit` lines.
    pub fn pretty(&self, limit: usize) -> Vec<Line> {
        self.lines(limit, true)
    }

    fn lines(&self, limit: usize, pretty: bool) -> Vec<Line> {
        let mut printer = Printer {
            tree: self,
            lines: Vec::new(),
            current: Line::default(),
            pretty,
            limit,
            elided: false,
        };
        if !self.nodes.is_empty() {
            printer.node(0);
        }
        printer.finish();
        printer.lines
    }
}

/// A printed line: its indentation level and its parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub depth: usize,
    pub parts: Vec<(String, Part)>,
}

struct Printer<'a> {
    tree: &'a Tree,
    lines: Vec<Line>,
    current: Line,
    pretty: bool,
    limit: usize,
    /// Whether the `…` for a cut container has been printed.
    elided: bool,
}
impl Printer<'_> {
    fn full(&self) -> bool {
        self.lines.len() >= self.limit
    }
    fn push(&mut self, text: &str, part: Part) {
        self.current.parts.push((text.to_owned(), part));
    }
    fn finish(&mut self) {
        if !self.current.parts.is_empty() && !self.full() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }
    fn newline(&mut self, depth: usize) {
        if self.pretty {
            self.finish();
            self.current = Line {
                depth,
                parts: Vec::new(),
            };
        }
    }
    /// Print node `index` and its subtree; returns the index after it.
    fn node(&mut self, index: usize) -> usize {
        let node = &self.tree.nodes[index];
        if let Some(key) = &node.key {
            self.push(&format!("\"{key}\""), Part::Key);
            self.push(if self.pretty { ": " } else { ":" }, Part::Punctuation);
        }
        let (open, close) = match &node.item {
            Item::Array(_) => ("[", "]"),
            Item::Object(_) => ("{", "}"),
            item => {
                let (text, part) = scalar(item);
                self.push(&text, part);
                return index + 1;
            }
        };
        self.push(open, Part::Punctuation);
        let (depth, end, cut) = (node.depth, node.end, node.cut);
        let mut child = index + 1;
        while child < end && !self.full() {
            if child > index + 1 {
                self.push(",", Part::Punctuation);
            }
            self.newline(depth + 1);
            child = self.node(child);
        }
        // Only the innermost cut container says that more follows.
        if cut {
            if !self.elided {
                self.push(if end > index + 1 { ",…" } else { "…" }, Part::Punctuation);
                self.elided = true;
            }
            return end;
        }
        if end > index + 1 {
            self.newline(depth);
        }
        self.push(close, Part::Punctuation);
        end
    }
}

fn scalar(item: &Item) -> (String, Part) {
    match item {
        Item::Null => ("null".into(), Part::Literal),
        Item::Bool(b) => (b.to_string(), Part::Literal),
        Item::Number(n) => (n.clone(), Part::Number),
        Item::Text(t) => (format!("\"{t}\""), Part::Text),
        Item::Bytes(hex, len) => (format!("h'{hex}' ({len} bytes)"), Part::Literal),
        Item::Array(_) | Item::Object(_) => (String::new(), Part::Punctuation),
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Item, Limits, jsonb, jsonb_prefix};
    use std::sync::atomic::AtomicBool;

    /// `{"job":"export-812","rows":48213,"ok":true,"tags":["a",null]}` as JSONB.
    fn sample() -> Vec<u8> {
        let mut members = Vec::new();
        for (k, v) in [
            (&b"job"[..], &b"\xa7export-812"[..]),
            (b"rows", b"\x5348213"),
            (b"ok", b"\x01"),
            (b"tags", b"\x3b\x17a\x00"),
        ] {
            members.push(0x07 | (k.len() as u8) << 4);
            members.extend(k);
            members.extend(v);
        }
        let mut out = vec![0xCC, members.len() as u8];
        out.extend(members);
        out
    }

    #[test]
    fn jsonb_prints_back_as_json() {
        let cancel = AtomicBool::new(false);
        let tree = jsonb(&sample(), &Limits::default(), &cancel).unwrap();
        assert!(tree.complete);
        assert_eq!(
            tree.json(),
            r#"{"job":"export-812","rows":48213,"ok":true,"tags":["a",null]}"#
        );
        assert_eq!(tree.nodes[0].item, Item::Object(4));
        assert_eq!(tree.nodes[0].end, tree.nodes.len());
        let pretty = tree.pretty(4);
        assert_eq!(pretty.len(), 4);
        let first: String = pretty[1].parts.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            (pretty[1].depth, first.as_str()),
            (1, "\"job\": \"export-812\",")
        );
    }

    #[test]
    fn invalid_truncated_and_trailing_bytes_are_refused() {
        let cancel = AtomicBool::new(false);
        let limits = Limits::default();
        let good = sample();
        for bad in [
            &good[..good.len() - 1],
            &[good.as_slice(), &[0]].concat()[..],
            &[0x1E, 0x00][..],
            &[0x1C, 0x13, b'1'][..],
            &[0x2B, 0x11, b'x'][..],
            &[0x0D][..],
            &[0xFB, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF][..],
            &[0x27, 0xFF, 0xFE][..],
            &[][..],
        ] {
            assert!(jsonb(bad, &limits, &cancel).is_err(), "{bad:?}");
        }
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(jsonb(&good, &limits, &cancel), Err(Error::Cancelled));
    }

    #[test]
    fn limits_stop_reading_without_failing() {
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            nodes: 3,
            ..Limits::default()
        };
        let tree = jsonb(&sample(), &limits, &cancel).unwrap();
        assert!(!tree.complete);
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.json(), r#"{"job":"export-812","rows":48213,…"#);
        // Nesting past the depth limit stops there.
        let mut deep = vec![0x0B];
        for _ in 0..100 {
            let len = deep.len();
            let mut outer = vec![0xEB];
            outer.extend((len as u32).to_be_bytes());
            outer.extend(deep);
            deep = outer;
        }
        let tree = jsonb(&deep, &Limits::default(), &cancel).unwrap();
        assert!(!tree.complete);
        assert!(tree.nodes.len() <= 66);
    }

    #[test]
    fn a_prefix_prints_as_far_as_it_goes() {
        let good = sample();
        let head = &good[..20];
        let tree = jsonb_prefix(head, good.len() as u64).unwrap();
        assert!(!tree.complete);
        assert_eq!(tree.json(), r#"{"job":"export-812",…"#);
        assert!(jsonb_prefix(&good, good.len() as u64 + 1).is_none());
        assert!(jsonb_prefix(b"\x13", 2).is_none());
        // Text whose first byte happens to give the right length is not JSONB.
        assert!(jsonb_prefix(b"Lab12", 5).is_none());
        // A whole value is read strictly: an element cut short is invalid.
        assert!(jsonb_prefix(b"\x3b\x17a", 3).is_none());
        assert!(jsonb_prefix(b"\x3b\x17a", 4).is_some());
        assert_eq!(jsonb_prefix(&good, good.len() as u64).unwrap().json(), {
            let cancel = AtomicBool::new(false);
            jsonb(&good, &Limits::default(), &cancel).unwrap().json()
        });
    }
}
