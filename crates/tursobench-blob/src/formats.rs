//! BSON, binary property lists, CBOR, MessagePack and protobuf read into the
//! same bounded tree as JSONB. Each must consume the value exactly.
use crate::{
    bytes,
    tree::{Error, Item, Limits, Node, Tree, escape},
};
use std::sync::atomic::{AtomicBool, Ordering};

/// A structured format the tree lens reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Jsonb,
    Bson,
    Plist,
    Cbor,
    MessagePack,
    Protobuf,
}
impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::Jsonb => "JSONB",
            Self::Bson => "BSON",
            Self::Plist => "plist",
            Self::Cbor => "CBOR",
            Self::MessagePack => "MessagePack",
            Self::Protobuf => "protobuf",
        }
    }
}

/// Read `bytes` as `format` into a tree.
pub fn decode(
    format: Format,
    bytes: &[u8],
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Tree, Error> {
    let mut b = Builder::new(limits, cancel);
    let end = match format {
        Format::Jsonb => return crate::tree::jsonb(bytes, limits, cancel),
        Format::Bson => bson::document(&mut b, bytes, 0, 0, None, false)?,
        Format::Plist => plist::read(&mut b, bytes)?,
        Format::Cbor => cbor::item(&mut b, bytes, cbor::start(bytes), 0, None)?,
        Format::MessagePack => msgpack::item(&mut b, bytes, 0, 0, None)?,
        Format::Protobuf => protobuf::message(&mut b, bytes, 0, None)?,
    };
    if end != bytes.len() {
        return Err(Error::Invalid("Bytes follow the value"));
    }
    Ok(b.finish())
}

/// A format with no signature that reads the whole value as a map, an
/// array or a message: offered as a possible reading, never chosen.
pub fn probe(bytes: &[u8], cancel: &AtomicBool) -> Option<Format> {
    let limits = Limits::default();
    let container = |tree: &Tree| matches!(tree.nodes.first().map(|n| &n.item), Some(Item::Object(n) | Item::Array(n)) if *n > 0);
    [Format::MessagePack, Format::Cbor, Format::Protobuf]
        .into_iter()
        .find(|&format| decode(format, bytes, &limits, cancel).is_ok_and(|tree| container(&tree)))
}

/// A format recognised by its first bytes: BSON by its length field, a
/// binary plist by its magic, CBOR by its self-describe tag.
pub fn signature(head: &[u8], len: u64) -> Option<Format> {
    if head.starts_with(b"bplist00") {
        return Some(Format::Plist);
    }
    if head.starts_with(&[0xd9, 0xd9, 0xf7]) {
        return Some(Format::Cbor);
    }
    let declared = u32::from_le_bytes(head.get(..4)?.try_into().ok()?);
    let first = *head.get(4)?;
    let key = head.get(5..)?.iter().take_while(|&&b| b != 0).count();
    let plausible = matches!(first, 0x01..=0x0b | 0x0d | 0x0e | 0x10..=0x13);
    (u64::from(declared) == len
        && len >= 7
        && plausible
        && key > 0
        && head[5..5 + key].iter().all(u8::is_ascii_graphic))
    .then_some(Format::Bson)
}

/// How deep a reader may recurse, map keys included. Past the tree's depth
/// limit nodes are no longer kept but the value is still read to its end, so
/// this bounds the stack, not the tree.
const NESTING: usize = 128;

/// Builds a tree within the limits.
struct Builder<'a> {
    limits: &'a Limits,
    cancel: &'a AtomicBool,
    nodes: Vec<Node>,
    text: usize,
    stopped: bool,
    /// The nesting of the value this builder reads a map key of.
    base: usize,
}
impl<'a> Builder<'a> {
    fn new(limits: &'a Limits, cancel: &'a AtomicBool) -> Self {
        Self {
            limits,
            cancel,
            nodes: Vec::new(),
            text: 0,
            stopped: false,
            base: 0,
        }
    }
    /// A builder for a map key read at `depth`, nested where the key is.
    fn key(&self, depth: usize) -> Builder<'a> {
        Builder {
            base: self.base + depth + 1,
            ..Builder::new(self.limits, self.cancel)
        }
    }
    /// Refuse a value nested deeper than a reader may recurse.
    fn enter(&self, depth: usize) -> Result<(), Error> {
        if self.base + depth > NESTING {
            return Err(Error::Invalid("The value is nested too deeply to read"));
        }
        Ok(())
    }
    /// Whether another node may be added; cancelling fails the read.
    fn room(&mut self, depth: usize) -> Result<bool, Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if self.nodes.len() >= self.limits.nodes
            || depth > self.limits.depth
            || self.text > self.limits.text
        {
            self.stopped = true;
        }
        Ok(!self.stopped)
    }
    fn scalar(&mut self, depth: usize, key: Option<String>, item: Item) -> Result<(), Error> {
        if self.room(depth)? {
            let end = self.nodes.len() + 1;
            self.nodes.push(Node {
                depth,
                key,
                item,
                end,
                cut: false,
            });
        }
        Ok(())
    }
    /// Start an array or object; `None` once the limits are reached.
    fn open(
        &mut self,
        depth: usize,
        key: Option<String>,
        object: bool,
    ) -> Result<Option<usize>, Error> {
        if !self.room(depth)? {
            return Ok(None);
        }
        let item = if object {
            Item::Object(0)
        } else {
            Item::Array(0)
        };
        self.nodes.push(Node {
            depth,
            key,
            item,
            end: self.nodes.len() + 1,
            cut: false,
        });
        Ok(Some(self.nodes.len() - 1))
    }
    fn close(&mut self, index: Option<usize>, count: usize) {
        let Some(index) = index else {
            return;
        };
        let end = self.nodes.len();
        let node = &mut self.nodes[index];
        node.end = end;
        node.cut = self.stopped;
        node.item = match node.item {
            Item::Object(_) => Item::Object(count),
            _ => Item::Array(count),
        };
    }
    /// Text as the body of a JSON string, counted against the text limit.
    fn text(&mut self, raw: &str) -> String {
        self.text += raw.len();
        escape(raw)
    }
    fn finish(self) -> Tree {
        Tree {
            complete: !self.stopped,
            nodes: self.nodes,
        }
    }
}

const TRUNCATED: Error = Error::Invalid("The value ends inside an element");

/// `n` bytes at `at`, or invalid.
fn take(bytes: &[u8], at: usize, n: usize) -> Result<&[u8], Error> {
    bytes::slice_at(bytes, at, n).ok_or(TRUNCATED)
}
fn array_at<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], Error> {
    bytes::array_at(bytes, at).ok_or(TRUNCATED)
}
fn uint_be(bytes: &[u8], at: usize, n: usize) -> Result<u64, Error> {
    bytes::uint_be(bytes, at, n).ok_or(TRUNCATED)
}

type ReadItem = fn(&mut Builder<'_>, &[u8], usize, usize, Option<String>) -> Result<usize, Error>;

/// Read the entries shared by MessagePack and CBOR arrays and maps.
/// A CBOR collection without a length ends at the next break marker.
fn packed_collection(
    b: &mut Builder,
    bytes: &[u8],
    mut at: usize,
    len: Option<usize>,
    (depth, key): (usize, Option<String>),
    map: bool,
    item: ReadItem,
) -> Result<usize, Error> {
    let index = b.open(depth, key, map)?;
    let mut count = 0;
    while len != Some(count) {
        if len.is_none() && bytes.get(at) == Some(&0xff) {
            at += 1;
            break;
        }
        let name = if map {
            let mut keys = b.key(depth);
            at = item(&mut keys, bytes, at, 0, None)?;
            Some(key_text(keys.finish()))
        } else {
            None
        };
        at = item(b, bytes, at, depth + 1, name)?;
        count += 1;
    }
    b.close(index, count);
    Ok(at)
}

fn number(n: impl std::fmt::Display) -> Item {
    Item::Number(n.to_string())
}
fn real(n: f64) -> Item {
    if n.is_finite() {
        Item::Number(format!("{n:?}"))
    } else {
        Item::Null
    }
}
/// Up to 16 bytes as hexadecimal, with the full length.
fn bytes_item(bytes: &[u8]) -> Item {
    let more = if bytes.len() > 16 { "…" } else { "" };
    Item::Bytes(
        format!(
            "{}{more}",
            crate::encode::hex(&bytes[..bytes.len().min(16)], "")
        ),
        bytes.len(),
    )
}

mod bson {
    use super::{Builder, Error, Item, array_at, bytes_item, number, real, take};

    fn i32_at(bytes: &[u8], at: usize) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(array_at(bytes, at)?))
    }
    fn cstring(bytes: &[u8], at: usize) -> Result<(&str, usize), Error> {
        let len = bytes
            .get(at..)
            .and_then(|rest| rest.iter().position(|&b| b == 0))
            .ok_or(Error::Invalid("Unterminated BSON name"))?;
        let text = std::str::from_utf8(&bytes[at..at + len])
            .map_err(|_| Error::Invalid("BSON name is not UTF-8"))?;
        Ok((text, at + len + 1))
    }
    /// A document (or array) at `at`; returns where it ends.
    pub(super) fn document(
        b: &mut Builder,
        bytes: &[u8],
        at: usize,
        depth: usize,
        key: Option<String>,
        array: bool,
    ) -> Result<usize, Error> {
        b.enter(depth)?;
        let len = usize::try_from(i32_at(bytes, at)?)
            .map_err(|_| Error::Invalid("Negative BSON length"))?;
        let end = at
            .checked_add(len)
            .filter(|&e| len >= 5 && e <= bytes.len())
            .ok_or(Error::Invalid("BSON length overruns the value"))?;
        if bytes[end - 1] != 0 {
            return Err(Error::Invalid("BSON document is not terminated"));
        }
        let index = b.open(depth, key, !array)?;
        let (mut at, mut count) = (at + 4, 0);
        while at < end - 1 {
            let kind = bytes[at];
            let (name, next) = cstring(bytes, at + 1)?;
            let name = b.text(name);
            at = element(b, bytes, kind, next, depth + 1, (!array).then_some(name))?;
            count += 1;
        }
        if at != end - 1 {
            return Err(Error::Invalid("BSON element overruns its document"));
        }
        b.close(index, count);
        Ok(end)
    }
    fn element(
        b: &mut Builder,
        bytes: &[u8],
        kind: u8,
        at: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        let string = |at| -> Result<(&str, usize), Error> {
            let len = usize::try_from(i32_at(bytes, at)?)
                .map_err(|_| Error::Invalid("Negative BSON length"))?;
            let raw = take(bytes, at + 4, len)?;
            let (&terminator, raw) = raw
                .split_last()
                .ok_or(Error::Invalid("Empty BSON string"))?;
            let text = std::str::from_utf8(raw).ok().filter(|_| terminator == 0);
            Ok((
                text.ok_or(Error::Invalid("Invalid BSON string"))?,
                at + 4 + len,
            ))
        };
        let (item, next) = match kind {
            0x01 => (real(f64::from_le_bytes(array_at(bytes, at)?)), at + 8),
            0x02 | 0x0d | 0x0e => {
                let (text, next) = string(at)?;
                (Item::Text(b.text(text)), next)
            }
            0x03 | 0x04 => return document(b, bytes, at, depth, key, kind == 0x04),
            0x05 => {
                let len = usize::try_from(i32_at(bytes, at)?)
                    .map_err(|_| Error::Invalid("Negative BSON length"))?;
                (bytes_item(take(bytes, at + 5, len)?), at + 5 + len)
            }
            0x06 | 0x0a | 0x7f | 0xff => (Item::Null, at),
            0x07 => (bytes_item(take(bytes, at, 12)?), at + 12),
            0x08 => (Item::Bool(take(bytes, at, 1)?[0] != 0), at + 1),
            0x09 | 0x11 | 0x12 => (number(i64::from_le_bytes(array_at(bytes, at)?)), at + 8),
            0x0b => {
                let (pattern, next) = cstring(bytes, at)?;
                let (_, next) = cstring(bytes, next)?;
                (Item::Text(b.text(pattern)), next)
            }
            0x10 => (number(i32_at(bytes, at)?), at + 4),
            0x13 => (bytes_item(take(bytes, at, 16)?), at + 16),
            _ => return Err(Error::Invalid("Unknown BSON element type")),
        };
        b.scalar(depth, key, item)?;
        Ok(next)
    }
}

mod msgpack {
    use super::{
        Builder, Error, Item, array_at, bytes_item, number, packed_collection, real, take, uint_be,
    };
    /// The item at `at`; returns where it ends.
    pub(super) fn item(
        b: &mut Builder,
        bytes: &[u8],
        at: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        b.enter(depth)?;
        let first = *bytes
            .get(at)
            .ok_or(Error::Invalid("The value ends inside an element"))?;
        let at = at + 1;
        let sized = |n| uint_be(bytes, at, n).map(|len| (len as usize, at + n));
        let (item, next) = match first {
            0x00..=0x7f => (number(first), at),
            0xe0..=0xff => (number(first as i8), at),
            0x80..=0x9f | 0xdc..=0xdf => {
                let (len, at, map) = if first < 0xa0 {
                    (usize::from(first & 0x0f), at, first < 0x90)
                } else {
                    let (len, at) = sized(if first & 1 == 0 { 2 } else { 4 })?;
                    (len, at, first >= 0xde)
                };
                if len > bytes.len() {
                    return Err(Error::Invalid("MessagePack length overruns the value"));
                }
                return packed_collection(b, bytes, at, Some(len), (depth, key), map, item);
            }
            0xa0..=0xbf => return text(b, bytes, at, usize::from(first & 0x1f), depth, key),
            0xd9..=0xdb => {
                let (len, at) = sized(1 << (first - 0xd9))?;
                return text(b, bytes, at, len, depth, key);
            }
            0xc4..=0xc6 => {
                let (len, at) = sized(1 << (first - 0xc4))?;
                (bytes_item(take(bytes, at, len)?), at + len)
            }
            0xc7..=0xc9 => {
                let (len, at) = sized(1 << (first - 0xc7))?;
                (bytes_item(take(bytes, at + 1, len)?), at + 1 + len)
            }
            0xd4..=0xd8 => {
                let len = 1 << (first - 0xd4);
                (bytes_item(take(bytes, at + 1, len)?), at + 1 + len)
            }
            0xc0 => (Item::Null, at),
            0xc2 | 0xc3 => (Item::Bool(first == 0xc3), at),
            0xca => (
                real(f64::from(f32::from_be_bytes(array_at(bytes, at)?))),
                at + 4,
            ),
            0xcb => (real(f64::from_be_bytes(array_at(bytes, at)?)), at + 8),
            0xcc..=0xcf => {
                let n = 1 << (first - 0xcc);
                (number(uint_be(bytes, at, n)?), at + n)
            }
            0xd0..=0xd3 => {
                let n = 1 << (first - 0xd0);
                let raw = uint_be(bytes, at, n)?;
                let shift = 64 - 8 * n as u32;
                (number(((raw << shift) as i64) >> shift), at + n)
            }
            _ => return Err(Error::Invalid("Reserved MessagePack type")),
        };
        b.scalar(depth, key, item)?;
        Ok(next)
    }
    fn text(
        b: &mut Builder,
        bytes: &[u8],
        at: usize,
        len: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        let raw = std::str::from_utf8(take(bytes, at, len)?)
            .map_err(|_| Error::Invalid("MessagePack string is not UTF-8"))?;
        let text = b.text(raw);
        b.scalar(depth, key, Item::Text(text))?;
        Ok(at + len)
    }
}

mod cbor {
    use super::{Builder, Error, Item, bytes_item, number, packed_collection, real, take, uint_be};

    /// Where the item starts, after a self-describe tag.
    pub(super) fn start(bytes: &[u8]) -> usize {
        if bytes.starts_with(&[0xd9, 0xd9, 0xf7]) {
            3
        } else {
            0
        }
    }
    /// The argument of a head: its value, and where the head ends; `None` for indefinite.
    fn argument(bytes: &[u8], at: usize, info: u8) -> Result<(Option<u64>, usize), Error> {
        let n = match info {
            0..=23 => return Ok((Some(u64::from(info)), at)),
            24..=27 => 1usize << (info - 24),
            31 => return Ok((None, at)),
            _ => return Err(Error::Invalid("Reserved CBOR argument")),
        };
        Ok((Some(uint_be(bytes, at, n)?), at + n))
    }
    pub(super) fn item(
        b: &mut Builder,
        bytes: &[u8],
        at: usize,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        b.enter(depth)?;
        // A tag only marks the item after it: skip tags without recursing.
        let mut at = at;
        let (major, info, arg) = loop {
            let first = *bytes
                .get(at)
                .ok_or(Error::Invalid("The value ends inside an element"))?;
            let (major, info) = (first >> 5, first & 0x1f);
            let (arg, next) = argument(bytes, at + 1, info)?;
            at = next;
            if b.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if !(major == 6 && arg.is_some()) {
                break (major, info, arg);
            }
        };
        let (item, next) = match (major, arg) {
            (0, Some(n)) => (number(n), at),
            (1, Some(n)) => (number(-1 - i128::from(n)), at),
            (2 | 3, Some(len)) => {
                let raw = take(
                    bytes,
                    at,
                    usize::try_from(len).map_err(|_| Error::Invalid("CBOR length"))?,
                )?;
                let item = if major == 2 {
                    bytes_item(raw)
                } else {
                    Item::Text(
                        b.text(
                            std::str::from_utf8(raw)
                                .map_err(|_| Error::Invalid("CBOR text is not UTF-8"))?,
                        ),
                    )
                };
                (item, at + raw.len())
            }
            (4 | 5, len) => {
                if len.is_some_and(|len| len > bytes.len() as u64) {
                    return Err(Error::Invalid("CBOR length overruns the value"));
                }
                let len = len.map(|len| len as usize);
                return packed_collection(b, bytes, at, len, (depth, key), major == 5, item);
            }
            (7, _) => (simple(info, arg)?, at),
            _ => return Err(Error::Invalid("Indefinite CBOR strings are not read")),
        };
        b.scalar(depth, key, item)?;
        Ok(next)
    }
    fn simple(info: u8, arg: Option<u64>) -> Result<Item, Error> {
        Ok(match (info, arg) {
            (20 | 21, _) => Item::Bool(info == 21),
            (22 | 23, _) => Item::Null,
            (25, Some(half)) => real(half_float(half as u16)),
            (26, Some(bits)) => real(f64::from(f32::from_bits(bits as u32))),
            (27, Some(bits)) => real(f64::from_bits(bits)),
            (0..=24, Some(n)) => number(n),
            _ => return Err(Error::Invalid("Unexpected CBOR break")),
        })
    }
    fn half_float(bits: u16) -> f64 {
        let (sign, exponent, fraction) = (bits >> 15, (bits >> 10) & 0x1f, bits & 0x3ff);
        let magnitude = match exponent {
            0 => f64::from(fraction) * 2f64.powi(-24),
            31 => {
                if fraction == 0 {
                    f64::INFINITY
                } else {
                    f64::NAN
                }
            }
            _ => (1.0 + f64::from(fraction) / 1024.0) * 2f64.powi(i32::from(exponent) - 15),
        };
        if sign == 1 { -magnitude } else { magnitude }
    }
}

mod protobuf {
    use super::{Builder, Error, Item, array_at, bytes_item, number, take};

    fn varint(bytes: &[u8], at: usize) -> Result<(u64, usize), Error> {
        let (value, len) = bytes
            .get(at..)
            .and_then(crate::bytes::leb128)
            .ok_or(Error::Invalid("Invalid protobuf varint"))?;
        Ok((value, at + len))
    }
    /// A message filling `bytes` from `at`: fields as members named by number.
    pub(super) fn message(
        b: &mut Builder,
        bytes: &[u8],
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        fields(bytes)?;
        let index = b.open(depth, key, true)?;
        let (mut at, mut count) = (0, 0);
        while at < bytes.len() {
            let (tag, next) = varint(bytes, at)?;
            let name = Some((tag >> 3).to_string());
            at = field(b, bytes, next, tag, depth + 1, name)?;
            count += 1;
        }
        b.close(index, count);
        Ok(at)
    }
    /// Whether `bytes` are a whole message: known wire types, field numbers from 1.
    fn fields(bytes: &[u8]) -> Result<usize, Error> {
        let (mut at, mut count) = (0, 0);
        while at < bytes.len() {
            let (tag, next) = varint(bytes, at)?;
            if tag >> 3 == 0 || tag >> 3 > 536_870_911 {
                return Err(Error::Invalid("Invalid protobuf field number"));
            }
            at = match tag & 7 {
                0 => varint(bytes, next)?.1,
                1 => next + take(bytes, next, 8)?.len(),
                2 => {
                    let (len, at) = varint(bytes, next)?;
                    at + take(
                        bytes,
                        at,
                        usize::try_from(len).map_err(|_| Error::Invalid("protobuf length"))?,
                    )?
                    .len()
                }
                5 => next + take(bytes, next, 4)?.len(),
                _ => return Err(Error::Invalid("Unsupported protobuf wire type")),
            };
            count += 1;
        }
        Ok(count)
    }
    fn field(
        b: &mut Builder,
        bytes: &[u8],
        at: usize,
        tag: u64,
        depth: usize,
        key: Option<String>,
    ) -> Result<usize, Error> {
        let (item, next) = match tag & 7 {
            0 => {
                let (value, next) = varint(bytes, at)?;
                (number(value), next)
            }
            1 => (number(u64::from_le_bytes(array_at(bytes, at)?)), at + 8),
            5 => (number(u32::from_le_bytes(array_at(bytes, at)?)), at + 4),
            _ => {
                let (len, at) = varint(bytes, at)?;
                let raw = take(bytes, at, len as usize)?;
                // A nested message when it reads as one, else text, else bytes.
                if !raw.is_empty() && fields(raw).is_ok() && depth < 64 {
                    message(b, raw, depth, key)?;
                    return Ok(at + raw.len());
                }
                let item = match std::str::from_utf8(raw) {
                    Ok(text) => Item::Text(b.text(text)),
                    Err(_) => bytes_item(raw),
                };
                (item, at + raw.len())
            }
        };
        b.scalar(depth, key, item)?;
        Ok(next)
    }
}

mod plist {
    use super::{Builder, Error, Item, array_at, bytes_item, number, real, take, uint_be};

    /// A binary plist: its bytes and the trailer's sizes and positions.
    struct Plist<'a> {
        bytes: &'a [u8],
        offset_size: usize,
        ref_size: usize,
        objects: usize,
        table: &'a [u8],
    }
    /// Where an object goes in the tree.
    struct Place {
        depth: usize,
        key: Option<String>,
    }
    fn uint(bytes: &[u8], at: usize, n: usize) -> Result<usize, Error> {
        usize::try_from(uint_be(bytes, at, n)?)
            .map_err(|_| Error::Invalid("plist number too large"))
    }
    pub(super) fn read(b: &mut Builder, bytes: &[u8]) -> Result<usize, Error> {
        if !bytes.starts_with(b"bplist00") || bytes.len() < 40 {
            return Err(Error::Invalid("Not a binary plist"));
        }
        let (body, trailer) = bytes.split_at(bytes.len() - 32);
        let offset_size = usize::from(trailer[6]);
        let ref_size = usize::from(trailer[7]);
        let objects = uint(trailer, 8, 8)?;
        let top = uint(trailer, 16, 8)?;
        let sizes = 1..=8;
        if !sizes.contains(&offset_size) || !sizes.contains(&ref_size) || top >= objects {
            return Err(Error::Invalid("Invalid plist trailer"));
        }
        let (data, table) = body
            .split_at_checked(uint(trailer, 24, 8)?)
            .ok_or(Error::Invalid("Invalid plist offset table"))?;
        if table.len() / offset_size < objects {
            return Err(Error::Invalid("Truncated plist offset table"));
        }
        let plist = Plist {
            bytes: data,
            offset_size,
            ref_size,
            objects,
            table,
        };
        plist.object(
            b,
            top,
            Place {
                depth: 0,
                key: None,
            },
        )?;
        Ok(bytes.len())
    }
    impl Plist<'_> {
        fn offset(&self, index: usize) -> Result<usize, Error> {
            if index >= self.objects {
                return Err(Error::Invalid("plist reference out of range"));
            }
            uint(self.table, index * self.offset_size, self.offset_size)
        }
        /// The length in a marker's low bits, or the integer object after it.
        fn length(&self, at: usize, low: u8) -> Result<(usize, usize), Error> {
            if low != 0xf {
                return Ok((usize::from(low), at + 1));
            }
            let marker = *self
                .bytes
                .get(at + 1)
                .ok_or(Error::Invalid("Truncated plist length"))?;
            let n = 1usize << (marker & 0xf);
            Ok((uint(self.bytes, at + 2, n)?, at + 2 + n))
        }
        fn object(&self, b: &mut Builder, index: usize, place: Place) -> Result<(), Error> {
            let at = self.offset(index)?;
            let bytes = self.bytes;
            let marker = *bytes
                .get(at)
                .ok_or(Error::Invalid("plist offset out of range"))?;
            let low = marker & 0xf;
            let item = match marker >> 4 {
                0x0 => match low {
                    0x8 | 0x9 => Item::Bool(low == 0x9),
                    _ => Item::Null,
                },
                0x1 => number(uint(bytes, at + 1, (1usize << low).min(8))? as i64),
                0x2 if low == 2 => real(f64::from(f32::from_be_bytes(array_at(bytes, at + 1)?))),
                0x2 | 0x3 => real(f64::from_be_bytes(array_at(bytes, at + 1)?)),
                0x4 => {
                    let (len, start) = self.length(at, low)?;
                    bytes_item(take(bytes, start, len)?)
                }
                0x5 | 0x6 => Item::Text(b.text(&self.string(at, marker)?)),
                0x8 => number(uint(bytes, at + 1, usize::from(low) + 1)?),
                0xa | 0xd => return self.collection(b, at, place, marker >> 4 == 0xd),
                _ => return Err(Error::Invalid("Unsupported plist object")),
            };
            b.scalar(place.depth, place.key, item)
        }
        /// The string object `index`, as a dictionary key.
        fn key(&self, index: usize) -> Result<String, Error> {
            let at = self.offset(index)?;
            let marker = *self
                .bytes
                .get(at)
                .ok_or(Error::Invalid("plist offset out of range"))?;
            if !matches!(marker >> 4, 0x5 | 0x6) {
                return Err(Error::Invalid("plist dictionary key is not a string"));
            }
            self.string(at, marker)
        }
        /// An ASCII or UTF-16 string object.
        fn string(&self, at: usize, marker: u8) -> Result<String, Error> {
            let (len, start) = self.length(at, marker & 0xf)?;
            if marker >> 4 == 0x5 {
                return Ok(String::from_utf8_lossy(take(self.bytes, start, len)?).into_owned());
            }
            let units = take(
                self.bytes,
                start,
                len.checked_mul(2).ok_or(Error::Invalid("plist length"))?,
            )?;
            Ok(char::decode_utf16(
                units
                    .chunks_exact(2)
                    .map(|u| u16::from_be_bytes([u[0], u[1]])),
            )
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect())
        }
        fn collection(
            &self,
            b: &mut Builder,
            at: usize,
            place: Place,
            dict: bool,
        ) -> Result<(), Error> {
            let (len, start) = self.length(at, self.bytes[at] & 0xf)?;
            if len > self.objects {
                return Err(Error::Invalid("plist collection larger than the plist"));
            }
            let depth = place.depth;
            let index = b.open(depth, place.key, dict)?;
            if index.is_none() {
                return Ok(());
            }
            let reference = |i: usize| uint(self.bytes, start + i * self.ref_size, self.ref_size);
            for i in 0..len {
                // Dictionary keys are strings, so a key never recurses.
                let key = if dict {
                    Some(b.text(&self.key(reference(i)?)?))
                } else {
                    None
                };
                let value = reference(if dict { len + i } else { i })?;
                self.object(
                    b,
                    value,
                    Place {
                        depth: depth + 1,
                        key,
                    },
                )?;
            }
            b.close(index, len);
            Ok(())
        }
    }
}

/// A map key read as its own small tree, as the body of a JSON string.
fn key_text(mut tree: Tree) -> String {
    match tree.nodes.first_mut().map(|n| &mut n.item) {
        Some(Item::Text(text)) => std::mem::take(text),
        _ => escape(&tree.json()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Format, decode, probe, signature};
    use crate::tree::Limits;
    use std::sync::atomic::AtomicBool;

    // The same value in each format. The binary plist was encoded with Python's plistlib.
    const SAMPLES: [(Format, &[u8]); 4] = [
        (
            Format::MessagePack,
            b"\x84\xa3job\xaaexport-812\xa4rows\xcd\xbcU\xa2ok\xc3\xa4tags\x92\xa1\
          a\xc0",
        ),
        (
            Format::Cbor,
            b"\xd9\xd9\xf7\xa4cjobjexport-812drows\x19\xbcUbok\xf5dtags\x82aa\xf6",
        ),
        (
            Format::Bson,
            b"?\x00\x00\x00\x02job\x00\x0b\x00\x00\x00export-812\x00\x10rows\x00U\
          \xbc\x00\x00\x08ok\x00\x01\x04tags\x00\x11\x00\x00\x00\x020\x00\x02\
          \x00\x00\x00a\x00\x0a1\x00\x00\x00",
        ),
        (
            Format::Plist,
            b"bplist00\xd4\x01\x02\x03\x04\x05\x06\x07\x08SjobTrowsRokTtagsZexport\
          -812\x11\xbcU\x09\xa2\x09\x0aQa\x00\x08\x11\x15\x1a\x1d\x22-0146\x00\
          \x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x0b\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x007",
        ),
    ];
    const WIDE_PLIST: &[u8] = b"bplist00\xd3\x01\x02\x03\x04\x05\x06a\x03\xbbUfloatTdatao\x10\x1f\
          \x00a\x00 \x00l\x00o\x00n\x00g\x00 \x00s\x00t\x00r\x00i\x00n\x00g\
          \x00 \x00w\x00i\x00t\x00h\x00 \x001\x006\x00b\x00i\x00t\x00 \x03\xbb\
          \x00 \x00t\x00e\x00x\x00t#?\xf8\x00\x00\x00\x00\x00\x00Dblob\x08\x0f\
          \x12\x18\x1d^g\x00\x00\x00\x00\x00\x00\x01\x01\x00\x00\x00\x00\x00\
          \x00\x00\x07\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00l";
    const PROTO: &[u8] = b"\x08\x96\x01\x12\x07testing";

    #[test]
    fn each_format_reads_into_the_same_tree() {
        let cancel = AtomicBool::new(false);
        for (format, bytes) in SAMPLES {
            let tree = decode(format, bytes, &Limits::default(), &cancel)
                .unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
            assert_eq!(
                tree.json(),
                r#"{"job":"export-812","rows":48213,"ok":true,"tags":["a",null]}"#,
                "{format:?}"
            );
            let mut trailing = bytes.to_vec();
            trailing.push(0);
            assert!(
                decode(format, &trailing, &Limits::default(), &cancel).is_err(),
                "{format:?}"
            );
            assert!(
                decode(
                    format,
                    &bytes[..bytes.len() - 1],
                    &Limits::default(),
                    &cancel
                )
                .is_err(),
                "{format:?}"
            );
        }
        for (format, bytes) in &SAMPLES[1..] {
            assert_eq!(signature(bytes, bytes.len() as u64), Some(*format));
        }
        assert_eq!(signature(b"plain text here", 15), None);

        let limits = Limits::default();
        let read = |format, bytes| decode(format, bytes, &limits, &cancel).unwrap();
        assert_eq!(
            read(Format::Protobuf, PROTO).json(),
            r#"{"1":150,"2":"testing"}"#
        );
        // An indefinite CBOR map and array, with a numeric key.
        assert_eq!(
            read(Format::Cbor, b"\xbf\x01\x9f\x02\x03\xff\xff").json(),
            r#"{"1":[2,3]}"#
        );
        assert_eq!(
            read(Format::MessagePack, b"\xde\x00\x01\xa1a\xdc\x00\x01\x01").json(),
            r#"{"a":[1]}"#
        );
        let plist = read(Format::Plist, WIDE_PLIST);
        assert!(plist.complete);
        assert!(
            plist
                .json()
                .contains(r#""λ":"a long string with 16bit λ text","float":1.5"#)
        );
        assert_eq!(
            plist.nodes.last().unwrap().item,
            super::Item::Bytes("626c6f62".into(), 4)
        );

        assert_eq!(probe(PROTO, &cancel), Some(Format::Protobuf));
        assert_eq!(probe(SAMPLES[0].1, &cancel), Some(Format::MessagePack));
        assert_eq!(probe(b"\x01", &cancel), None);
    }

    /// Deeply nested values are refused, not read until the stack runs out,
    /// whether they nest as items, as map keys or as CBOR tags.
    #[test]
    fn deep_nesting_is_refused_without_recursing_to_the_end() {
        let cancel = AtomicBool::new(false);
        let limits = Limits::default();
        let deep = 200_000;
        // A one-element array inside another, `deep` times, around a 0.
        let mut arrays = vec![0x91; deep];
        arrays.push(0x00);
        assert!(decode(Format::MessagePack, &arrays, &limits, &cancel).is_err());
        let mut cbor_arrays = vec![0x81; deep];
        cbor_arrays.push(0x00);
        assert!(decode(Format::Cbor, &cbor_arrays, &limits, &cancel).is_err());
        // A one-entry map whose key is itself a one-entry map, `deep` times.
        let mut keys = vec![0x81; deep];
        keys.extend([0x00, 0x00]);
        assert!(decode(Format::MessagePack, &keys, &limits, &cancel).is_err());
        let mut cbor_keys = vec![0xa1; deep];
        cbor_keys.extend([0x00, 0x00]);
        assert!(decode(Format::Cbor, &cbor_keys, &limits, &cancel).is_err());
        assert_eq!(probe(&arrays, &cancel), None);
        assert_eq!(probe(&keys, &cancel), None);
        // Tags only mark the item after them, so any number of them is read.
        let mut tags = vec![0xc6; deep];
        tags.push(0x01);
        let tree = decode(Format::Cbor, &tags, &limits, &cancel).unwrap();
        assert_eq!(tree.json(), "1");
        // BSON documents nested in documents.
        let mut nested = Vec::new();
        let levels = 20_000i32;
        for level in (1..=levels).rev() {
            nested.extend((5 + level * 8).to_le_bytes());
            nested.extend(b"\x03a\0");
        }
        nested.extend(b"\x05\0\0\0\0");
        nested.resize(nested.len() + levels as usize, 0);
        assert!(decode(Format::Bson, &nested, &limits, &cancel).is_err());
        // Nesting within the limit still reads.
        let mut shallow = vec![0x91; 100];
        shallow.push(0x00);
        let tree = decode(Format::MessagePack, &shallow, &limits, &cancel).unwrap();
        assert!(!tree.complete);
    }

    #[test]
    fn random_input_never_panics_and_stays_bounded() {
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            nodes: 200,
            ..Limits::default()
        };
        let mut state = 0x1234_5678_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for (format, sample) in SAMPLES
            .into_iter()
            .chain([(Format::Protobuf, PROTO), (Format::Plist, WIDE_PLIST)])
        {
            for _ in 0..3000 {
                let mut bytes = sample.to_vec();
                for _ in 0..=next() % 4 {
                    let at = (next() as usize) % bytes.len();
                    bytes[at] = next() as u8;
                }
                bytes.extend((0..next() % 40).map(|_| next() as u8));
                if let Ok(tree) = decode(format, &bytes, &limits, &cancel) {
                    assert!(tree.nodes.len() <= 200);
                }
            }
        }
    }
}
