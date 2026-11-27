//! Pure, bounded readers for untrusted database value bytes.
//!
//! Every function here takes bytes and returns a description of them; none
//! touches the UI or the engine, allocates without a cap, or trusts a length
//! read from the bytes themselves.
#![forbid(unsafe_code)]
mod bytes;
pub mod encode;
pub mod formats;
pub mod geometry;
pub mod image;
pub mod inflate;
pub mod inspect;
pub mod sniff;
pub mod text;
pub mod tree;
pub mod uuid;
pub mod vector;

pub use sniff::{Kind, sniff};

/// Detection reads at most this many leading bytes, whatever the value's size.
pub const HEAD: usize = 64;

/// A little- or big-endian reading of multi-byte numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Little,
    Big,
}
