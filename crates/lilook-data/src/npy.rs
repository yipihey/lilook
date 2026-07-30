//! NumPy's `.npy`, and the zip of them that is a `.npz`.
//!
//! Hand-rolled rather than taken from a crate, because the format is small and
//! the alternative pulls in `ndarray` and a zip implementation for the sake of
//! reading a header and a block of little-endian doubles. What is here is the
//! whole format as it appears in practice: the magic, a Python dict literal
//! giving dtype and shape, then raw values.

use crate::{take, Column, DataError, Dataset};

/// One `.npy` file: a single array, so a single column -- or several, if it is
/// two-dimensional, which is how people store a table in one array.
pub fn read_one(bytes: &[u8]) -> Result<Dataset, DataError> {
    let (dtype, shape, data) = header(bytes)?;
    let values = dtype.read_all(data)?;
    columns_from(values, &shape, "column")
}

/// A `.npz`: a zip archive whose members are `.npy` files, named after the
/// keyword they were saved under.
pub fn read_archive(bytes: &[u8]) -> Result<Dataset, DataError> {
    let members = crate::zip::members(bytes)?;
    if members.is_empty() {
        return Err(DataError::Malformed("an empty archive".into()));
    }
    let mut columns = vec![];
    for (name, contents) in members {
        // `np.savez` names members `key.npy`.
        let stem = name.strip_suffix(".npy").unwrap_or(&name).to_string();
        let Ok((dtype, shape, data)) = header(&contents) else {
            continue; // Something else in the archive; not an error.
        };
        let Ok(values) = dtype.read_all(data) else {
            continue;
        };
        // A member that is not numbers, or is too many dimensions to plot, is
        // skipped rather than fatal: an archive usually holds several things and
        // only some of them are columns.
        if let Ok(d) = columns_from(values, &shape, &stem) {
            for mut c in d.columns {
                // A 1-D member is the column itself and keeps the member's name;
                // a 2-D one is already named `stem[i]` by `columns_from`.
                if shape.len() == 1 {
                    c.name = stem.clone();
                }
                columns.push(c);
            }
        }
    }
    if columns.is_empty() {
        return Err(DataError::NoNumericColumns);
    }
    Ok(Dataset { columns })
}

/// Beyond this many columns a 2-D array is an image, not a table, and splitting
/// it into one column per pixel column helps nobody.
pub(crate) const MAX_SPLIT: usize = 32;

/// Split a flat array into columns according to its shape.
fn columns_from(values: Vec<f64>, shape: &[usize], stem: &str) -> Result<Dataset, DataError> {
    match shape {
        // A scalar is a one-value column, which is a legitimate thing to plot.
        [] | [_] => Ok(Dataset {
            columns: vec![Column::new(stem, values)],
        }),
        // Rows by columns, C order. It comes back whole, as a field a mesh can
        // take, *and* split into columns -- because a 2-D array is a table about
        // as often as it is an image and the file does not say which.
        //
        // The split is capped: past 32 columns this is an image, and offering
        // 512 one-pixel-wide columns would bury the entry anyone wanted.
        [rows, cols] => {
            if values.len() != rows * cols {
                return Err(DataError::Malformed(format!(
                    "shape says {rows}x{cols} but there are {} values",
                    values.len()
                )));
            }
            let mut columns = vec![Column::field(stem, values.clone(), *cols, *rows)];
            if *cols <= MAX_SPLIT {
                columns.extend((0..*cols).map(|c| {
                    Column::new(
                        format!("{stem}{}", c + 1),
                        (0..*rows).map(|r| values[r * cols + c]).collect(),
                    )
                }));
            }
            Ok(Dataset { columns })
        }
        _ => Err(DataError::Unsupported(format!(
            "{}-dimensional data has no obvious columns",
            shape.len()
        ))),
    }
}

/// The magic, the version, the dict, and where the values start.
fn header(bytes: &[u8]) -> Result<(Dtype, Vec<usize>, &[u8]), DataError> {
    if !bytes.starts_with(b"\x93NUMPY") {
        return Err(DataError::Malformed("not a .npy file".into()));
    }
    let major = *bytes.get(6).ok_or_else(truncated)?;
    // v1 counts the header in two bytes, v2 and v3 in four.
    let (len, at) = match major {
        1 => {
            let b = take(bytes, 8, 2)?;
            (u16::from_le_bytes([b[0], b[1]]) as usize, 10)
        }
        2 | 3 => {
            let b = take(bytes, 8, 4)?;
            (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize, 12)
        }
        other => {
            return Err(DataError::Unsupported(format!(
                ".npy version {other} is newer than this reader"
            )))
        }
    };
    let dict = core::str::from_utf8(take(bytes, at, len)?)
        .map_err(|_| DataError::Malformed("the header is not utf-8".into()))?;
    let descr = field(dict, "descr")
        .ok_or_else(|| DataError::Malformed("no dtype in the header".into()))?;
    let dtype = Dtype::parse(&descr)?;
    if field(dict, "fortran_order").as_deref() == Some("True") {
        return Err(DataError::Unsupported(
            "column-major (Fortran order) arrays are not read yet".into(),
        ));
    }
    let shape = shape_of(dict)?;
    Ok((dtype, shape, &bytes[at + len..]))
}

fn truncated() -> DataError {
    DataError::Malformed("the file ends inside its header".into())
}

/// A value out of the Python dict literal the header is.
///
/// Not a Python parser: the header is machine-written and always
/// `{'descr': '<f8', 'fortran_order': False, 'shape': (3,), }`, so finding the
/// key and taking what follows is enough, and anything unexpected fails cleanly
/// further along.
fn field(dict: &str, key: &str) -> Option<String> {
    let at = dict.find(&format!("'{key}'"))?;
    let rest = dict[at + key.len() + 2..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if let Some(q) = rest.strip_prefix('\'') {
        return Some(q[..q.find('\'')?].to_string());
    }
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

fn shape_of(dict: &str) -> Result<Vec<usize>, DataError> {
    let at = dict
        .find("'shape'")
        .ok_or_else(|| DataError::Malformed("no shape in the header".into()))?;
    let rest = &dict[at..];
    let open = rest
        .find('(')
        .ok_or_else(|| DataError::Malformed("malformed shape".into()))?;
    let close = rest[open..]
        .find(')')
        .ok_or_else(|| DataError::Malformed("malformed shape".into()))?
        + open;
    rest[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| DataError::Malformed(format!("{s:?} is not a dimension")))
        })
        .collect()
}

/// The numeric types worth reading. Everything becomes `f64`, because that is
/// what a figure plots.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dtype {
    kind: char,
    size: usize,
    big_endian: bool,
}

impl Dtype {
    fn parse(descr: &str) -> Result<Dtype, DataError> {
        let mut chars = descr.chars();
        let first = chars
            .next()
            .ok_or_else(|| DataError::Malformed("empty dtype".into()))?;
        let (big_endian, kind) = match first {
            '<' | '=' | '|' => (false, chars.next()),
            '>' => (true, chars.next()),
            k => (false, Some(k)),
        };
        let kind = kind.ok_or_else(|| DataError::Malformed("empty dtype".into()))?;
        let size: usize = chars.as_str().parse().unwrap_or(1);
        if !matches!(kind, 'f' | 'i' | 'u' | 'b') {
            return Err(DataError::Unsupported(format!(
                "dtype {descr:?} is not numeric"
            )));
        }
        if kind == 'f' && !matches!(size, 4 | 8) {
            return Err(DataError::Unsupported(format!(
                "{}-byte floats are not read",
                size
            )));
        }
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(DataError::Unsupported(format!("{size}-byte values")));
        }
        Ok(Dtype {
            kind,
            size,
            big_endian,
        })
    }

    fn read_all(self, data: &[u8]) -> Result<Vec<f64>, DataError> {
        if !data.len().is_multiple_of(self.size) {
            return Err(DataError::Malformed(format!(
                "{} bytes is not a whole number of {}-byte values",
                data.len(),
                self.size
            )));
        }
        Ok(data.chunks_exact(self.size).map(|c| self.one(c)).collect())
    }

    fn one(self, c: &[u8]) -> f64 {
        let mut b = [0u8; 8];
        b[..c.len()].copy_from_slice(c);
        if self.big_endian {
            b[..c.len()].reverse();
        }
        let u = u64::from_le_bytes(b);
        match (self.kind, self.size) {
            ('f', 4) => f32::from_bits(u as u32) as f64,
            ('f', 8) => f64::from_bits(u),
            ('u', _) | ('b', _) => u as f64,
            ('i', 1) => (u as u8 as i8) as f64,
            ('i', 2) => (u as u16 as i16) as f64,
            ('i', 4) => (u as u32 as i32) as f64,
            ('i', _) => (u as i64) as f64,
            _ => f64::NAN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `.npy` the way numpy does, so the reader is tested against the
    /// real layout rather than against itself.
    fn npy(descr: &str, shape: &str, data: &[u8]) -> Vec<u8> {
        let dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
        let mut header = dict.into_bytes();
        // numpy pads the header so the data starts on a 64-byte boundary.
        while !(10 + header.len()).is_multiple_of(64) {
            header.push(b' ');
        }
        header.push(b'\n');
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out
    }

    fn f64s(values: &[f64]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn a_one_dimensional_float_array_is_one_column() {
        let file = npy("<f8", "(3,)", &f64s(&[1.5, -2.0, 1e-9]));
        let d = read_one(&file).unwrap();
        assert_eq!(d.names(), ["column"]);
        assert_eq!(d.columns[0].values, [1.5, -2.0, 1e-9]);
    }

    #[test]
    fn every_numeric_dtype_reads_as_the_number_it_is() {
        let cases: Vec<(&str, Vec<u8>, Vec<f64>)> = vec![
            ("<f4", 1.5f32.to_le_bytes().to_vec(), vec![1.5]),
            (">f8", 2.5f64.to_be_bytes().to_vec(), vec![2.5]),
            ("<i4", (-7i32).to_le_bytes().to_vec(), vec![-7.0]),
            (">i2", (-300i16).to_be_bytes().to_vec(), vec![-300.0]),
            ("<u2", 40000u16.to_le_bytes().to_vec(), vec![40000.0]),
            ("|i1", vec![0xff], vec![-1.0]),
            ("|u1", vec![0xff], vec![255.0]),
            ("|b1", vec![1], vec![1.0]),
            ("<i8", (-1i64).to_le_bytes().to_vec(), vec![-1.0]),
        ];
        for (descr, data, want) in cases {
            let file = npy(descr, "(1,)", &data);
            let d = read_one(&file).unwrap_or_else(|e| panic!("{descr}: {e}"));
            assert_eq!(d.columns[0].values, want, "{descr}");
        }
    }

    #[test]
    fn a_two_dimensional_array_is_both_a_field_and_its_columns() {
        // Two rows of three, C order.
        let file = npy("<f8", "(2, 3)", &f64s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let d = read_one(&file).unwrap();
        // The whole thing first, keeping its shape, because that is what a mesh
        // takes and a mesh is the only thing that can take it.
        assert_eq!(d.names(), ["column", "column1", "column2", "column3"]);
        assert_eq!(d.columns[0].grid, Some((3, 2)));
        assert_eq!(d.columns[0].values, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        // Then the columns, for when it was a table after all.
        assert_eq!(d.columns[1].values, [1.0, 4.0]);
        assert_eq!(d.columns[3].values, [3.0, 6.0]);
        assert!(d.columns[1].grid.is_none());
    }

    /// Past the cap, a 2-D array is an image and only the field comes back.
    #[test]
    fn a_wide_array_is_not_split_into_a_column_per_pixel() {
        let cols = MAX_SPLIT + 1;
        let file = npy("<f8", &format!("(2, {cols})"), &f64s(&vec![0.0; 2 * cols]));
        let d = read_one(&file).unwrap();
        assert_eq!(d.columns.len(), 1, "the field alone");
        assert_eq!(d.columns[0].grid, Some((cols, 2)));
    }

    #[test]
    fn what_cannot_be_read_says_so_rather_than_guessing() {
        assert!(matches!(
            read_one(b"not a npy file at all"),
            Err(DataError::Malformed(_))
        ));
        // Strings are a real numpy dtype and not a column of numbers.
        let file = npy("<U4", "(1,)", &[0; 16]);
        assert!(matches!(read_one(&file), Err(DataError::Unsupported(_))));
        // Three dimensions have no obvious columns.
        let file = npy("<f8", "(2, 2, 2)", &f64s(&[0.0; 8]));
        assert!(matches!(read_one(&file), Err(DataError::Unsupported(_))));
        // Fortran order is refused rather than silently transposed.
        let dict = "{'descr': '<f8', 'fortran_order': True, 'shape': (2, 2), }";
        let mut file = b"\x93NUMPY\x01\x00".to_vec();
        file.extend_from_slice(&(dict.len() as u16).to_le_bytes());
        file.extend_from_slice(dict.as_bytes());
        file.extend_from_slice(&f64s(&[0.0; 4]));
        assert!(matches!(read_one(&file), Err(DataError::Unsupported(_))));
        // A truncated file is not a panic.
        assert!(read_one(b"\x93NUMPY\x01\x00").is_err());
        assert!(read_one(b"\x93NUMPY").is_err());
    }

    #[test]
    fn a_version_2_header_is_read_too() {
        let dict = "{'descr': '<f8', 'fortran_order': False, 'shape': (2,), }";
        let mut file = b"\x93NUMPY\x02\x00".to_vec();
        file.extend_from_slice(&(dict.len() as u32).to_le_bytes());
        file.extend_from_slice(dict.as_bytes());
        file.extend_from_slice(&f64s(&[3.0, 4.0]));
        assert_eq!(read_one(&file).unwrap().columns[0].values, [3.0, 4.0]);
    }
}
