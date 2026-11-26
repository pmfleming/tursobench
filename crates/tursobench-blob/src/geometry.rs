//! Geometry stored as GeoPackage, SpatiaLite or plain WKB blobs, written out
//! as WKT. Only two-dimensional coordinates are read; Z and M are named.

/// How a geometry blob is wrapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    GeoPackage,
    SpatiaLite,
    Wkb,
}
impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Self::GeoPackage => "GeoPackage",
            Self::SpatiaLite => "SpatiaLite",
            Self::Wkb => "WKB",
        }
    }
}

/// The most coordinates written out.
const POINTS: usize = 100_000;
/// The deepest nesting of collections read.
const DEPTH: usize = 16;

const TYPES: [&str; 7] = [
    "POINT",
    "LINESTRING",
    "POLYGON",
    "MULTIPOINT",
    "MULTILINESTRING",
    "MULTIPOLYGON",
    "GEOMETRYCOLLECTION",
];

/// A WKB type code's name, without Z, M or SRID flags.
fn type_name(code: u32) -> Option<&'static str> {
    let base = (code & 0x0fff_ffff) % 1000;
    TYPES
        .get(usize::try_from(base).ok()?.checked_sub(1)?)
        .copied()
}
fn u32_at(bytes: &[u8], at: usize, little: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    })
}

/// The container and geometry type of a value from its first bytes, its
/// last bytes `tail`, and its length.
pub fn detect(head: &[u8], tail: &[u8], len: u64) -> Option<(Container, &'static str)> {
    if head.starts_with(b"GP") && head.len() >= 8 {
        let envelope = [0, 32, 48, 48, 64]
            .get(usize::from(head[3] >> 1 & 7))
            .copied()?;
        let at = 8 + envelope;
        let little = *head.get(at)? == 1;
        return Some((
            Container::GeoPackage,
            type_name(u32_at(head, at + 1, little)?)?,
        ));
    }
    let spatialite = head.first() == Some(&0)
        && matches!(head.get(1), Some(0 | 1))
        && head.get(38) == Some(&0x7c);
    if spatialite && tail.last() == Some(&0xfe) {
        return Some((
            Container::SpatiaLite,
            type_name(u32_at(head, 39, head[1] == 1)?)?,
        ));
    }
    // Plain WKB has no signature: a whole small value must read as one geometry.
    if (head.len() as u64) == len && matches!(head.first(), Some(0 | 1)) && wkt(head).is_ok() {
        return Some((Container::Wkb, type_name(u32_at(head, 1, head[0] == 1)?)?));
    }
    None
}

/// The geometry as WKT.
pub fn wkt(bytes: &[u8]) -> Result<String, String> {
    let mut reader = Reader {
        bytes,
        at: 0,
        points: 0,
        spatialite: false,
    };
    if bytes.starts_with(b"GP") {
        let flags = *bytes.get(3).ok_or("Truncated GeoPackage header")?;
        let envelope = [0, 32, 48, 48, 64]
            .get(usize::from(flags >> 1 & 7))
            .copied()
            .ok_or("Invalid GeoPackage envelope")?;
        reader.at = 8 + envelope;
    } else if bytes.first() == Some(&0)
        && bytes.get(38) == Some(&0x7c)
        && bytes.last() == Some(&0xfe)
    {
        reader.spatialite = true;
        reader.at = 38;
    }
    let mut out = String::new();
    reader.geometry(&mut out, 0)?;
    let trailing = if reader.spatialite { 1 } else { 0 };
    if reader.at + trailing != bytes.len() {
        return Err("Bytes follow the geometry".into());
    }
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    points: usize,
    /// SpatiaLite: one byte order for the blob, entity markers instead.
    spatialite: bool,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let slice = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or("The geometry ends early")?;
        self.at += n;
        Ok(slice)
    }
    fn u32(&mut self, little: bool) -> Result<u32, String> {
        let raw: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| "The geometry ends early")?;
        Ok(if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }
    fn f64(&mut self, little: bool) -> Result<f64, String> {
        let raw: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| "The geometry ends early")?;
        Ok(if little {
            f64::from_le_bytes(raw)
        } else {
            f64::from_be_bytes(raw)
        })
    }
    /// One geometry: its byte order (or SpatiaLite's marker), type and body.
    fn geometry(&mut self, out: &mut String, depth: usize) -> Result<(), String> {
        if depth > DEPTH {
            return Err("Geometry nested too deeply".into());
        }
        let little = if self.spatialite {
            self.take(1)?;
            self.bytes.get(1) == Some(&1)
        } else {
            match self.take(1)?[0] {
                0 => false,
                1 => true,
                _ => return Err("Invalid WKB byte order".into()),
            }
        };
        let code = self.u32(little)?;
        if code & 0x2000_0000 != 0 {
            self.u32(little)?;
        }
        let name = type_name(code).ok_or("Unknown geometry type")?;
        let dims = dimensions(code);
        out.push_str(name);
        let base = (code & 0x0fff_ffff) % 1000;
        self.body(out, base, little, dims, depth)
    }
    fn body(
        &mut self,
        out: &mut String,
        base: u32,
        little: bool,
        dims: usize,
        depth: usize,
    ) -> Result<(), String> {
        match base {
            1 => {
                out.push_str(" (");
                self.point(out, little, dims)?;
                out.push(')');
            }
            2 => self.points(out, little, dims)?,
            3 => self.rings(out, little, dims)?,
            _ => {
                let count = self.count(little, 9)?;
                out.push_str(" (");
                for i in 0..count {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    self.geometry(out, depth + 1)?;
                }
                out.push(')');
            }
        }
        Ok(())
    }
    /// A count no larger than the bytes left could hold at `size` bytes each.
    fn count(&mut self, little: bool, size: usize) -> Result<usize, String> {
        let count = self.u32(little)? as usize;
        if count.saturating_mul(size) > self.bytes.len() - self.at {
            return Err("A count overruns the geometry".into());
        }
        Ok(count)
    }
    fn point(&mut self, out: &mut String, little: bool, dims: usize) -> Result<(), String> {
        self.points += 1;
        if self.points > POINTS {
            return Err("More coordinates than a lens writes".into());
        }
        let (x, y) = (self.f64(little)?, self.f64(little)?);
        for _ in 2..dims {
            self.f64(little)?;
        }
        out.push_str(&format!("{x} {y}"));
        Ok(())
    }
    fn points(&mut self, out: &mut String, little: bool, dims: usize) -> Result<(), String> {
        let count = self.count(little, dims * 8)?;
        out.push_str(" (");
        for i in 0..count {
            if i > 0 {
                out.push_str(", ");
            }
            self.point(out, little, dims)?;
        }
        out.push(')');
        Ok(())
    }
    fn rings(&mut self, out: &mut String, little: bool, dims: usize) -> Result<(), String> {
        let count = self.count(little, 4)?;
        out.push_str(" (");
        for i in 0..count {
            if i > 0 {
                out.push_str(", ");
            }
            self.points(out, little, dims)?;
        }
        out.push(')');
        Ok(())
    }
}

/// Coordinates per point: 2, or 3 or 4 with Z or M (ISO codes or EWKB flags).
fn dimensions(code: u32) -> usize {
    let iso = (code & 0x0fff_ffff) / 1000;
    let (z, m) = (
        code & 0x8000_0000 != 0 || matches!(iso, 1 | 3),
        code & 0x4000_0000 != 0 || matches!(iso, 2 | 3),
    );
    2 + usize::from(z) + usize::from(m)
}

#[cfg(test)]
mod tests {
    use super::{Container, detect, wkt};

    fn point(little: bool, x: f64, y: f64) -> Vec<u8> {
        let mut out = vec![u8::from(little)];
        if little {
            out.extend(1u32.to_le_bytes());
            out.extend(x.to_le_bytes());
            out.extend(y.to_le_bytes());
        } else {
            out.extend(1u32.to_be_bytes());
            out.extend(x.to_be_bytes());
            out.extend(y.to_be_bytes());
        }
        out
    }

    #[test]
    fn wkb_geopackage_and_spatialite_points_write_as_wkt() {
        let wkb = point(true, -6.26, 53.35);
        assert_eq!(wkt(&wkb).unwrap(), "POINT (-6.26 53.35)");
        assert_eq!(wkt(&point(false, 1.0, 2.5)).unwrap(), "POINT (1 2.5)");
        assert_eq!(
            detect(&wkb, &wkb[wkb.len() - 8..], wkb.len() as u64),
            Some((Container::Wkb, "POINT"))
        );
        let mut gp = b"GP\0\x01".to_vec();
        gp.extend(4326i32.to_le_bytes());
        gp.extend(&wkb);
        assert_eq!(wkt(&gp).unwrap(), "POINT (-6.26 53.35)");
        assert_eq!(
            detect(&gp, &[], gp.len() as u64),
            Some((Container::GeoPackage, "POINT"))
        );
        let mut line = vec![1u8];
        line.extend(2u32.to_le_bytes());
        line.extend(2u32.to_le_bytes());
        for v in [0.0f64, 0.0, 1.0, 1.5] {
            line.extend(v.to_le_bytes());
        }
        assert_eq!(wkt(&line).unwrap(), "LINESTRING (0 0, 1 1.5)");
        let mut multi = vec![1u8];
        multi.extend(4u32.to_le_bytes());
        multi.extend(2u32.to_le_bytes());
        multi.extend(point(true, 1.0, 2.0));
        multi.extend(point(true, 3.0, 4.0));
        assert_eq!(
            wkt(&multi).unwrap(),
            "MULTIPOINT (POINT (1 2), POINT (3 4))"
        );
        // SpatiaLite: header, MBR, marker, class, coordinates, end.
        let mut spatialite = vec![0u8, 1];
        spatialite.extend(4326i32.to_le_bytes());
        spatialite.extend([0u8; 32]);
        spatialite.push(0x7c);
        spatialite.extend(1u32.to_le_bytes());
        spatialite.extend(2.0f64.to_le_bytes());
        spatialite.extend(3.0f64.to_le_bytes());
        spatialite.push(0xfe);
        assert_eq!(wkt(&spatialite).unwrap(), "POINT (2 3)");
        let tail = &spatialite[spatialite.len() - 8..];
        assert_eq!(
            detect(
                &spatialite[..64.min(spatialite.len())],
                tail,
                spatialite.len() as u64
            ),
            Some((Container::SpatiaLite, "POINT"))
        );
    }

    #[test]
    fn truncated_and_lying_geometry_is_refused() {
        let wkb = point(true, 1.0, 2.0);
        assert!(wkt(&wkb[..wkb.len() - 1]).is_err());
        let mut lying = vec![1u8];
        lying.extend(2u32.to_le_bytes());
        lying.extend(u32::MAX.to_le_bytes());
        assert!(wkt(&lying).is_err());
        assert!(wkt(&[9, 1, 0, 0, 0]).is_err());
        assert_eq!(detect(b"\x01hello", &[], 6), None);
        let mut state = 99u64;
        for _ in 0..3000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut bytes = wkb.clone();
            let at = (state >> 33) as usize % bytes.len();
            bytes[at] ^= (state >> 7) as u8;
            let _ = wkt(&bytes);
        }
    }
}
