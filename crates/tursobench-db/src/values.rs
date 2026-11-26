//! Disposable, session-owned backing for cells too large for retained UI memory.
use std::{
    collections::VecDeque,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
use tursobench_core::{DEFERRED_HEAD, DeferredCell, EventKind, Failure, LargeKind, Value};

const TOTAL: usize = 256 * 1024 * 1024;
const CELL: usize = 64 * 1024 * 1024;
/// The most of one value a lens may load whole.
pub(crate) const LOAD: usize = 16 * 1024 * 1024;
/// The longest byte sequence a search looks for.
pub(crate) const NEEDLE: usize = 1024;
/// How much of a value a viewer reads, or a search scans, at a time.
const CHUNK: usize = 64 * 1024;
pub(crate) struct Depot {
    files: VecDeque<(u64, std::fs::File, usize)>,
    next: u64,
    used: usize,
}
impl Default for Depot {
    fn default() -> Self {
        Self {
            files: VecDeque::new(),
            next: 1,
            used: 0,
        }
    }
}
impl Depot {
    pub(crate) fn defer(&mut self, value: Value) -> Value {
        let (kind, data) = match &value {
            Value::Text(v) => (LargeKind::Text, v.as_bytes()),
            Value::Blob(v) => (LargeKind::Blob, v.as_slice()),
            _ => return value,
        };
        let bytes = data.len();
        let handle = if bytes <= CELL {
            while self.used + bytes > TOTAL || self.files.len() >= 1024 {
                if let Some((_, _, size)) = self.files.pop_front() {
                    self.used -= size;
                } else {
                    break;
                }
            }
            // Anonymous backing is reclaimed on close, unwind and process death.
            tempfile::tempfile()
                .and_then(|mut file| {
                    file.write_all(data)?;
                    Ok(file)
                })
                .ok()
                .map(|file| {
                    let id = self.next;
                    self.next += 1;
                    self.used += bytes;
                    self.files.push_back((id, file, bytes));
                    id
                })
        } else {
            None
        };
        Value::Deferred(Box::new(DeferredCell {
            handle,
            kind,
            bytes: bytes as u64,
            head: data[..bytes.min(DEFERRED_HEAD)].to_vec(),
        }))
    }
    fn backing(&mut self, id: u64) -> Result<(&mut std::fs::File, usize), Failure> {
        self.files
            .iter_mut()
            .find(|(key, _, _)| *key == id)
            .map(|(_, file, size)| (file, *size))
            .ok_or_else(|| Failure::input("Value backing expired or belongs to another session"))
    }
    /// Up to `size` bytes of value `handle` from `offset`, as the chunk event.
    pub(crate) fn read(
        &mut self,
        handle: u64,
        offset: u64,
        size: usize,
    ) -> Result<EventKind, Failure> {
        if size == 0 || size > CHUNK {
            return Err(Failure::input("Value chunks require 1–65536 bytes"));
        }
        let (file, total) = self.backing(handle)?;
        if offset > total as u64 {
            return Err(Failure::input("Offset exceeds value size"));
        }
        file.seek(SeekFrom::Start(offset)).map_err(input)?;
        let mut data = vec![0; size.min(total - offset as usize)];
        file.read_exact(&mut data).map_err(input)?;
        Ok(EventKind::ValueChunk {
            handle,
            offset,
            bytes: data,
            total: total as u64,
        })
    }
    /// The whole of value `handle`, when it is at most [`LOAD`] bytes.
    pub(crate) fn load(&mut self, handle: u64) -> Result<EventKind, Failure> {
        let (file, total) = self.backing(handle)?;
        if total > LOAD {
            return Err(Failure::input(
                "The value exceeds the 16 MiB a lens may load; page through it as hex or save it",
            ));
        }
        file.seek(SeekFrom::Start(0)).map_err(input)?;
        let mut bytes = vec![0; total];
        file.read_exact(&mut bytes).map_err(input)?;
        Ok(EventKind::ValueLoaded { handle, bytes })
    }
    /// The first occurrence of `needle` in value `handle` at or after `from`,
    /// scanning chunk by chunk with an overlap, so a match across a chunk
    /// boundary is found.
    pub(crate) fn find(
        &mut self,
        handle: u64,
        needle: &[u8],
        from: u64,
    ) -> Result<EventKind, Failure> {
        if needle.is_empty() || needle.len() > NEEDLE {
            return Err(Failure::input("Searches look for 1–1024 bytes"));
        }
        let (file, total) = self.backing(handle)?;
        let mut start = from.min(total as u64);
        file.seek(SeekFrom::Start(start)).map_err(input)?;
        let mut window: Vec<u8> = Vec::with_capacity(CHUNK + needle.len());
        let mut chunk = vec![0; CHUNK];
        loop {
            let read = file.read(&mut chunk).map_err(input)?;
            if read == 0 {
                return Ok(EventKind::ValueFound {
                    handle,
                    offset: None,
                });
            }
            window.extend_from_slice(&chunk[..read]);
            if let Some(at) = window.windows(needle.len()).position(|w| w == needle) {
                let offset = Some(start + at as u64);
                return Ok(EventKind::ValueFound { handle, offset });
            }
            // Keep the last bytes, which may begin a match the next chunk ends.
            let keep = window.len().min(needle.len() - 1);
            start += (window.len() - keep) as u64;
            window.drain(..window.len() - keep);
        }
    }
    /// Writes value `handle` to a new file at `path`, answering with the saved event.
    pub(crate) fn save(&mut self, handle: u64, path: &Path) -> Result<EventKind, Failure> {
        let (file, _) = self.backing(handle)?;
        let mut output = crate::transfers::sibling_temp(path).map_err(input)?;
        file.seek(SeekFrom::Start(0)).map_err(input)?;
        std::io::copy(file, &mut output).map_err(input)?;
        output.as_file().sync_all().map_err(input)?;
        output.persist_noclobber(path).map_err(input)?;
        Ok(EventKind::ValueSaved(path.display().to_string()))
    }
}
fn input(e: impl std::fmt::Display) -> Failure {
    Failure::input(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{CELL, CHUNK, Depot, LOAD, TOTAL};
    use tursobench_core::{EventKind, Value};
    fn handle(value: Value) -> u64 {
        let Value::Deferred(cell) = value else {
            panic!("expected deferred value")
        };
        cell.handle.unwrap()
    }
    #[test]
    fn resource_limits_expire_handles_without_substituting_values() {
        let mut depot = Depot::default();
        let first = handle(depot.defer(Value::Blob(vec![7; CELL])));
        for _ in 0..4 {
            depot.defer(Value::Blob(vec![8; CELL]));
        }
        assert_eq!(depot.used, TOTAL);
        assert!(depot.read(first, 0, 1).is_err());
        let oversized = depot.defer(Value::Blob(vec![0; CELL + 1]));
        assert!(
            matches!(oversized, Value::Deferred(cell) if cell.handle.is_none() && cell.bytes == (CELL + 1) as u64)
        );
        assert_eq!(depot.used, TOTAL);
        drop(depot);

        let mut depot = Depot::default();
        let first = handle(depot.defer(Value::Text("first".into())));
        let mut last = 0;
        for _ in 0..1024 {
            last = handle(depot.defer(Value::Blob(vec![1])));
        }
        assert_eq!(depot.files.len(), 1024);
        assert_eq!(depot.used, 1024);
        assert!(depot.read(first, 0, 1).is_err());
        assert!(
            matches!(depot.read(last, 0, 1), Ok(EventKind::ValueChunk { bytes, total: 1, .. }) if bytes == [1])
        );
        assert!(depot.read(last, 2, 1).is_err());
        assert!(depot.read(last, 0, 65537).is_err());
    }
    #[test]
    fn whole_values_load_under_the_cap_and_searches_cross_chunk_boundaries() {
        let mut depot = Depot::default();
        let mut bytes = vec![0u8; 3 * CHUNK];
        // A needle split across the first chunk boundary, then one inside the third chunk.
        bytes[CHUNK - 2..CHUNK + 2].copy_from_slice(b"seam");
        bytes[2 * CHUNK + 10..2 * CHUNK + 14].copy_from_slice(b"seam");
        let id = handle(depot.defer(Value::Blob(bytes.clone())));
        let found = |depot: &mut Depot, from| match depot.find(id, b"seam", from) {
            Ok(EventKind::ValueFound { offset, .. }) => offset,
            other => panic!("{other:?}"),
        };
        assert_eq!(found(&mut depot, 0), Some(CHUNK as u64 - 2));
        assert_eq!(
            found(&mut depot, CHUNK as u64 - 1),
            Some(2 * CHUNK as u64 + 10)
        );
        assert_eq!(found(&mut depot, 2 * CHUNK as u64 + 11), None);
        assert_eq!(found(&mut depot, u64::MAX), None);
        assert!(depot.find(id, b"", 0).is_err());
        assert!(depot.find(id, &[1; 1025], 0).is_err());
        assert!(
            matches!(depot.load(id), Ok(EventKind::ValueLoaded { bytes: loaded, .. }) if loaded == bytes)
        );
        let large = handle(depot.defer(Value::Blob(vec![1; LOAD + 1])));
        assert!(depot.load(large).is_err());
        assert!(depot.load(9999).is_err());
        assert!(depot.find(9999, b"x", 0).is_err());
    }
    #[test]
    fn deferred_values_keep_their_first_bytes_even_without_backing() {
        let mut depot = Depot::default();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend((0..200).map(|i| i as u8));
        let Value::Deferred(cell) = depot.defer(Value::Blob(png.clone())) else {
            panic!("expected deferred value")
        };
        assert_eq!(cell.head, png[..64]);
        assert_eq!(cell.bytes, png.len() as u64);
        let Value::Deferred(cell) = depot.defer(Value::Text("short".into())) else {
            panic!("expected deferred value")
        };
        assert_eq!(cell.head, b"short");
        let mut oversized = vec![0x1f, 0x8b, 0x08];
        oversized.resize(CELL + 1, 0);
        let Value::Deferred(cell) = depot.defer(Value::Blob(oversized)) else {
            panic!("expected deferred value")
        };
        assert_eq!(
            (cell.handle, &cell.head[..3]),
            (None, &[0x1f, 0x8b, 0x08][..])
        );
        assert_eq!(cell.head.len(), 64);
    }
}
