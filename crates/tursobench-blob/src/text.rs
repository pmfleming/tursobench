//! Text stored as bytes: UTF-8, or UTF-16 recognised by its byte-order mark
//! or by the zero bytes of mostly-ASCII text.

/// How the bytes of a text value are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}
impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
        }
    }
}

/// The encoding of `head`, the first bytes of a value `len` bytes long, when
/// it reads as text: no NUL characters and few other controls. A multi-byte
/// character cut by the end of `head` counts only when the value continues.
pub fn detect(head: &[u8], len: u64) -> Option<Encoding> {
    if head.is_empty() {
        return None;
    }
    let partial = (head.len() as u64) < len;
    if let Some(encoding) = utf16_encoding(head, len) {
        return readable(utf16_chars(head, encoding, partial)?).then_some(encoding);
    }
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // An incomplete final character, cut by the end of the head.
        Err(e) if partial && e.error_len().is_none() => {
            std::str::from_utf8(&head[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    readable(text.chars()).then_some(Encoding::Utf8)
}

/// Text decoded from `bytes`, replacing invalid sequences; a byte-order mark is dropped.
pub fn decode(bytes: &[u8], encoding: Encoding) -> String {
    match encoding {
        Encoding::Utf8 => {
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
            String::from_utf8_lossy(bytes).into_owned()
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mark = utf16_mark(bytes) == Some(encoding);
            let units = units(&bytes[if mark { 2 } else { 0 }..], encoding);
            char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect()
        }
    }
}

/// Whether characters read as text: none is NUL and at most one in sixteen
/// is another control character.
fn readable(chars: impl Iterator<Item = char>) -> bool {
    let (mut count, mut controls) = (0usize, 0usize);
    for c in chars {
        if c == '\0' {
            return false;
        }
        count += 1;
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c') {
            controls += 1;
        }
    }
    count > 0 && controls * 16 <= count
}

fn utf16_mark(head: &[u8]) -> Option<Encoding> {
    match head {
        [0xFF, 0xFE, ..] => Some(Encoding::Utf16Le),
        [0xFE, 0xFF, ..] => Some(Encoding::Utf16Be),
        _ => None,
    }
}

/// UTF-16 by its mark, or by every other byte being zero in at least four
/// code units, as ASCII-range text is.
fn utf16_encoding(head: &[u8], len: u64) -> Option<Encoding> {
    if !len.is_multiple_of(2) {
        return None;
    }
    if let Some(mark) = utf16_mark(head) {
        return Some(mark);
    }
    let pairs = head.chunks_exact(2);
    if pairs.len() < 4 {
        return None;
    }
    let all = |f: fn(&[u8]) -> bool| head.chunks_exact(2).all(f);
    if all(|p| p[1] == 0 && p[0] != 0) {
        Some(Encoding::Utf16Le)
    } else if all(|p| p[0] == 0 && p[1] != 0) {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

fn units(bytes: &[u8], encoding: Encoding) -> impl Iterator<Item = u16> + '_ {
    bytes.chunks_exact(2).map(move |p| match encoding {
        Encoding::Utf16Be => u16::from_be_bytes([p[0], p[1]]),
        _ => u16::from_le_bytes([p[0], p[1]]),
    })
}

/// The characters of a UTF-16 head; an unpaired surrogate fails unless it is
/// the last unit of a head the value continues past.
fn utf16_chars(head: &[u8], encoding: Encoding, partial: bool) -> Option<std::vec::IntoIter<char>> {
    let body = &head[if utf16_mark(head).is_some() { 2 } else { 0 }..];
    let count = body.len() / 2;
    let mut chars = Vec::with_capacity(count);
    let mut seen = 0;
    for unit in char::decode_utf16(units(body, encoding)) {
        match unit {
            Ok(c) => {
                seen += c.len_utf16();
                chars.push(c);
            }
            Err(_) if partial && seen + 1 == count => break,
            Err(_) => return None,
        }
    }
    Some(chars.into_iter())
}

#[cfg(test)]
mod tests {
    use super::{Encoding, decode, detect};

    fn utf16(text: &str, encoding: Encoding, mark: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let units = text.encode_utf16();
        if mark {
            out.extend(match encoding {
                Encoding::Utf16Be => [0xFE, 0xFF],
                _ => [0xFF, 0xFE],
            });
        }
        for unit in units {
            out.extend(match encoding {
                Encoding::Utf16Be => unit.to_be_bytes(),
                _ => unit.to_le_bytes(),
            });
        }
        out
    }

    #[test]
    fn utf8_and_utf16_text_is_recognised_and_decoded() {
        let plain = "row 1 λ; ' quote\nsecond line\t";
        assert_eq!(
            detect(plain.as_bytes(), plain.len() as u64),
            Some(Encoding::Utf8)
        );
        assert_eq!(decode(plain.as_bytes(), Encoding::Utf8), plain);
        for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
            for mark in [true, false] {
                let bytes = utf16("Straße λ text", encoding, mark);
                let detected = detect(&bytes, bytes.len() as u64);
                // Without a mark, the zero pattern needs ASCII-range text.
                if mark {
                    assert_eq!(detected, Some(encoding), "{encoding:?}");
                }
                assert_eq!(decode(&bytes, encoding), "Straße λ text");
            }
            let ascii = utf16("plain ascii", encoding, false);
            assert_eq!(detect(&ascii, ascii.len() as u64), Some(encoding));
        }
    }

    #[test]
    fn a_character_cut_by_the_head_counts_only_when_the_value_continues() {
        let text = "ααααα".as_bytes();
        let cut = &text[..5];
        assert_eq!(detect(cut, 100), Some(Encoding::Utf8));
        assert_eq!(detect(cut, 5), None);
        let wide = utf16("a😀", Encoding::Utf16Le, true);
        let cut = &wide[..6];
        assert_eq!(detect(cut, 100), Some(Encoding::Utf16Le));
        assert_eq!(detect(cut, 6), None);
    }

    #[test]
    fn binary_is_not_mistaken_for_text() {
        for bytes in [
            &[0u8, 255, 122][..],
            b"text\0\xff\r\n",
            b"undo\0\r\n",
            &[0x42, 0x01, 0x02, 0x03, 0x04, 0x05],
            &[1, 0, 2, 0, 3, 0, 4, 0],
            &[],
        ] {
            assert_eq!(detect(bytes, bytes.len() as u64), None, "{bytes:?}");
        }
        // Few controls are allowed; many are not.
        let mostly = "abcdefghijklmnop\x07".repeat(2);
        assert_eq!(detect(mostly.as_bytes(), 34), Some(Encoding::Utf8));
        let many = "ab\x07".repeat(8);
        assert_eq!(detect(many.as_bytes(), 24), None);
        // An odd length is never UTF-16.
        let odd = utf16("plain ascii", Encoding::Utf16Le, true);
        assert_eq!(detect(&odd, odd.len() as u64 + 1), None);
        assert_eq!(
            decode(&[0xFF, 0xFE, 0x00, 0xD8], Encoding::Utf16Le),
            "\u{FFFD}"
        );
    }
}
