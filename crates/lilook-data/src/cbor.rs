//! Writing the sidecar a linked binary file becomes.
//!
//! CBOR because typst reads it natively and gets *native floats* back, with no
//! per-cell `float()` and no decisions about quoting or delimiters. A whole
//! encoder is about forty lines for the one shape needed here -- a map of names
//! to arrays of doubles -- which is less than the cost of a dependency and leaves
//! nothing to go wrong at a version bump.

use crate::Column;

/// A CBOR map from column name to array of `f64`, or -- for a two-dimensional
/// column -- to an array of rows.
///
/// The nesting is what lets a FITS image or a 2-D HDF5 dataset be linked at all:
/// lilaq's `colormesh`, `contour` and `mesh` take `z` as m rows of n values, so
/// a field has to arrive already shaped. Flattening it here would leave the user
/// to reshape it in the manuscript, which is exactly the interpreted arithmetic
/// the sidecar exists to avoid.
pub fn map_of_arrays(columns: &[Column]) -> Vec<u8> {
    let mut out = Vec::new();
    head(&mut out, 5, columns.len() as u64); // map
    for c in columns {
        head(&mut out, 3, c.name.len() as u64); // text string
        out.extend_from_slice(c.name.as_bytes());
        match c.grid {
            // Row-major, the same reading the probe gives a mesh's field.
            Some((cols, rows)) if cols > 0 && c.values.len() >= cols * rows => {
                head(&mut out, 4, rows as u64);
                for r in 0..rows {
                    head(&mut out, 4, cols as u64);
                    for v in &c.values[r * cols..(r + 1) * cols] {
                        double(&mut out, *v);
                    }
                }
            }
            _ => {
                head(&mut out, 4, c.values.len() as u64); // array
                for v in &c.values {
                    double(&mut out, *v);
                }
            }
        }
    }
    out
}

fn double(out: &mut Vec<u8>, v: f64) {
    out.push(0xfb); // IEEE 754 double
    out.extend_from_slice(&v.to_bits().to_be_bytes());
}

/// A CBOR item head: three bits of major type, then the argument in the smallest
/// of the five widths the format allows.
fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    match arg {
        0..=23 => out.push(m | arg as u8),
        24..=0xff => out.extend_from_slice(&[m | 24, arg as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, values: &[f64]) -> Column {
        Column {
            name: name.into(),
            values: values.to_vec(),
            grid: None,
        }
    }

    #[test]
    fn a_small_map_is_encoded_byte_for_byte() {
        let out = map_of_arrays(&[col("t", &[1.0])]);
        assert_eq!(
            out,
            [
                0xa1, // map(1)
                0x61, b't', // "t"
                0x81, // array(1)
                0xfb, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0, // 1.0
            ]
        );
        // Thirteen bytes for one named value: 1 map head, 2 name, 1 array head,
        // 9 for the double.
        assert_eq!(out.len(), 13);
    }

    /// The widths matter: a real column is longer than 23 and often longer than
    /// 255, and a head that lies about its length makes the file unreadable.
    #[test]
    fn every_length_width_is_used_correctly() {
        for n in [0usize, 1, 23, 24, 255, 256, 65535, 65536] {
            let values: Vec<f64> = (0..n).map(|i| i as f64).collect();
            let out = map_of_arrays(&[col("v", &values)]);
            // 3 bytes of map+name, then the array head, then 9 bytes per value.
            let head_len = match n {
                0..=23 => 1,
                24..=0xff => 2,
                0x100..=0xffff => 3,
                _ => 5,
            };
            assert_eq!(out.len(), 3 + head_len + 9 * n, "n = {n}");
        }
    }

    #[test]
    fn a_long_name_gets_a_wider_head_too() {
        let name = "x".repeat(300);
        let out = map_of_arrays(&[col(&name, &[])]);
        // map(1), then a 3-byte text head, the name, then array(0).
        assert_eq!(out.len(), 1 + 3 + 300 + 1);
        assert_eq!(out[1], (3 << 5) | 25);
    }
}
