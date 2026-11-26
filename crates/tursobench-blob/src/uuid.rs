//! Sixteen bytes read as an RFC 9562 UUID.

/// Which byte order the first three fields are stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Network order, as RFC 9562 stores it.
    Standard,
    /// The first three fields little-endian, as Microsoft GUIDs are stored.
    Mixed,
}

/// The RFC version of 16 bytes whose version and variant bits are those of
/// an RFC UUID in network order. These are four bits of evidence: random
/// bytes pass about one time in eight, so a UUID is a summary, never a lens.
pub fn version(bytes: &[u8]) -> Option<u8> {
    let bytes: &[u8; 16] = bytes.try_into().ok()?;
    let version = bytes[6] >> 4;
    (bytes[8] & 0xC0 == 0x80 && (1..=8).contains(&version)).then_some(version)
}

/// The canonical 36-character form of 16 bytes stored in `layout`.
pub fn format(bytes: &[u8; 16], layout: Layout) -> String {
    let mut ordered = *bytes;
    if layout == Layout::Mixed {
        ordered[..4].reverse();
        ordered[4..6].reverse();
        ordered[6..8].reverse();
    }
    let mut out = String::with_capacity(36);
    for (i, byte) in ordered.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Layout, format, version};

    #[test]
    fn rfc_uuids_are_formatted_in_both_layouts() {
        let bytes: [u8; 16] = [
            0x3f, 0x2a, 0x9c, 0x1e, 0x7b, 0x40, 0x4d, 0x1a, 0x9e, 0x5c, 0x0a, 0x1b, 0x2c, 0x3d,
            0x4e, 0x5f,
        ];
        assert_eq!(version(&bytes), Some(4));
        assert_eq!(
            format(&bytes, Layout::Standard),
            "3f2a9c1e-7b40-4d1a-9e5c-0a1b2c3d4e5f"
        );
        assert_eq!(
            format(&bytes, Layout::Mixed),
            "1e9c2a3f-407b-1a4d-9e5c-0a1b2c3d4e5f"
        );
    }

    #[test]
    fn other_sixteen_bytes_are_not_uuids() {
        assert_eq!(version(&[0; 16]), None);
        assert_eq!(version(&[0xff; 16]), None);
        assert_eq!(version(b"abcdefgh12345678"), None);
        assert_eq!(version(&[0x80; 15]), None);
        let mut nil_variant = [0u8; 16];
        nil_variant[6] = 0x40;
        assert_eq!(version(&nil_variant), None);
    }
}
