//! Checked byte windows and integer encodings shared by the readers.
pub(crate) fn slice_at(bytes: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(len)?)
}

pub(crate) fn array_at<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..)?.first_chunk().copied()
}

pub(crate) fn little_endian<const N: usize>(
    bytes: &[u8],
    at: usize,
    little: bool,
) -> Option<[u8; N]> {
    let mut array = array_at(bytes, at)?;
    if !little {
        array.reverse();
    }
    Some(array)
}

pub(crate) fn uint_be(bytes: &[u8], at: usize, len: usize) -> Option<u64> {
    Some(
        slice_at(bytes, at, len)?
            .iter()
            .fold(0, |n, &byte| (n << 8) | u64::from(byte)),
    )
}

/// An unsigned LEB128 number and its length in bytes; at most ten bytes are read.
pub fn leb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in bytes.iter().take(10).enumerate() {
        let part = u64::from(byte & 0x7F);
        if i == 9 && part > 1 {
            return None;
        }
        value |= part << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}
