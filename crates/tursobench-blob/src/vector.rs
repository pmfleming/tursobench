//! Vectors in Turso's layouts (`turso_core` 0.8.3, `vector/vector_types.rs`).
//!
//! An even-length blob is float32 with no type byte. An odd-length blob ends
//! in its type: 1 float32, 2 float64, 3 one-bit, 4 eight-bit, 9 sparse.

/// How a vector's numbers are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    F32,
    F64,
    OneBit,
    EightBit,
    Sparse,
}
impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::OneBit => "1-bit",
            Self::EightBit => "i8",
            Self::Sparse => "sparse f32",
        }
    }
}

/// The most dimensions read into memory.
pub const DIMS: usize = 1 << 20;

/// A vector's layout and dimensions from its type byte and length: the
/// last bytes of the value, `tail`, and its length. Plain float32, which has
/// no type byte, is never named here.
pub fn layout(tail: &[u8], len: u64) -> Option<(Layout, usize)> {
    if len.is_multiple_of(2) || tail.is_empty() {
        return None;
    }
    let n = usize::try_from(len - 1).ok()?;
    let byte = |back: usize| tail.len().checked_sub(back).map(|i| tail[i]);
    match byte(1)? {
        1 if n.is_multiple_of(4) && n > 0 => Some((Layout::F32, n / 4)),
        2 if n.is_multiple_of(8) && n > 0 => Some((Layout::F64, n / 8)),
        3 if n > 0 && n.is_multiple_of(2) => {
            let dims = (n * 8).checked_sub(usize::from(byte(2)?))?;
            (dims > 0 && dims.div_ceil(8) < n).then_some((Layout::OneBit, dims))
        }
        4 if n >= 10 && n.is_multiple_of(2) => {
            let dims = n.checked_sub(10)?.checked_sub(usize::from(byte(2)?))?;
            (dims > 0 && align4(dims) + 8 == n - 2).then_some((Layout::EightBit, dims))
        }
        9 if n.is_multiple_of(4) && n >= 4 && (n - 4).is_multiple_of(8) => {
            let dims = u32::from_le_bytes(
                tail.get(tail.len().checked_sub(5)?..tail.len() - 1)?
                    .try_into()
                    .ok()?,
            );
            Some((Layout::Sparse, dims as usize))
        }
        _ => None,
    }
}

fn align4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

/// A vector read whole, as numbers in its own precision.
#[derive(Clone, Debug, PartialEq)]
pub struct Vector {
    pub layout: Layout,
    pub dims: usize,
    pub values: Values,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    F32(Vec<f32>),
    F64(Vec<f64>),
    Bits(Vec<i8>),
}

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Read `bytes` as a vector: by its type byte, or as plain float32 when
/// `plain` and the length allows.
pub fn read(bytes: &[u8], plain: bool) -> Result<Vector, String> {
    let len = bytes.len() as u64;
    let tail = &bytes[bytes.len().saturating_sub(8)..];
    let (layout, dims) = match layout(tail, len) {
        Some(found) => found,
        None if plain && len.is_multiple_of(4) && len > 0 => (Layout::F32, bytes.len() / 4),
        None => return Err("Not a vector in a Turso layout".into()),
    };
    if dims > DIMS {
        return Err(format!("{dims} dimensions are more than a lens reads"));
    }
    let data = if len.is_multiple_of(2) {
        bytes
    } else {
        &bytes[..bytes.len() - 1]
    };
    let values = match layout {
        Layout::F32 => Values::F32(f32s(data)),
        Layout::F64 => Values::F64(
            data.chunks_exact(8)
                .map(|c| f64::from_le_bytes(c.try_into().unwrap_or([0; 8])))
                .collect(),
        ),
        Layout::OneBit => Values::Bits(
            (0..dims)
                .map(|i| {
                    if data[i / 8] >> (i & 7) & 1 == 1 {
                        1
                    } else {
                        -1
                    }
                })
                .collect(),
        ),
        Layout::EightBit => {
            let at = align4(dims);
            let number = |i: usize| f32::from_le_bytes(data[i..i + 4].try_into().unwrap_or([0; 4]));
            let (alpha, shift) = (number(at), number(at + 4));
            Values::F32(
                data[..dims]
                    .iter()
                    .map(|&q| alpha * f32::from(q) + shift)
                    .collect(),
            )
        }
        Layout::Sparse => {
            let entries = (data.len() - 4) / 8;
            let values = f32s(&data[..entries * 4]);
            let mut dense = vec![0f32; dims];
            for (entry, value) in data[entries * 4..entries * 8].chunks_exact(4).zip(values) {
                let index = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
                *dense
                    .get_mut(index)
                    .ok_or("A sparse index is past the dimensions")? = value;
            }
            Values::F32(dense)
        }
    };
    Ok(Vector {
        layout,
        dims,
        values,
    })
}

impl Vector {
    /// Each value as a double.
    pub fn numbers(&self) -> Vec<f64> {
        match &self.values {
            Values::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Values::F64(v) => v.clone(),
            Values::Bits(v) => v.iter().map(|&x| f64::from(x)).collect(),
        }
    }
    /// The vector as the engine's `vector_extract` prints it.
    pub fn text(&self) -> String {
        let parts: Vec<String> = match &self.values {
            Values::F32(v) => v.iter().map(ToString::to_string).collect(),
            Values::F64(v) => v.iter().map(ToString::to_string).collect(),
            Values::Bits(v) => v.iter().map(ToString::to_string).collect(),
        };
        format!("[{}]", parts.join(","))
    }
    /// The Euclidean norm, smallest and largest value.
    pub fn stats(&self) -> (f64, f64, f64) {
        stats(&self.numbers())
    }
    /// The values in `n` buckets, each its value of greatest magnitude, for bars.
    pub fn buckets(&self, n: usize) -> Vec<f64> {
        buckets(&self.numbers(), n)
    }
}

/// The Euclidean norm, smallest and largest of `numbers`.
pub fn stats(numbers: &[f64]) -> (f64, f64, f64) {
    let norm = numbers.iter().map(|x| x * x).sum::<f64>().sqrt();
    let min = numbers.iter().copied().fold(f64::INFINITY, f64::min);
    let max = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (norm, min, max)
}
/// `numbers` in `n` buckets, each its value of greatest magnitude, for bars.
pub fn buckets(numbers: &[f64], n: usize) -> Vec<f64> {
    let n = n.clamp(1, numbers.len().max(1));
    (0..n)
        .map(|b| {
            let range =
                b * numbers.len() / n..((b + 1) * numbers.len() / n).max(b * numbers.len() / n + 1);
            numbers
                .get(range)
                .unwrap_or_default()
                .iter()
                .copied()
                .fold(
                    0.0,
                    |best: f64, x| if x.abs() > best.abs() { x } else { best },
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Layout, layout, read};

    #[test]
    fn type_bytes_and_lengths_name_the_layout() {
        let f64s: Vec<u8> = [1.5f64, -2.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .chain([2])
            .collect();
        assert_eq!(
            layout(&f64s[f64s.len() - 8..], f64s.len() as u64),
            Some((Layout::F64, 2))
        );
        assert_eq!(read(&f64s, false).unwrap().text(), "[1.5,-2]");
        // Even lengths are plain float32 only when asked.
        let plain: Vec<u8> = [0.25f32, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert!(read(&plain, false).is_err());
        let vector = read(&plain, true).unwrap();
        assert_eq!(
            (vector.layout, vector.dims, vector.text()),
            (Layout::F32, 2, "[0.25,1]".into())
        );
        assert_eq!(vector.stats(), ((0.0625f64 + 1.0).sqrt(), 0.25, 1.0));
        assert_eq!(vector.buckets(1), vec![1.0]);
        // A type byte the length does not fit is not a vector.
        assert_eq!(layout(&[0, 0, 2], 3), None);
        assert_eq!(layout(&[7], 5), None);
        assert!(read(&[1, 2, 3, 4, 5, 6, 7, 8, 9], false).is_err());
        assert!(read(&[], true).is_err());
    }
}
