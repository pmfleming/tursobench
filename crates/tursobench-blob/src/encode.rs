//! Bytes written out as text: the formats a value or selection can be copied as.

/// A text form of bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Pairs of hexadecimal digits separated by spaces.
    Hex,
    Base64,
    /// A SQL blob literal, `X'00FF'`.
    Sql,
    /// The bytes as UTF-8 text, invalid sequences replaced.
    Text,
    /// A C array initialiser.
    CArray,
}
impl Format {
    pub const ALL: [Self; 5] = [Self::Hex, Self::Base64, Self::Sql, Self::Text, Self::CArray];
    pub fn name(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Base64 => "Base64",
            Self::Sql => "SQL literal",
            Self::Text => "Text",
            Self::CArray => "C array",
        }
    }
    /// How long the text of `len` bytes would be, without making it.
    pub fn length(self, len: usize) -> usize {
        match self {
            Self::Hex => (len * 3).saturating_sub(1),
            Self::Base64 => len.div_ceil(3) * 4,
            Self::Sql => len * 2 + 3,
            Self::Text => len,
            Self::CArray => (len * 6).max(2),
        }
    }
    pub fn write(self, bytes: &[u8]) -> String {
        match self {
            Self::Hex => hex(bytes, " "),
            Self::Base64 => base64(bytes),
            Self::Sql => format!("X'{}'", hex(bytes, "").to_uppercase()),
            Self::Text => String::from_utf8_lossy(bytes).into_owned(),
            Self::CArray => {
                let items: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
                format!("{{{}}}", items.join(", "))
            }
        }
    }
}

/// Each byte as two lower-case hexadecimal digits, joined by `separator`.
pub fn hex(bytes: &[u8], separator: &str) -> String {
    let mut out = String::with_capacity(bytes.len() * (2 + separator.len()));
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push_str(separator);
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Standard Base64 with padding (RFC 4648).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= group.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Bytes typed as hexadecimal pairs, with optional spaces and `0x` prefix;
/// `None` unless every character is part of a pair.
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let digits: String = text
        .split_whitespace()
        .map(|word| word.strip_prefix("0x").unwrap_or(word))
        .collect();
    // Only digits: `from_str_radix` would also take a sign, as in "+f".
    if digits.is_empty()
        || !digits.len().is_multiple_of(2)
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Format, base64, parse_hex};

    #[test]
    fn every_format_writes_the_bytes_and_predicts_its_length() {
        let bytes = [0x00, 0xff, 0x7a, b'h', b'i'];
        let cases = [
            (Format::Hex, "00 ff 7a 68 69"),
            (Format::Base64, "AP96aGk="),
            (Format::Sql, "X'00FF7A6869'"),
            (Format::Text, "\0\u{FFFD}zhi"),
            (Format::CArray, "{0x00, 0xff, 0x7a, 0x68, 0x69}"),
        ];
        for (format, expected) in cases {
            let text = format.write(&bytes);
            assert_eq!(text, expected, "{format:?}");
            if format != Format::Text {
                assert_eq!(format.length(bytes.len()), text.len(), "{format:?}");
            }
        }
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        assert_eq!(Format::Sql.write(&[]), "X''");
    }

    #[test]
    fn hex_input_needs_whole_pairs() {
        assert_eq!(parse_hex("66 6a 73"), Some(vec![0x66, 0x6a, 0x73]));
        assert_eq!(parse_hex("0x66 0X6A"), None);
        assert_eq!(parse_hex("666A"), Some(vec![0x66, 0x6a]));
        assert_eq!(parse_hex("6"), None);
        assert_eq!(parse_hex("export"), None);
        assert_eq!(parse_hex("  "), None);
        // A sign is not a digit.
        assert_eq!(parse_hex("+f"), None);
        assert_eq!(parse_hex("-1"), None);
    }
}
