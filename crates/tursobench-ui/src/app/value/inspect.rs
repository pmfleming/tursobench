//! The line under the hex lens: the cursor's offset, the selection's size,
//! and the bytes at the cursor read as numbers in the chosen byte order.
use super::view::View;
use eframe::egui;
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

/// Every reading of the bytes at the cursor, for the expanded tile's list.
pub(in crate::app) fn all_readings(view: &View) -> Vec<(&'static str, String)> {
    let bytes = view.bytes(view.cursor, 10).unwrap_or_default();
    Field::ALL
        .iter()
        .map(|field| {
            let value = field.read(&bytes, view.order).unwrap_or_else(|| "—".into());
            (field.label(), value)
        })
        .collect()
}

/// The inspector list beside the hex in an expanded tile: each reading's
/// name, then its value at the right.
pub(in crate::app) fn list(ui: &egui::Ui, rect: egui::Rect, view: &View) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.vline(
        rect.left(),
        rect.y_range(),
        egui::Stroke::new(1.0, tokens.line_soft),
    );
    let font = crate::appearance::monospace(ui.ctx(), 11.0);
    let order = match view.order {
        tursobench_blob::Order::Little => "LE",
        tursobench_blob::Order::Big => "BE",
    };
    let mut rows = all_readings(view);
    rows.push(("byte order", order.into()));
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = rect.top() + 4.0 + i as f32 * 18.0;
        if y + 18.0 > rect.bottom() {
            break;
        }
        let left = egui::pos2(rect.left() + 10.0, y);
        painter.text(
            left,
            egui::Align2::LEFT_TOP,
            *label,
            font.clone(),
            tokens.faint,
        );
        let right = egui::pos2(rect.right() - 8.0, y);
        let color = ui.visuals().text_color();
        painter.text(right, egui::Align2::RIGHT_TOP, value, font.clone(), color);
    }
}

/// An entropy strip for the whole value: each segment shaded by how random
/// its bytes are, the lines shown outlined. Segments not in memory stay
/// blank. Returns the first line a click asks for.
pub(in crate::app) fn minimap(ui: &egui::Ui, rect: egui::Rect, view: &mut View) -> Option<i64> {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 2.0, tokens.raised);
    let len = view.source.len().max(1);
    let segments = ((rect.height() / 3.0) as u64).clamp(1, len);
    let size = len.div_ceil(segments);
    let height = rect.height() / segments as f32;
    for i in 0..segments {
        let key = (size, i);
        // Each segment's first 4 KiB, measured once its bytes are in memory.
        if !view.entropy.contains_key(&key)
            && let Some(sample) = view.bytes(i * size, size.min(4096) as usize)
        {
            let bits = tursobench_blob::inspect::entropy(&sample);
            view.entropy.insert(key, bits);
        }
        if let Some(&bits) = view.entropy.get(&key) {
            let band = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + i as f32 * height),
                egui::vec2(rect.width(), height.max(1.0)),
            );
            painter.rect_filled(band, 0.0, tokens.primary.gamma_multiply(bits / 8.0));
        }
    }
    let per = view.per.max(1) as u64;
    let y = |line: u64| rect.top() + (line * per) as f32 / len as f32 * rect.height();
    let (top, bottom) = (y(view.top), y(view.top + view.lines as u64));
    let window = egui::Rect::from_x_y_ranges(rect.x_range(), top..=bottom.max(top + 2.0));
    let stroke = egui::Stroke::new(1.0, tokens.muted);
    painter.rect_stroke(window, 1.0, stroke, egui::StrokeKind::Inside);
    let sense = egui::Sense::click_and_drag();
    let response = ui.interact(rect, ui.scope_id().with("value-minimap"), sense);
    let response = response.on_hover_text("Entropy · click to go there");
    let pressed = response.clicked() || response.dragged();
    let pos = response.interact_pointer_pos().filter(|_| pressed)?;
    let at = f64::from(((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0)) * len as f64;
    Some((at as u64 / per) as i64 - view.lines as i64 / 2)
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
