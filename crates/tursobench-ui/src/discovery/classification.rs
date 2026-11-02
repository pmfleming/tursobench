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

    pub fn symbol(self) -> &'static str {
        match self {
            Self::LikelyTurso => "T",
            Self::LikelySqlite => "S",
            Self::Unknown => "?",
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
        let number = |offset| u32::from_be_bytes(header[offset..offset + 4].try_into().unwrap());
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
    use super::*;

    #[test]
    fn shared_version_is_not_enough_to_claim_turso() {
        let mut header = [0; 100];
        header[96..100].copy_from_slice(&3_047_000u32.to_be_bytes());
        assert_eq!(
            DatabaseKind::from_header(&header),
            DatabaseKind::LikelySqlite
        );
        header[24..28].copy_from_slice(&7u32.to_be_bytes());
        assert_eq!(DatabaseKind::from_header(&header), DatabaseKind::Unknown);
        header[92..96].copy_from_slice(&7u32.to_be_bytes());
        assert_eq!(
            DatabaseKind::from_header(&header),
            DatabaseKind::LikelySqlite
        );
        header[24..28].copy_from_slice(&1u32.to_be_bytes());
        header[92..96].copy_from_slice(&3_047_000u32.to_be_bytes());
        header[18..20].copy_from_slice(&[2, 2]);
        header[44..48].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(
            DatabaseKind::from_header(&header),
            DatabaseKind::LikelyTurso
        );
    }

    #[test]
    fn missing_or_unrecognized_writer_information_stays_unknown() {
        for writer in [0u32, 1, 4_000_000, u32::MAX] {
            let mut header = [0; 100];
            header[96..100].copy_from_slice(&writer.to_be_bytes());
            assert_eq!(DatabaseKind::from_header(&header), DatabaseKind::Unknown);
        }
    }
}
