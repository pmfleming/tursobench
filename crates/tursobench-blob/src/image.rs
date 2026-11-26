//! Images decoded within limits, then scaled on the CPU to a size the GPU
//! accepts. Only the first frame of an animation is decoded for display.
use crate::sniff::ImageFormat;
use image::{AnimationDecoder, ImageReader, Limits};
use std::{
    io::Cursor,
    sync::atomic::{AtomicBool, Ordering},
};

/// The largest encoded image decoded.
pub const ENCODED: usize = 16 * 1024 * 1024;
/// The largest width or height decoded.
pub const SIDE: u32 = 8192;
/// The most memory a decoded image may take.
pub const DECODED: u64 = 64 * 1024 * 1024;
/// Frames counted before an animation is said to have "at least" that many.
const FRAMES: usize = 64;

/// An image ready to upload: its RGBA pixels at `size`, and what it was.
#[derive(Clone, Debug)]
pub struct Picture {
    pub format: ImageFormat,
    /// The image's own width and height.
    pub width: u32,
    pub height: u32,
    /// The pixels' width and height after scaling to fit.
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    /// Colour model and depth, as "RGB 8-bit".
    pub color: &'static str,
    /// Frames counted, and whether there may be more.
    pub frames: usize,
    pub more_frames: bool,
}

fn format(format: ImageFormat) -> image::ImageFormat {
    match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::WebP => image::ImageFormat::WebP,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Ico => image::ImageFormat::Ico,
    }
}

fn reader(bytes: &[u8], kind: ImageFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format(kind));
    let mut limits = Limits::default();
    limits.max_image_width = Some(SIDE);
    limits.max_image_height = Some(SIDE);
    limits.max_alloc = Some(DECODED);
    reader.limits(limits);
    reader
}

/// The image's width and height, refused past the limits. The decoders'
/// own allocation limit is advisory, so the size is checked here too.
pub fn dimensions(bytes: &[u8], kind: ImageFormat) -> Result<(u32, u32), String> {
    if bytes.len() > ENCODED {
        return Err("Over the 16 MiB an image may be to preview".into());
    }
    let (width, height) = reader(bytes, kind)
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if width == 0 || height == 0 {
        return Err("The image is empty".into());
    }
    if width > SIDE || height > SIDE || u64::from(width) * u64::from(height) * 4 > DECODED {
        return Err(format!("{width} × {height} is too large to preview"));
    }
    Ok((width, height))
}

/// Decode the image, its first frame for an animation, and scale it to fit
/// within `fit` pixels a side.
pub fn decode(
    bytes: &[u8],
    kind: ImageFormat,
    fit: u32,
    cancel: &AtomicBool,
) -> Result<Picture, String> {
    let (width, height) = dimensions(bytes, kind)?;
    let image = reader(bytes, kind).decode().map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let color = color(image.color());
    let fit = fit.clamp(1, SIDE);
    let image = if width > fit || height > fit {
        image.thumbnail(fit, fit)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let (frames, more_frames) = frames(bytes, kind, width, height, cancel);
    Ok(Picture {
        format: kind,
        width,
        height,
        size,
        rgba: rgba.into_raw(),
        color,
        frames,
        more_frames,
    })
}

fn color(color: image::ColorType) -> &'static str {
    use image::ColorType::{L8, L16, La8, La16, Rgb8, Rgb16, Rgb32F, Rgba8, Rgba16, Rgba32F};
    match color {
        L8 => "Gray 8-bit",
        La8 => "Gray and alpha 8-bit",
        Rgb8 => "RGB 8-bit",
        Rgba8 => "RGBA 8-bit",
        L16 => "Gray 16-bit",
        La16 => "Gray and alpha 16-bit",
        Rgb16 => "RGB 16-bit",
        Rgba16 => "RGBA 16-bit",
        Rgb32F => "RGB float",
        Rgba32F => "RGBA float",
        _ => "Other",
    }
}

/// How many frames an animated GIF or WebP has, counted while the frames
/// decoded stay within the memory limit.
fn frames(
    bytes: &[u8],
    kind: ImageFormat,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> (usize, bool) {
    let budget = (DECODED / (u64::from(width) * u64::from(height) * 4).max(1)).max(1) as usize;
    let cap = budget.min(FRAMES);
    let animation = match kind {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .ok()
            .map(|d| d.into_frames()),
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .ok()
            .filter(|d| d.has_animation())
            .map(|d| d.into_frames()),
        _ => None,
    };
    let Some(animation) = animation else {
        return (1, false);
    };
    let mut count = 0;
    for frame in animation {
        if frame.is_err() || cancel.load(Ordering::Relaxed) {
            break;
        }
        count += 1;
        if count == cap {
            return (count, true);
        }
    }
    (count.max(1), false)
}

/// Encode RGBA pixels as PNG, for copying an image.
pub fn png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(
        encoder,
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{SIDE, decode, dimensions, png};
    use crate::sniff::{ImageFormat, Kind, sniff};
    use image::{Delay, Frame, ImageFormat as Format, Rgba, RgbaImage};
    use std::sync::atomic::AtomicBool;

    fn picture(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([x as u8, y as u8, 128, 255]))
    }
    fn encode(image: &RgbaImage, format: Format) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        match format {
            Format::Jpeg | Format::Bmp => image::DynamicImage::ImageRgba8(image.clone())
                .to_rgb8()
                .write_to(&mut out, format)
                .unwrap(),
            _ => image.write_to(&mut out, format).unwrap(),
        }
        out.into_inner()
    }
    fn gif(frames: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        let frames = (0..frames)
            .map(|_| Frame::from_parts(picture(8, 6), 0, 0, Delay::from_numer_denom_ms(100, 1)));
        encoder.encode_frames(frames).unwrap();
        drop(encoder);
        out
    }
    fn format_of(bytes: &[u8]) -> ImageFormat {
        match sniff(bytes, bytes.len() as u64) {
            Kind::Image { format, .. } => format,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_format_decodes_and_scales_to_fit() {
        let cancel = AtomicBool::new(false);
        let image = picture(40, 30);
        for format in [
            Format::Png,
            Format::Jpeg,
            Format::WebP,
            Format::Bmp,
            Format::Ico,
        ] {
            let bytes = encode(&image, format);
            let picture = decode(&bytes, format_of(&bytes), 20, &cancel).unwrap();
            assert_eq!((picture.width, picture.height), (40, 30), "{format:?}");
            assert_eq!(picture.size, [20, 15], "{format:?}");
            assert_eq!(picture.rgba.len(), 20 * 15 * 4);
            assert_eq!((picture.frames, picture.more_frames), (1, false));
        }
        let bytes = gif(3);
        let picture = decode(&bytes, ImageFormat::Gif, 64, &cancel).unwrap();
        assert_eq!((picture.size, picture.frames), ([8, 6], 3));
        let copied = png(&picture.rgba, 8, 6).unwrap();
        assert_eq!(format_of(&copied), ImageFormat::Png);
    }

    #[test]
    fn oversize_truncated_and_bomb_inputs_are_refused_without_panicking() {
        let cancel = AtomicBool::new(false);
        let header = |w: u32, h: u32| {
            let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            png.extend(w.to_be_bytes());
            png.extend(h.to_be_bytes());
            png.extend([8, 6, 0, 0, 0]);
            let crc = crc(&png[12..]);
            png.extend(crc.to_be_bytes());
            png
        };
        // Past a side, or a decoded size, the header alone refuses the image.
        assert!(dimensions(&header(SIDE + 1, 1), ImageFormat::Png).is_err());
        assert!(dimensions(&header(8000, 8000), ImageFormat::Png).is_err());
        assert!(decode(&vec![0; 17 * 1024 * 1024], ImageFormat::Png, 64, &cancel).is_err());
        // Truncated and garbled images fail, for every format.
        let image = picture(40, 30);
        for format in [
            Format::Png,
            Format::Jpeg,
            Format::WebP,
            Format::Bmp,
            Format::Ico,
            Format::Gif,
        ] {
            let bytes = if format == Format::Gif {
                gif(2)
            } else {
                encode(&image, format)
            };
            let kind = format_of(&bytes);
            for cut in [bytes.len() / 2, 20, 8] {
                assert!(
                    decode(&bytes[..cut], kind, 64, &cancel).is_err(),
                    "{format:?} cut at {cut}"
                );
            }
            let mut garbled = bytes.clone();
            for b in garbled.iter_mut().skip(16).step_by(7) {
                *b ^= 0x5a;
            }
            let _ = decode(&garbled, kind, 64, &cancel);
        }
        assert!(decode(&header(16, 16), ImageFormat::Png, 64, &cancel).is_err());
    }

    /// CRC-32 as PNG chunks use it.
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
