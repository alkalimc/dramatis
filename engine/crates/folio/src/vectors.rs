use crate::error::{Error, Result};

/// The dense side of the corpus: one contiguous f16 blob, ordered by `chunks.ord`.
///
/// Measured on the live corpus: 58,853 units at 1024 dimensions is 120 MB. That number is
/// why an exact full scan is viable at all — it fits comfortably in the resident budget,
/// so there is no approximate index, no build step for one, and no approximation error to
/// contaminate the dimension-ablation curve the retrieval paper depends on.
pub struct Vectors {
    /// Raw little-endian f16 pairs. Kept encoded rather than widened to f32 on load:
    /// widening would triple resident memory to save a conversion that is a handful of
    /// instructions inside a loop that is already memory-bound.
    raw: Vec<u8>,
    dim: usize,
    count: usize,
}

/// IEEE 754 binary16 → f32.
///
/// Written out rather than pulled from a crate: it is fifteen lines, it is on the hot
/// path, and a dependency here would have to be audited for exactly the subnormal and
/// infinity handling below.
#[inline(always)]
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits as u32 & 0x8000) << 16;
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = bits as u32 & 0x03ff;

    match exponent {
        // Zero or subnormal.
        0 => {
            if mantissa == 0 {
                return f32::from_bits(sign);
            }
            // Renormalise: shift the mantissa up until its leading bit clears bit 10,
            // decrementing the exponent to match.
            let mut mantissa = mantissa;
            let mut exponent: i32 = -1;
            while mantissa & 0x0400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            let mantissa = mantissa & 0x03ff;
            let exponent = (127 - 15 + exponent) as u32;
            f32::from_bits(sign | (exponent << 23) | (mantissa << 13))
        }
        // Infinity or NaN.
        0x1f => f32::from_bits(sign | 0x7f80_0000 | (mantissa << 13)),
        // Normal.
        _ => {
            let exponent = (exponent as u32) + (127 - 15);
            f32::from_bits(sign | (exponent << 23) | (mantissa << 13))
        }
    }
}

impl Vectors {
    pub fn new(raw: Vec<u8>, dim: usize, count: usize) -> Result<Self> {
        let expected = count * dim * 2;
        if raw.len() != expected {
            return Err(Error::VectorSizeMismatch {
                actual: raw.len(),
                expected,
                count,
                dim,
            });
        }
        Ok(Self { raw, dim, count })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn bytes(&self) -> usize {
        self.raw.len()
    }

    /// Decode one vector. For inspection and tests; the scan below avoids it.
    pub fn get(&self, ord: usize) -> Option<Vec<f32>> {
        if ord >= self.count {
            return None;
        }
        let start = ord * self.dim * 2;
        Some(
            self.raw[start..start + self.dim * 2]
                .chunks_exact(2)
                .map(|pair| f16_to_f32(u16::from_le_bytes([pair[0], pair[1]])))
                .collect(),
        )
    }

    /// Exact cosine scan against every unit, returning the top `k`.
    ///
    /// Assumes both sides are L2-normalised, which the encoder guarantees, so a dot
    /// product *is* cosine similarity and no per-row norm is needed.
    ///
    /// The inner loop decodes on the fly and accumulates in four independent partial sums.
    /// The split is not superstition: a single accumulator serialises on the dependency
    /// chain of the adds, and four lets the CPU overlap them. This is the hot path P11
    /// measures.
    pub fn top_k(&self, query: &[f32], k: usize) -> Result<Vec<(usize, f32)>> {
        if query.len() != self.dim {
            return Err(Error::DimMismatch {
                query: query.len(),
                corpus: self.dim,
            });
        }
        if k == 0 || self.count == 0 {
            return Ok(Vec::new());
        }

        // A bounded min-heap would beat a full sort for small k, but at 59k rows the sort
        // is a rounding error next to the scan itself — and it keeps this readable.
        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.count);
        let row_bytes = self.dim * 2;

        for ord in 0..self.count {
            let row = &self.raw[ord * row_bytes..(ord + 1) * row_bytes];
            let mut a = 0.0f32;
            let mut b = 0.0f32;
            let mut c = 0.0f32;
            let mut d = 0.0f32;

            let mut i = 0;
            while i + 4 <= self.dim {
                let base = i * 2;
                a += f16_to_f32(u16::from_le_bytes([row[base], row[base + 1]])) * query[i];
                b += f16_to_f32(u16::from_le_bytes([row[base + 2], row[base + 3]])) * query[i + 1];
                c += f16_to_f32(u16::from_le_bytes([row[base + 4], row[base + 5]])) * query[i + 2];
                d += f16_to_f32(u16::from_le_bytes([row[base + 6], row[base + 7]])) * query[i + 3];
                i += 4;
            }
            let mut tail = 0.0f32;
            while i < self.dim {
                let base = i * 2;
                tail += f16_to_f32(u16::from_le_bytes([row[base], row[base + 1]])) * query[i];
                i += 1;
            }

            scored.push((ord, a + b + c + d + tail));
        }

        scored.sort_unstable_by(|x, y| y.1.total_cmp(&x.1));
        scored.truncate(k);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(values: &[f32]) -> Vec<u8> {
        // Minimal f32 -> f16 for round-trip tests only. Truncates rather than rounds,
        // which is fine for the exact-power-of-two values used below.
        values
            .iter()
            .flat_map(|&v| {
                let bits = v.to_bits();
                let sign = ((bits >> 16) & 0x8000) as u16;
                let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
                let mantissa = ((bits >> 13) & 0x03ff) as u16;
                let half = if v == 0.0 {
                    sign
                } else {
                    sign | ((exponent as u16) << 10) | mantissa
                };
                half.to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn round_trips_representable_values() {
        for value in [0.0f32, 1.0, -1.0, 0.5, -0.25, 2.0, 1024.0] {
            let encoded = encode(&[value]);
            let decoded = f16_to_f32(u16::from_le_bytes([encoded[0], encoded[1]]));
            assert_eq!(decoded, value, "round trip failed for {value}");
        }
    }

    #[test]
    fn identical_vector_scores_highest() {
        let raw = encode(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let vectors = Vectors::new(raw, 4, 2).unwrap();
        let hits = vectors.top_k(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits[0].0, 0);
        assert!(hits[0].1 > hits[1].1);
    }

    #[test]
    fn rejects_a_query_of_the_wrong_width() {
        let vectors = Vectors::new(encode(&[1.0, 0.0]), 2, 1).unwrap();
        assert!(vectors.top_k(&[1.0, 0.0, 0.0], 1).is_err());
    }

    #[test]
    fn rejects_a_blob_that_does_not_match_its_declared_shape() {
        assert!(Vectors::new(vec![0u8; 10], 4, 2).is_err());
    }

    #[test]
    fn handles_a_dimension_that_is_not_a_multiple_of_four() {
        // The scan unrolls by four; the tail loop must still contribute.
        let raw = encode(&[1.0, 1.0, 1.0, 1.0, 1.0]);
        let vectors = Vectors::new(raw, 5, 1).unwrap();
        let hits = vectors.top_k(&[1.0, 1.0, 1.0, 1.0, 1.0], 1).unwrap();
        assert_eq!(hits[0].1, 5.0);
    }
}
