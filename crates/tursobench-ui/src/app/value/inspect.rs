//! The line under the hex lens: the cursor's offset, the selection's size,
//! and the bytes at the cursor read as numbers in the chosen byte order.
use super::view::View;
use tursobench_blob::inspect::Field;

/// The readings the footer has room for: three when wide, two otherwise.
pub(in crate::app) fn readings(view: &View, wide: bool) -> Vec<(&'static str, String)> {
    let fields: &[Field] = if wide {
        &[Field::U32, Field::I32, Field::F32]
    } else {
        &[Field::U32, Field::F32]
    };
    let bytes = view.bytes(view.cursor, 8).unwrap_or_default();
    fields
        .iter()
        .map(|field| {
            let value = field.read(&bytes, view.order).unwrap_or_else(|| "—".into());
            (field.label(), value)
        })
        .collect()
}

/// An offset as the footer shows it.
pub(in crate::app) fn offset_text(at: u64) -> String {
    format!("0x{at:06x}")
}

/// A typed offset: hexadecimal with `0x`, otherwise decimal.
pub(in crate::app) fn parse_offset(text: &str) -> Option<u64> {
    let text = text.trim().replace([' ', '_'], "");
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::{offset_text, parse_offset, readings};
    use crate::app::value::view::View;
    use tursobench_blob::Order;
    use tursobench_core::Value;

    #[test]
    fn readings_follow_the_cursor_and_byte_order() {
        let value = Value::Blob(vec![0x00, 0x66, 0x6a, 0x73, 0x80]);
        let mut view = View::new(0, 0, &value, (None, 0)).unwrap();
        view.cursor = 2;
        let read = |view: &View, wide| readings(view, wide);
        assert_eq!(read(&view, false)[0], ("u32", "—".into()));
        view.cursor = 0;
        assert_eq!(read(&view, true)[0], ("u32", "1 936 352 768".into()));
        assert_eq!(read(&view, true).len(), 3);
        view.order = Order::Big;
        assert_eq!(read(&view, false)[0], ("u32", "6 711 923".into()));
        assert_eq!(offset_text(0x1a3d4), "0x01a3d4");
        assert_eq!(parse_offset("0x1A3D4"), Some(0x1a3d4));
        assert_eq!(parse_offset(" 1 024 "), Some(1024));
        assert_eq!(parse_offset("zz"), None);
    }
}
