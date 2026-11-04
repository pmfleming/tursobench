//! Disposable, session-owned backing for cells too large for retained UI memory.
use std::{
    collections::VecDeque,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
use tursobench_core::{DeferredCell, Failure, LargeKind, Value};

const TOTAL: usize = 256 * 1024 * 1024;
const CELL: usize = 64 * 1024 * 1024;
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
        }))
    }
    pub(crate) fn read(
        &mut self,
        id: u64,
        offset: u64,
        size: usize,
    ) -> Result<(Vec<u8>, u64), Failure> {
        if size == 0 || size > 64 * 1024 {
            return Err(Failure::input("Value chunks require 1–65536 bytes"));
        }
        let (_, file, total) = self
            .files
            .iter_mut()
            .find(|(key, _, _)| *key == id)
            .ok_or_else(|| Failure::input("Value backing expired or belongs to another session"))?;
        if offset > *total as u64 {
            return Err(Failure::input("Offset exceeds value size"));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| Failure::input(e.to_string()))?;
        let mut data = vec![0; size.min(*total - offset as usize)];
        file.read_exact(&mut data)
            .map_err(|e| Failure::input(e.to_string()))?;
        Ok((data, *total as u64))
    }
    pub(crate) fn save(&mut self, id: u64, path: &Path) -> Result<(), Failure> {
        let (_, file, _) = self
            .files
            .iter_mut()
            .find(|(key, _, _)| *key == id)
            .ok_or_else(|| Failure::input("Value backing expired or belongs to another session"))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut output =
            tempfile::NamedTempFile::new_in(parent).map_err(|e| Failure::input(e.to_string()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| Failure::input(e.to_string()))?;
        std::io::copy(file, &mut output).map_err(|e| Failure::input(e.to_string()))?;
        output
            .as_file()
            .sync_all()
            .map_err(|e| Failure::input(e.to_string()))?;
        output
            .persist_noclobber(path)
            .map_err(|e| Failure::input(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(depot.read(last, 0, 1).unwrap().0, vec![1]);
        assert!(depot.read(last, 2, 1).is_err());
        assert!(depot.read(last, 0, 65537).is_err());
    }
}
