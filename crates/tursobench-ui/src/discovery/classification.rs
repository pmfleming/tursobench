//! Engine hints from the shared SQLite-format header, never engine validation.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DatabaseKind {
    LikelyTurso,
    LikelySqlite,
    #[default]
    Unknown,
}

impl DatabaseKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::LikelyTurso => "Likely Turso",
            Self::LikelySqlite => "Likely SQLite",
            Self::Unknown => "Unknown engine",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::LikelyTurso => {
                "The header matches Turso 0.8.3's default writer signature. This is a heuristic; SQLite and Turso share a file format, and another writer can change the header."
            }
            Self::LikelySqlite => {
                "The header has a SQLite writer version with a matching change counter. This suggests SQLite, but does not prove which engine created or can open the file."
            }
            Self::Unknown => {
                "The shared SQLite-format header does not identify the engine reliably. Turso compatibility is checked when opening."
            }
        }
    }

    pub(super) fn from_header(header: &[u8; 100]) -> Self {
        let number = |offset| {
            u32::from_be_bytes([
                header[offset],
                header[offset + 1],
                header[offset + 2],
                header[offset + 3],
            ])
        };
        let counter = number(24);
        let valid_for = number(92);
        let writer = number(96);
        // Pinned turso_core 0.8.3's DatabaseHeader::default sets BOTH fields
        // to 3047000, independently of its change counter (initially 1).
        // A writer version of 3047000 alone also describes SQLite 3.47.0.
        if writer == 3_047_000
            && valid_for == 3_047_000
            && counter == 1
            && header[18..20] == [2, 2]
            && number(44) == 4
        {
            Self::LikelyTurso
        } else if (3_000_000..4_000_000).contains(&writer) && valid_for == counter {
            Self::LikelySqlite
        } else {
            Self::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DatabaseKind::{self, LikelySqlite, LikelyTurso, Unknown};

    #[test]
    fn header_classification_requires_the_complete_writer_signature() {
        for (writer, counter, valid_for, wal, schema, expected) in [
            (3_047_000u32, 0u32, 0u32, 0, 0u32, LikelySqlite),
            (3_047_000, 7, 0, 0, 0, Unknown),
            (3_047_000, 7, 7, 0, 0, LikelySqlite),
            (3_047_000, 1, 3_047_000, 2, 4, LikelyTurso),
            (3_047_000, 1, 3_047_000, 1, 4, Unknown),
            (3_047_000, 1, 3_047_000, 2, 3, Unknown),
            (0, 0, 0, 0, 0, Unknown),
            (1, 0, 0, 0, 0, Unknown),
            (4_000_000, 0, 0, 0, 0, Unknown),
            (u32::MAX, 0, 0, 0, 0, Unknown),
        ] {
            let mut header = [0; 100];
            for (offset, value) in [(96, writer), (24, counter), (92, valid_for), (44, schema)] {
                header[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
            }
            header[18..20].fill(wal);
            assert_eq!(DatabaseKind::from_header(&header), expected, "{header:?}");
        }
    }
}
