//! The data inspector: the bytes at a cursor read as each common number type.
use crate::{Order, bytes::little_endian};

pub use crate::bytes::leb128;

/// One reading of the bytes at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Binary,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
    Leb128,
    Utf8,
    UnixSeconds,
}
impl Field {
    /// Every field, in the order the inspector lists them.
    pub const ALL: [Self; 14] = [
        Self::Binary,
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
        Self::U64,
        Self::I64,
        Self::F32,
        Self::F64,
        Self::Leb128,
        Self::Utf8,
        Self::UnixSeconds,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::U8 => "u8",
            Self::I8 => "i8",
            Self::U16 => "u16",
            Self::I16 => "i16",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::U64 => "u64",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Leb128 => "LEB128",
            Self::Utf8 => "UTF-8",
            Self::UnixSeconds => "time · s",
        }
    }
    /// How many bytes the reading takes, at most for variable-length fields.
    pub fn width(self) -> usize {
        match self {
            Self::Binary | Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 | Self::UnixSeconds => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
            Self::Leb128 => 10,
            Self::Utf8 => 4,
        }
    }
    /// The bytes at the start of `bytes` read as this field, or `None` when
    /// too few bytes remain.
    pub fn read(self, bytes: &[u8], order: Order) -> Option<String> {
        Some(match self {
            Self::Binary => format!("{:08b}", bytes.first()?),
            Self::U8 => group(u128::from(*bytes.first()?), false),
            Self::I8 => signed(i64::from(*bytes.first()? as i8)),
            Self::U16 => group(u128::from(u16::from_le_bytes(take(bytes, order)?)), false),
            Self::I16 => signed(i64::from(i16::from_le_bytes(take(bytes, order)?))),
            Self::U32 => group(u128::from(u32::from_le_bytes(take(bytes, order)?)), false),
            Self::I32 => signed(i64::from(i32::from_le_bytes(take(bytes, order)?))),
            Self::U64 => group(u128::from(u64::from_le_bytes(take(bytes, order)?)), false),
            Self::I64 => signed(i64::from_le_bytes(take(bytes, order)?)),
            Self::F32 => real(f64::from(f32::from_le_bytes(take(bytes, order)?))),
            Self::F64 => real(f64::from_le_bytes(take(bytes, order)?)),
            Self::Leb128 => group(u128::from(leb128(bytes)?.0), false),
            Self::Utf8 => utf8(bytes)?,
            Self::UnixSeconds => utc(i64::from(u32::from_le_bytes(take(bytes, order)?))),
        })
    }
}

/// The first `N` bytes in little-endian order, reversed from big-endian.
fn take<const N: usize>(bytes: &[u8], order: Order) -> Option<[u8; N]> {
    little_endian(bytes, 0, order == Order::Little)
}

/// Digits in groups of three separated by spaces: `1 936 352 768`.
fn group(n: u128, negative: bool) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if negative {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}
fn signed(n: i64) -> String {
    group(u128::from(n.unsigned_abs()), n < 0)
}

/// A real in plain notation when it is of ordinary size, otherwise scientific.
pub fn real(n: f64) -> String {
    if !n.is_finite() {
        return format!("{n}");
    }
    let magnitude = n.abs();
    if magnitude == 0.0 || (1e-4..1e9).contains(&magnitude) {
        let fixed = format!("{n:.6}");
        let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
        if trimmed == "-0" { "0" } else { trimmed }.to_owned()
    } else {
        format!("{n:.3e}")
    }
}

/// The character starting at the cursor, `NUL` for zero, or `None` when the
/// bytes there do not start a valid UTF-8 character.
fn utf8(bytes: &[u8]) -> Option<String> {
    let first = *bytes.first()?;
    let width = match first {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let c = std::str::from_utf8(bytes.get(..width)?)
        .ok()?
        .chars()
        .next()?;
    Some(match c {
        '\0' => "NUL".into(),
        c if c.is_control() => format!("U+{:04X}", u32::from(c)),
        c => c.to_string(),
    })
}

/// Shannon entropy in bits per byte, from 0 (one value) to 8 (uniform).
pub fn entropy(bytes: &[u8]) -> f32 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[usize::from(b)] += 1;
    }
    let total = bytes.len() as f32;
    counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = n as f32 / total;
            -p * p.log2()
        })
        .sum()
}

/// Seconds since 1970 as a UTC date and time, `2031-05-12 11:46:08`.
pub fn utc(seconds: i64) -> String {
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{Field, Order, leb128, real, utc};

    #[test]
    fn readings_follow_the_byte_order_and_report_short_input() {
        let bytes = [0x00, 0x66, 0x6a, 0x73, 0x80, 0x00, 0x00, 0x00];
        let read = |field: Field, order| field.read(&bytes, order);
        assert_eq!(read(Field::Binary, Order::Little).unwrap(), "00000000");
        assert_eq!(read(Field::U16, Order::Little).unwrap(), "26 112");
        assert_eq!(read(Field::U16, Order::Big).unwrap(), "102");
        assert_eq!(read(Field::U32, Order::Little).unwrap(), "1 936 352 768");
        assert_eq!(read(Field::I32, Order::Little).unwrap(), "1 936 352 768");
        assert_eq!(read(Field::U64, Order::Little).unwrap(), "551 692 166 656");
        assert_eq!(read(Field::Utf8, Order::Little).unwrap(), "NUL");
        assert_eq!(read(Field::Leb128, Order::Little).unwrap(), "0");
        assert_eq!(read(Field::F32, Order::Little).unwrap(), "1.857e31");
        assert_eq!(Field::I8.read(&[0xFF], Order::Big).unwrap(), "-1");
        assert_eq!(Field::I64.read(&[0xFF; 8], Order::Big).unwrap(), "-1");
        assert_eq!(Field::Utf8.read("λ".as_bytes(), Order::Big).unwrap(), "λ");
        assert_eq!(Field::Utf8.read(&[0x07], Order::Big).unwrap(), "U+0007");
        assert_eq!(Field::Utf8.read(&[0xCE], Order::Big), None);
        for field in Field::ALL {
            assert_eq!(field.read(&[], Order::Little), None, "{field:?}");
            let short = vec![0x80; field.width() - 1];
            if field.width() > 1 && field != Field::Utf8 {
                assert_eq!(field.read(&short, Order::Little), None, "{field:?}");
            }
            assert!(!field.label().is_empty());
        }
    }

    #[test]
    fn numbers_dates_and_varints_are_formatted() {
        assert_eq!(real(0.0312), "0.0312");
        assert_eq!(real(-0.0), "0");
        assert_eq!(real(1.0), "1");
        assert_eq!(real(-2.202e12), "-2.202e12");
        assert_eq!(real(f64::NAN), "NaN");
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_936_352_768), "2031-05-12 11:46:08");
        assert_eq!(utc(-1), "1969-12-31 23:59:59");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(leb128(&[0xE5, 0x8E, 0x26]), Some((624_485, 3)));
        assert_eq!(leb128(&[0x80; 10]), None);
        assert_eq!(super::entropy(&[7; 100]), 0.0);
        let uniform: Vec<u8> = (0..=255).collect();
        assert!((super::entropy(&uniform) - 8.0).abs() < 1e-4);
        assert_eq!(super::entropy(&[]), 0.0);
        assert_eq!(
            leb128(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]),
            Some((u64::MAX, 10))
        );
    }
}
