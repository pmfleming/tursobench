//! What a value's bytes are, judged from at most its first [`HEAD`] bytes and
//! its length, so a 3-byte and a 60 MiB value cost the same.
use crate::{HEAD, text::Encoding};

/// An image format the viewer can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
}
impl ImageFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
        }
    }
}

/// A compression format whose contents can be inflated and detected again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Gzip,
    Zlib,
    Zstd,
}
impl Compression {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zlib => "zlib",
            Self::Zstd => "zstd",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gzip => "gz",
            Self::Zlib => "zz",
            Self::Zstd => "zst",
        }
    }
}

/// A format recognised by its signature and only named, not decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: &'static str,
    pub extension: &'static str,
}

/// What the bytes of a value are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Empty,
    /// A decodable image, with its pixel size when the header holds it.
    Image {
        format: ImageFormat,
        size: Option<(u32, u32)>,
    },
    Text(Encoding),
    /// Sixteen bytes with RFC UUID version and variant bits.
    Uuid {
        version: u8,
    },
    /// 16, 20, 32 or 64 bytes that are not text: a hash or key, never named.
    Digest,
    Compressed(Compression),
    Named(Named),
    Unknown,
}
impl Kind {
    /// The file extension to suggest when saving the value.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Image { format, .. } => format.extension(),
            Self::Text(_) => "txt",
            Self::Compressed(compression) => compression.extension(),
            Self::Named(named) => named.extension,
            Self::Empty | Self::Uuid { .. } | Self::Digest | Self::Unknown => "bin",
        }
    }
}

/// What a value `len` bytes long is, judged from `head`, its first bytes;
/// bytes after the first [`HEAD`] are ignored.
pub fn sniff(head: &[u8], len: u64) -> Kind {
    let head = &head[..head.len().min(HEAD)];
    if len == 0 {
        return Kind::Empty;
    }
    if let Some(kind) = image(head) {
        return kind;
    }
    if let Some(compression) = compression(head) {
        return Kind::Compressed(compression);
    }
    if let Some(named) = named(head) {
        return Kind::Named(named);
    }
    if let Some(encoding) = crate::text::detect(head, len) {
        return Kind::Text(encoding);
    }
    // Two bytes of zlib header are weaker evidence than text.
    if zlib(head) {
        return Kind::Compressed(Compression::Zlib);
    }
    if len == 16
        && let Some(version) = crate::uuid::version(head)
    {
        return Kind::Uuid { version };
    }
    if matches!(len, 16 | 20 | 32 | 64) {
        return Kind::Digest;
    }
    Kind::Unknown
}

fn u16_le(head: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(
        head.get(at..at + 2)?.try_into().ok()?,
    )))
}
fn u24_le(head: &[u8], at: usize) -> Option<u32> {
    let b = head.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}
fn u32_be(head: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(head.get(at..at + 4)?.try_into().ok()?))
}
fn i32_le(head: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(head.get(at..at + 4)?.try_into().ok()?))
}

/// A decodable image by its signature, with the size its header records.
fn image(head: &[u8]) -> Option<Kind> {
    let (format, size) = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        let size = (head.get(12..16) == Some(b"IHDR"))
            .then(|| Some((u32_be(head, 16)?, u32_be(head, 20)?)))
            .flatten();
        (ImageFormat::Png, size)
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // The frame header usually follows metadata beyond the head.
        (ImageFormat::Jpeg, None)
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        (ImageFormat::Gif, u16_le(head, 6).zip(u16_le(head, 8)))
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        (ImageFormat::WebP, webp_size(head))
    } else if head.starts_with(b"BM") && head.len() >= 18 {
        (ImageFormat::Bmp, Some(bmp_size(head)?))
    } else if head.starts_with(&[0, 0, 1, 0]) && u16_le(head, 4).is_some_and(|n| n > 0) {
        let side = |b: Option<&u8>| b.map(|&b| if b == 0 { 256 } else { u32::from(b) });
        (ImageFormat::Ico, side(head.get(6)).zip(side(head.get(7))))
    } else {
        return None;
    };
    // A header that claims an empty image is not believed.
    let size = size.filter(|&(w, h)| w > 0 && h > 0);
    Some(Kind::Image { format, size })
}

/// The canvas of a WebP from its first chunk: extended, lossless or lossy.
fn webp_size(head: &[u8]) -> Option<(u32, u32)> {
    match head.get(12..16)? {
        b"VP8X" => Some((u24_le(head, 24)? + 1, u24_le(head, 27)? + 1)),
        b"VP8L" if head.get(20) == Some(&0x2F) => {
            let bits = u32::from_le_bytes(head.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8 " if head.get(23..26) == Some(&[0x9D, 0x01, 0x2A]) => {
            Some((u16_le(head, 26)? & 0x3FFF, u16_le(head, 28)? & 0x3FFF))
        }
        _ => None,
    }
}

/// A BMP's size from its DIB header; `None` when the header is not one.
fn bmp_size(head: &[u8]) -> Option<(u32, u32)> {
    let header = u32::from_le_bytes(head.get(14..18)?.try_into().ok()?);
    match header {
        12 => Some((u16_le(head, 18)?, u16_le(head, 20)?)),
        40 | 52 | 56 | 64 | 108 | 124 => {
            let (w, h) = (i32_le(head, 18)?, i32_le(head, 22)?);
            Some((w.unsigned_abs(), h.unsigned_abs())).filter(|_| w > 0)
        }
        // Not a DIB header: another file that starts with "BM".
        _ => None,
    }
}

fn compression(head: &[u8]) -> Option<Compression> {
    match head {
        [0x1F, 0x8B, 0x08, ..] => Some(Compression::Gzip),
        [0x28, 0xB5, 0x2F, 0xFD, ..] => Some(Compression::Zstd),
        _ => None,
    }
}

/// Deflate with a window of at most 32 KiB, no preset dictionary, and a valid check.
fn zlib(head: &[u8]) -> bool {
    let [cmf, flg, ..] = *head else {
        return false;
    };
    cmf & 0x0F == 8
        && cmf >> 4 <= 7
        && flg & 0x20 == 0
        && (u16::from(cmf) << 8 | u16::from(flg)).is_multiple_of(31)
}

/// Signatures of formats that are only named: offset, bytes, name, extension.
const SIGNATURES: &[(usize, &[u8], &str, &str)] = &[
    (0, b"%PDF-", "PDF", "pdf"),
    (0, b"SQLite format 3\0", "SQLite", "sqlite"),
    (0, b"PK\x03\x04", "ZIP", "zip"),
    (0, b"PK\x05\x06", "ZIP", "zip"),
    (0, b"7z\xBC\xAF\x27\x1C", "7z", "7z"),
    (0, b"Rar!\x1A\x07", "RAR", "rar"),
    (0, b"\xFD7zXZ\0", "xz", "xz"),
    (0, b"BZh", "bzip2", "bz2"),
    (0, b"\x04\x22\x4D\x18", "LZ4", "lz4"),
    (0, b"\xFF\x06\0\0sNaPpY", "Snappy", "sz"),
    (0, b"ID3", "MP3", "mp3"),
    (0, b"fLaC", "FLAC", "flac"),
    (0, b"OggS", "Ogg", "ogg"),
    (0, b"MThd", "MIDI", "mid"),
    (0, b"\x1A\x45\xDF\xA3", "Matroska", "mkv"),
    (0, b"FLV\x01", "FLV", "flv"),
    (0, b"\0\x01\0\0\0", "TrueType", "ttf"),
    (0, b"OTTO", "OpenType", "otf"),
    (0, b"ttcf", "Font collection", "ttc"),
    (0, b"wOFF", "WOFF", "woff"),
    (0, b"wOF2", "WOFF2", "woff2"),
    (0, b"\0asm", "WebAssembly", "wasm"),
    (0, b"\x7FELF", "ELF", "elf"),
    (0, b"\xCF\xFA\xED\xFE", "Mach-O", "macho"),
    (0, b"\xCE\xFA\xED\xFE", "Mach-O", "macho"),
    (0, b"\xCA\xFE\xBA\xBE", "Java class", "class"),
    (0, b"dex\n", "Dalvik", "dex"),
    (0, b"8BPS", "Photoshop", "psd"),
    (0, b"II*\0", "TIFF", "tif"),
    (0, b"MM\0*", "TIFF", "tif"),
    (0, b"\0\0\x02\0", "Cursor", "cur"),
    (0, b"icns", "Apple icon", "icns"),
    (0, b"\xFF\x0A", "JPEG XL", "jxl"),
    (0, b"\0\0\0\x0CJXL ", "JPEG XL", "jxl"),
    (0, b"PAR1", "Parquet", "parquet"),
    (0, b"ARROW1", "Arrow", "arrow"),
    (0, b"Obj\x01", "Avro", "avro"),
    (0, b"PACK", "Git pack", "pack"),
    (0, b"\x1BLua", "Lua bytecode", "luac"),
    (0, b"%!PS", "PostScript", "ps"),
    (0, b"AT&TFORM", "DjVu", "djvu"),
];

/// RIFF containers name their contents a few bytes in: an outer signature at
/// the start, then an inner one at an offset.
type Container = (&'static [u8], usize, &'static [u8]);
const CONTAINERS: &[(Container, &str, &str)] = &[
    ((b"RIFF", 8, b"WAVE"), "WAV", "wav"),
    ((b"RIFF", 8, b"AVI "), "AVI", "avi"),
    ((b"FORM", 8, b"AIFF"), "AIFF", "aiff"),
];

fn named(head: &[u8]) -> Option<Named> {
    let at = |offset: usize, bytes: &[u8]| head.get(offset..offset + bytes.len()) == Some(bytes);
    let found = SIGNATURES
        .iter()
        .find(|(offset, bytes, ..)| at(*offset, bytes))
        .map(|&(_, _, name, extension)| (name, extension))
        .or_else(|| {
            CONTAINERS
                .iter()
                .find(|((outer, offset, inner), ..)| at(0, outer) && at(*offset, inner))
                .map(|&(_, name, extension)| (name, extension))
        })
        .or_else(|| iso_media(head))
        .or_else(|| mp3_frame(head))?;
    Some(Named {
        name: found.0,
        extension: found.1,
    })
}

/// An ISO base media file (`ftyp` box) named by its major brand.
fn iso_media(head: &[u8]) -> Option<(&'static str, &'static str)> {
    if head.get(4..8) != Some(b"ftyp") {
        return None;
    }
    Some(match head.get(8..12)? {
        b"heic" | b"heix" | b"mif1" | b"msf1" => ("HEIF", "heic"),
        b"avif" | b"avis" => ("AVIF", "avif"),
        b"qt  " => ("QuickTime", "mov"),
        b"M4A " => ("MPEG-4 audio", "m4a"),
        b"3gp4" | b"3gp5" | b"3gp6" => ("3GPP", "3gp"),
        _ => ("MPEG-4", "mp4"),
    })
}

/// An MPEG audio frame header without an ID3 tag: sync bits, a layer and a bitrate.
fn mp3_frame(head: &[u8]) -> Option<(&'static str, &'static str)> {
    let [0xFF, b, c, ..] = *head else {
        return None;
    };
    let valid = b & 0xE0 == 0xE0 && b & 0x06 != 0 && c >> 4 != 0x0F && c >> 4 != 0;
    valid.then_some(("MP3", "mp3"))
}

#[cfg(test)]
mod tests {
    use super::{Compression, ImageFormat, Kind, sniff};
    use crate::text::Encoding;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut head = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        head.extend(width.to_be_bytes());
        head.extend(height.to_be_bytes());
        head.extend([8, 6, 0, 0, 0]);
        head
    }

    fn image(kind: Kind) -> Option<(ImageFormat, Option<(u32, u32)>)> {
        match kind {
            Kind::Image { format, size } => Some((format, size)),
            _ => None,
        }
    }

    #[test]
    fn image_signatures_report_their_format_and_size() {
        let mut bmp = b"BM\0\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0".to_vec();
        bmp.extend(3i32.to_le_bytes());
        bmp.extend((-2i32).to_le_bytes());
        let mut webp_x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp_x.extend([63, 0, 0, 31, 0, 0]);
        let mut webp_l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        webp_l.extend((9u32 | 4 << 14).to_le_bytes());
        let mut webp = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        webp.extend([10, 0, 20, 0]);
        type Case<'a> = (&'a [u8], ImageFormat, Option<(u32, u32)>);
        let cases: [Case; 9] = [
            (&png(64, 48), ImageFormat::Png, Some((64, 48))),
            (&png(0, 48), ImageFormat::Png, None),
            (b"\xFF\xD8\xFF\xE0\0\x10JFIF", ImageFormat::Jpeg, None),
            (b"GIF89a\x40\0\x20\0", ImageFormat::Gif, Some((64, 32))),
            (&bmp, ImageFormat::Bmp, Some((3, 2))),
            (b"\0\0\x01\0\x01\0\0\x10", ImageFormat::Ico, Some((256, 16))),
            (&webp_x, ImageFormat::WebP, Some((64, 32))),
            (&webp_l, ImageFormat::WebP, Some((10, 5))),
            (&webp, ImageFormat::WebP, Some((10, 20))),
        ];
        for (head, format, size) in cases {
            assert_eq!(image(sniff(head, 1000)), Some((format, size)), "{head:?}");
        }
        assert_eq!(sniff(&png(1, 1), 1000).extension(), "png");
        // A truncated header still names the format, without a size.
        assert_eq!(
            image(sniff(&png(64, 48)[..18], 18)),
            Some((ImageFormat::Png, None))
        );
        assert_eq!(
            image(sniff(b"GIF89a\x40", 7)),
            Some((ImageFormat::Gif, None))
        );
        // "BM" text and an ICO with no images are not images.
        assert_eq!(image(sniff(b"BM is not a bitmap header", 25)), None);
        assert_eq!(sniff(b"\0\0\x01\0\0\0", 6), Kind::Unknown);
    }

    #[test]
    fn compressed_and_named_signatures_are_recognised() {
        let cases: [(&[u8], Kind); 4] = [
            (
                b"\x1f\x8b\x08\0\0\0\0\0",
                Kind::Compressed(Compression::Gzip),
            ),
            (b"\x78\x9c\x4b\x4c", Kind::Compressed(Compression::Zlib)),
            (b"\x78\x01\x01\x02", Kind::Compressed(Compression::Zlib)),
            (b"\x28\xb5\x2f\xfd\x20", Kind::Compressed(Compression::Zstd)),
        ];
        for (head, kind) in cases {
            assert_eq!(sniff(head, 4096), kind, "{head:?}");
        }
        let names: [(&[u8], &str, &str); 12] = [
            (b"%PDF-1.7\n", "PDF", "pdf"),
            (b"SQLite format 3\0\x10\0", "SQLite", "sqlite"),
            (b"PK\x03\x04\x14\0", "ZIP", "zip"),
            (b"RIFF\0\0\0\0WAVEfmt ", "WAV", "wav"),
            (b"RIFF\0\0\0\0AVI LIST", "AVI", "avi"),
            (b"\0\0\0\x18ftypheic", "HEIF", "heic"),
            (b"\0\0\0\x18ftypisom", "MPEG-4", "mp4"),
            (b"\xFF\xFB\x90\x64", "MP3", "mp3"),
            (b"\0asm\x01\0\0\0", "WebAssembly", "wasm"),
            (b"\x7FELF\x02\x01", "ELF", "elf"),
            (b"wOF2\0\x01\0\0", "WOFF2", "woff2"),
            (b"OggS\0\x02", "Ogg", "ogg"),
        ];
        for (head, name, extension) in names {
            let kind = sniff(head, 4096);
            assert!(
                matches!(kind, Kind::Named(n) if n.name == name && n.extension == extension),
                "{head:?}: {kind:?}"
            );
        }
        // A zlib-like pair whose check fails is not zlib.
        assert_eq!(sniff(b"\x78\x9d\0\0\0\0\0", 7), Kind::Unknown);
    }

    #[test]
    fn text_uuid_digest_empty_and_unknown_by_length() {
        assert_eq!(sniff(b"", 0), Kind::Empty);
        assert_eq!(sniff(b"hello", 5), Kind::Text(Encoding::Utf8));
        // Only the first 64 bytes are read, whatever is passed.
        let mut long = b"x".repeat(64);
        long.extend([0, 0, 0]);
        assert_eq!(sniff(&long, 1 << 30), Kind::Text(Encoding::Utf8));
        let uuid = [
            0x3f, 0x2a, 0x9c, 0x1e, 0x7b, 0x40, 0x4d, 0x1a, 0x9e, 0x5c, 0x0a, 0x1b, 0x2c, 0x3d,
            0x4e, 0x5f,
        ];
        assert_eq!(sniff(&uuid, 16), Kind::Uuid { version: 4 });
        // The same bytes as part of a longer value are not a UUID.
        assert_eq!(sniff(&uuid, 17), Kind::Unknown);
        // Sixteen bytes of text stay text.
        assert_eq!(sniff(b"abcdefgh12345678", 16), Kind::Text(Encoding::Utf8));
        for len in [16, 20, 32, 64] {
            let bytes = vec![0u8; len];
            assert_eq!(sniff(&bytes, len as u64), Kind::Digest, "{len}");
        }
        assert_eq!(sniff(&[0, 255, 122], 3), Kind::Unknown);
        assert_eq!(sniff(&[0u8; 24], 24), Kind::Unknown);
        assert_eq!(Kind::Digest.extension(), "bin");
    }
}
