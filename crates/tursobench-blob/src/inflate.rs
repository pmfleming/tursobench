//! Compressed contents, inflated one level deep and never past a cap.
//! Sizes declared in the stream are ignored; the output is counted.
use crate::sniff::Compression;
use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};

/// The most a value inflates to for the contents lens.
pub const OUT: usize = 16 * 1024 * 1024;
/// The largest zstd window accepted (32 MiB), bounding the decoder's memory.
const ZSTD_WINDOW_LOG: u32 = 25;

/// Inflated bytes, and whether the cap cut them short.
#[derive(Debug)]
pub struct Inflated {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Inflate `bytes` up to `cap` bytes of output.
pub fn inflate(
    bytes: &[u8],
    compression: Compression,
    cap: usize,
    cancel: &AtomicBool,
) -> Result<Inflated, String> {
    let reader: Box<dyn Read + '_> = match compression {
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(bytes)),
        Compression::Zlib => Box::new(flate2::read::ZlibDecoder::new(bytes)),
        Compression::Zstd => {
            let mut decoder =
                zstd::stream::read::Decoder::with_buffer(bytes).map_err(|e| e.to_string())?;
            decoder
                .window_log_max(ZSTD_WINDOW_LOG)
                .map_err(|e| e.to_string())?;
            Box::new(decoder)
        }
    };
    let mut limited = reader.take(cap as u64 + 1);
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let read = limited.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..read]);
    }
    let truncated = out.len() > cap;
    out.truncate(cap);
    Ok(Inflated {
        bytes: out,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::{Compression, inflate};
    use std::{io::Write, sync::atomic::AtomicBool};

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn each_format_inflates_and_a_bomb_stops_at_the_cap() {
        let cancel = AtomicBool::new(false);
        let text = b"{\"job\":\"export-812\"}\n".repeat(50);
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(&text).unwrap();
        let zstd = zstd::bulk::compress(&text, 3).unwrap();
        for (format, bytes) in [
            (Compression::Gzip, gzip(&text)),
            (Compression::Zlib, zlib.finish().unwrap()),
            (Compression::Zstd, zstd),
        ] {
            let inflated = inflate(&bytes, format, 1 << 20, &cancel).unwrap();
            assert_eq!(
                (inflated.bytes, inflated.truncated),
                (text.clone(), false),
                "{format:?}"
            );
            assert!(
                inflate(&bytes[..bytes.len() / 2], format, 1 << 20, &cancel).is_err(),
                "{format:?}"
            );
        }
        // 64 MiB of zeros compresses to about 64 KiB; reading stops at the cap.
        let bomb = gzip(&vec![0u8; 64 << 20]);
        assert!(bomb.len() < 1 << 20);
        let inflated = inflate(&bomb, Compression::Gzip, 1 << 20, &cancel).unwrap();
        assert_eq!((inflated.bytes.len(), inflated.truncated), (1 << 20, true));
        assert!(inflate(b"\x1f\x8b\x08garbage", Compression::Gzip, 1 << 20, &cancel).is_err());
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(inflate(&gzip(&text), Compression::Gzip, 1 << 20, &cancel).is_err());
    }
}
