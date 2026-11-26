//! Lenses that decode before they paint: images, uploaded as textures once
//! decoded off the UI thread, at most eight kept.
use super::{
    Target,
    job::{Decoder, Output, Work},
};
use eframe::egui;
use std::{collections::VecDeque, sync::Arc};
use tursobench_blob::{image::Picture, sniff::ImageFormat};

/// Textures kept; the least recently shown is freed first.
pub(in crate::app) const TEXTURES: usize = 8;

/// How large an image is decoded for where it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Size {
    /// A thumbnail in the field's row.
    Thumb,
    /// The selected field's preview.
    Preview,
    /// The value view, at most as large as the GPU takes.
    Full,
    /// A copy to the clipboard: decoded, copied and not kept.
    Copy,
}
impl Size {
    fn fit(self, ctx: &egui::Context) -> u32 {
        match self {
            Self::Thumb => 48,
            Self::Preview => 256,
            Self::Full | Self::Copy => {
                let side = ctx.input(|i| i.max_texture_side) as u32;
                side.min(tursobench_blob::image::SIDE)
            }
        }
    }
}

/// What an image is, kept after its pixels are uploaded.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct Meta {
    pub width: u32,
    pub height: u32,
    pub color: &'static str,
    pub frames: usize,
    pub more_frames: bool,
}
impl Meta {
    fn of(picture: &Picture) -> Self {
        Self {
            width: picture.width,
            height: picture.height,
            color: picture.color,
            frames: picture.frames,
            more_frames: picture.more_frames,
        }
    }
    /// "1 frame", "3 frames" or "64+ frames".
    pub fn frames(&self) -> String {
        let more = if self.more_frames { "+" } else { "" };
        let plural = if self.frames == 1 && !self.more_frames {
            ""
        } else {
            "s"
        };
        format!("{}{more} frame{plural}", self.frames)
    }
}

enum State {
    Pending(u64),
    Ready(egui::TextureHandle, Meta),
    Failed(String),
}

/// An image as it can be shown now.
#[derive(Clone)]
pub(in crate::app) enum Shown {
    Pending,
    Ready(egui::TextureId, egui::Vec2, Meta),
    Failed(String),
}

/// A document's decoded images and the thread that decodes them.
#[derive(Default)]
pub(in crate::app) struct Images {
    decoder: Decoder,
    states: Vec<((Target, Size), State)>,
    /// Ready images, least recently shown first.
    order: VecDeque<(Target, Size)>,
    #[cfg(test)]
    pub uploads: usize,
}
impl Images {
    /// The image of the value at `target` decoded for `size`, its decode
    /// started on first request with the bytes `bytes` gives.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        target: Target,
        size: Size,
        bytes: impl FnOnce() -> Option<(Arc<[u8]>, ImageFormat)>,
    ) -> Shown {
        self.poll(ctx);
        let key = (target, size);
        if let Some((_, state)) = self.states.iter().find(|(k, _)| *k == key) {
            let shown = match state {
                State::Pending(_) => Shown::Pending,
                State::Ready(texture, meta) => {
                    Shown::Ready(texture.id(), texture.size_vec2(), *meta)
                }
                State::Failed(reason) => Shown::Failed(reason.clone()),
            };
            if matches!(shown, Shown::Ready(..)) {
                self.order.retain(|k| *k != key);
                self.order.push_back(key);
            }
            return shown;
        }
        let Some((bytes, format)) = bytes() else {
            return Shown::Failed("Open the value to load the image".into());
        };
        let id = self
            .decoder
            .queue(ctx, Work::Image(bytes, format, size.fit(ctx)));
        self.states.push((key, State::Pending(id)));
        Shown::Pending
    }

    /// Upload each decoded image that is still wanted; drop the rest.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for (id, output) in self.decoder.poll_all() {
            let Some(index) = self
                .states
                .iter()
                .position(|(_, s)| matches!(s, State::Pending(p) if *p == id))
            else {
                continue;
            };
            let Output::Image(result) = output else {
                continue;
            };
            let key = self.states[index].0;
            match result {
                Ok(picture) if key.1 == Size::Copy => {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(picture.size, &picture.rgba);
                    ctx.copy_image(image);
                    self.states.remove(index);
                }
                Ok(picture) => {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(picture.size, &picture.rgba);
                    let name = format!("value-image-{id}");
                    let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                    #[cfg(test)]
                    {
                        self.uploads += 1;
                    }
                    self.states[index].1 = State::Ready(texture, Meta::of(&picture));
                    self.order.push_back(key);
                    self.evict();
                }
                Err(reason) => self.states[index].1 = State::Failed(reason),
            }
        }
    }

    /// Free the least recently shown textures past the limit.
    fn evict(&mut self) {
        while self.order.len() > TEXTURES {
            if let Some(old) = self.order.pop_front() {
                self.states.retain(|(k, _)| *k != old);
            }
        }
    }

    /// Keep decodes for record `row` of this result only: decodes for other
    /// records are cancelled, so they upload nothing.
    pub fn keep_row(&mut self, result: Option<tursobench_core::Ticket>, drafts: u64, row: usize) {
        let stale =
            |k: &(Target, Size)| k.0.result != result || k.0.drafts != drafts || k.0.row != row;
        let before = self.states.len();
        self.states
            .retain(|(k, s)| !(matches!(s, State::Pending(_)) && stale(k)));
        if self.states.len() != before {
            self.decoder.cancel_queued();
        }
    }

    /// Textures held now.
    #[cfg(test)]
    pub fn textures(&self) -> usize {
        self.states
            .iter()
            .filter(|(_, s)| matches!(s, State::Ready(..)))
            .count()
    }
}

/// A checkerboard behind transparent pixels, in two quiet tones.
pub(in crate::app) fn checkerboard(painter: &egui::Painter, rect: egui::Rect, ctx: &egui::Context) {
    let tokens = crate::appearance::Tokens::of(ctx);
    painter.rect_filled(rect, 0.0, tokens.raised);
    let cell = 8.0;
    let (columns, rows) = (
        (rect.width() / cell).ceil() as usize,
        (rect.height() / cell).ceil() as usize,
    );
    for row in 0..rows.min(512) {
        for column in (row % 2..columns.min(512)).step_by(2) {
            let min = rect.min + egui::vec2(column as f32, row as f32) * cell;
            let square = egui::Rect::from_min_size(min, egui::Vec2::splat(cell)).intersect(rect);
            painter.rect_filled(square, 0.0, tokens.raised_hover);
        }
    }
}

/// Paint texture `id` of `size` scaled by `scale`, centred in `rect` and
/// moved by `pan`, over a checkerboard; returns where it was drawn.
pub(in crate::app) fn paint_image(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: egui::TextureId,
    size: egui::Vec2,
    scale: f32,
    pan: egui::Vec2,
) -> egui::Rect {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let shown = egui::Rect::from_center_size(rect.center() + pan, size * scale);
    checkerboard(&painter, shown.intersect(rect), ui.ctx());
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(id, shown, uv, egui::Color32::WHITE);
    shown
}

/// The scale that fits `size` inside `rect`.
pub(in crate::app) fn fit(rect: egui::Rect, size: egui::Vec2) -> f32 {
    (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::{Images, Shown, Size, TEXTURES, Target};
    use eframe::egui;
    use std::sync::Arc;
    use tursobench_blob::sniff::ImageFormat;

    fn png() -> Arc<[u8]> {
        let rgba = [1, 2, 3, 255].repeat(12);
        Arc::from(tursobench_blob::image::png(&rgba, 4, 3).unwrap())
    }
    fn target(row: usize) -> Target {
        Target {
            result: None,
            drafts: 0,
            row,
            col: 0,
            whole: false,
        }
    }
    fn settle(images: &mut Images, ctx: &egui::Context, row: usize) -> Shown {
        for _ in 0..500 {
            let shown = images.get(ctx, target(row), Size::Thumb, || {
                Some((png(), ImageFormat::Png))
            });
            if !matches!(shown, Shown::Pending) {
                return shown;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the image never decoded")
    }

    #[test]
    fn textures_stay_bounded_and_cancelled_decodes_upload_nothing() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        for row in 0..TEXTURES + 4 {
            assert!(
                matches!(settle(&mut images, &ctx, row), Shown::Ready(_, size, meta) if size == egui::vec2(4.0, 3.0) && meta.width == 4)
            );
            assert!(images.textures() <= TEXTURES);
        }
        assert_eq!(images.textures(), TEXTURES);
        assert_eq!(images.uploads, TEXTURES + 4);
        // A decode for a record left behind is cancelled and never uploaded.
        let uploads = images.uploads;
        images.get(&ctx, target(100), Size::Thumb, || {
            Some((png(), ImageFormat::Png))
        });
        images.keep_row(None, 0, 101);
        std::thread::sleep(std::time::Duration::from_millis(50));
        images.poll(&ctx);
        assert_eq!(images.uploads, uploads);
        // Garbage fails without a texture.
        let shown = images.get(&ctx, target(200), Size::Thumb, || {
            Some((Arc::from(&b"\x89PNG\r\n\x1a\nxx"[..]), ImageFormat::Png))
        });
        assert!(matches!(shown, Shown::Pending));
        let mut failed = false;
        for _ in 0..500 {
            if let Shown::Failed(_) = images.get(&ctx, target(200), Size::Thumb, || None) {
                failed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(failed);
        assert!(matches!(
            images.get(&ctx, target(300), Size::Thumb, || None),
            Shown::Failed(_)
        ));
    }
}
