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
/// Images that failed to decode, remembered so they are not decoded again;
/// the oldest is forgotten first.
const FAILURES: usize = 32;

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
    /// A texture, and the last pass it was asked for.
    Ready(egui::TextureHandle, Meta, u64),
    Failed(String),
}

/// An image as it can be shown now.
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
    /// How the last copy to the clipboard went, until it is reported.
    pub copied: Option<Result<(), String>>,
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
        let pass = ctx.cumulative_pass_nr();
        if let Some((_, state)) = self.states.iter_mut().find(|(k, _)| *k == key) {
            let shown = match state {
                State::Pending(_) => Shown::Pending,
                State::Ready(texture, meta, asked) => {
                    *asked = pass;
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
        let Some(id) = self
            .decoder
            .queue(ctx, Work::Image(bytes, format, size.fit(ctx)))
        else {
            let reason = "Could not start the image decoder".to_owned();
            if size == Size::Copy {
                self.copied = Some(Err(reason.clone()));
            }
            return Shown::Failed(reason);
        };
        self.states.push((key, State::Pending(id)));
        Shown::Pending
    }

    /// Upload each decoded image that is still wanted; drop the rest.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Some((id, output)) = self.decoder.next() {
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
                    self.copied = Some(Ok(()));
                }
                // A copy is not kept, so a failed one can be tried again.
                Err(reason) if key.1 == Size::Copy => {
                    self.states.remove(index);
                    self.copied = Some(Err(reason));
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
                    let pass = ctx.cumulative_pass_nr();
                    self.states[index].1 = State::Ready(texture, Meta::of(&picture), pass);
                    self.order.push_back(key);
                    self.evict(pass);
                }
                Err(reason) => {
                    self.states[index].1 = State::Failed(reason);
                    self.forget_failures();
                }
            }
        }
    }

    /// Forget the oldest failures past the limit.
    fn forget_failures(&mut self) {
        let failed = |s: &State| matches!(s, State::Failed(_));
        let mut extra = self
            .states
            .iter()
            .filter(|(_, s)| failed(s))
            .count()
            .saturating_sub(FAILURES);
        self.states.retain(|(_, s)| {
            let forget = extra > 0 && failed(s);
            extra -= usize::from(forget);
            !forget
        });
    }

    /// Free the least recently shown textures past the limit. Textures asked
    /// for in this pass or the one before are on screen and kept, even past
    /// the limit, or a record showing more images than the limit would
    /// decode and free them in turn forever.
    fn evict(&mut self, pass: u64) {
        while self.order.len() > TEXTURES {
            let shown = |key: &(Target, Size)| {
                self.states.iter().any(|(k, s)| {
                    k == key && matches!(s, State::Ready(_, _, asked) if *asked + 1 >= pass)
                })
            };
            let Some(at) = self.order.iter().position(|key| !shown(key)) else {
                return;
            };
            if let Some(old) = self.order.remove(at) {
                self.states.retain(|(k, _)| *k != old);
            }
        }
    }

    /// Keep decodes for record `row` of this result only: decodes for other
    /// records are cancelled, so they upload nothing.
    pub fn keep_row(&mut self, result: Option<tursobench_core::Ticket>, drafts: u64, row: usize) {
        let stale =
            |k: &(Target, Size)| k.0.result != result || k.0.drafts != drafts || k.0.row != row;
        let pending = |s: &State| matches!(s, State::Pending(_));
        if !self.states.iter().any(|(k, s)| pending(s) && stale(k)) {
            return;
        }
        // Queued jobs share one cancel flag, so cancelling the stale ones
        // cancels every queued job: forget them all, and those still wanted
        // are queued again when next asked for.
        self.decoder.cancel_queued();
        if self
            .states
            .iter()
            .any(|((_, size), s)| pending(s) && *size == Size::Copy)
        {
            self.copied = Some(Err("the record changed before it was read".into()));
        }
        self.states.retain(|(_, s)| !pending(s));
    }

    /// Failures remembered, and decodes not yet answered.
    #[cfg(test)]
    pub fn failures_and_pending(&self) -> (usize, usize) {
        let count = |f: fn(&State) -> bool| self.states.iter().filter(|(_, s)| f(s)).count();
        (
            count(|s| matches!(s, State::Failed(_))),
            count(|s| matches!(s, State::Pending(_))),
        )
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
    use super::{FAILURES, Images, Shown, Size, TEXTURES, Target};
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
    /// Run `f` in one frame, as the app asks for images while painting.
    fn pass<T>(ctx: &egui::Context, f: impl FnOnce() -> T) -> T {
        let mut value = None;
        let mut f = Some(f);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
            value = f.take().map(|f| f());
        });
        output.textures_delta.clear();
        value.expect("the frame ran")
    }
    fn settle(images: &mut Images, ctx: &egui::Context, row: usize) -> Shown {
        for _ in 0..500 {
            let shown = pass(ctx, || {
                images.get(ctx, target(row), Size::Thumb, || {
                    Some((png(), ImageFormat::Png))
                })
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

    #[test]
    fn failures_stay_bounded_and_a_failed_copy_is_reported_and_tried_again() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        let garbage = || Some((Arc::from(&b"\x89PNG\r\n\x1a\nxx"[..]), ImageFormat::Png));
        for row in 0..FAILURES + 10 {
            images.get(&ctx, target(row), Size::Thumb, garbage);
        }
        for _ in 0..500 {
            images.poll(&ctx);
            if images.failures_and_pending().1 == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(images.failures_and_pending(), (FAILURES, 0));
        // A copy that fails says so, and is not remembered as failed.
        images.get(&ctx, target(0), Size::Copy, garbage);
        for _ in 0..500 {
            images.poll(&ctx);
            if images.copied.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(images.copied.take(), Some(Err(_))));
        assert!(matches!(
            images.get(&ctx, target(0), Size::Copy, garbage),
            Shown::Pending
        ));
    }

    /// Wait for every queued decode, a frame at a time, asking for `rows`.
    fn frames(images: &mut Images, ctx: &egui::Context, rows: std::ops::Range<usize>, n: usize) {
        for _ in 0..n {
            pass(ctx, || {
                for row in rows.clone() {
                    images.get(ctx, target(row), Size::Thumb, || {
                        Some((png(), ImageFormat::Png))
                    });
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_record_with_more_images_than_the_limit_decodes_each_once() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        let shown = 0..TEXTURES + 1;
        frames(&mut images, &ctx, shown.clone(), 300);
        assert_eq!(
            images.textures(),
            TEXTURES + 1,
            "every image on screen is kept"
        );
        let uploads = images.uploads;
        frames(&mut images, &ctx, shown, 50);
        assert_eq!(images.uploads, uploads, "nothing is decoded again");
        // Off screen, the extra textures go when the next image arrives.
        frames(&mut images, &ctx, 100..101, 300);
        assert!(images.textures() <= TEXTURES);
    }

    #[test]
    fn a_decode_still_wanted_after_a_cancel_is_queued_again() {
        let ctx = egui::Context::default();
        let mut images = Images::default();
        let bytes = || Some((png(), ImageFormat::Png));
        // Row 5's decode is queued, then row 6's under the same cancel flag.
        images.get(&ctx, target(5), Size::Full, bytes);
        images.get(&ctx, target(6), Size::Full, bytes);
        images.keep_row(None, 0, 6);
        assert_eq!(images.failures_and_pending(), (0, 0));
        assert!(matches!(
            settle_size(&mut images, &ctx, 6, Size::Full),
            Shown::Ready(..)
        ));
    }

    fn settle_size(images: &mut Images, ctx: &egui::Context, row: usize, size: Size) -> Shown {
        for _ in 0..500 {
            let shown = images.get(ctx, target(row), size, || Some((png(), ImageFormat::Png)));
            if !matches!(shown, Shown::Pending) {
                return shown;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the image never decoded")
    }
}
