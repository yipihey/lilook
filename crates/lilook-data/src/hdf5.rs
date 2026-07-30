//! HDF5, on the platforms that can have it.
//!
//! The exception to this crate's rule. Every other decoder takes `&[u8]`, which
//! is what makes them portable; HDF5 means libhdf5, a C library whose API is
//! built around a *file path*, and which has no wasm build. So this module takes
//! a path, is `cfg`'d off for wasm32, and sits behind an off-by-default feature
//! so that `cargo install lilook` needs no C toolchain and the three wasm32
//! checks in `scripts/check.sh` are unaffected.
//!
//! There is no pure-Rust HDF5 reader to use instead, and writing one is not a
//! subset problem that can be time-boxed: the superblock, B-tree v1 and v2
//! indexing, chunked layouts and the shuffle+gzip filter pipeline are each
//! individually larger than the rest of this feature, and h5py's defaults
//! exercise most of them.
//!
//! In the browser the answer is different: `lilook-web` reads HDF5 through
//! h5wasm in JavaScript and hands the decoded columns back, so the format works
//! there without this module existing.

use std::path::Path;

use crate::{Column, DataError, Dataset};

/// The binding itself, re-exported so a test can *write* a file with libhdf5 and
/// read it back with this module -- which checks the reader against a real file
/// rather than against itself. Cargo will not take the same crate under two
/// names, so this is how the test gets at it.
pub use ::hdf5 as lib;

/// Every numeric dataset in the file, flattened to columns.
///
/// Nested groups are walked, and a dataset's full path becomes its name, so
/// `/results/t` is distinguishable from `/reference/t`.
pub fn read_path(path: &Path) -> Result<Dataset, DataError> {
    let file = hdf5::File::open(path).map_err(|e| DataError::Malformed(e.to_string()))?;
    let mut columns = vec![];
    walk(&file.group("/").map_err(hdf5_err)?, "", &mut columns);
    if columns.is_empty() {
        return Err(DataError::NoNumericColumns);
    }
    Ok(Dataset { columns })
}

/// The `&[u8]` form every other format has. HDF5 does not: libhdf5 wants a path.
pub fn read(_bytes: &[u8]) -> Result<Dataset, DataError> {
    Err(DataError::Unsupported(
        "HDF5 is read from a path rather than from bytes, because libhdf5 is".into(),
    ))
}

fn hdf5_err(e: hdf5::Error) -> DataError {
    DataError::Malformed(e.to_string())
}

/// Depth-first through the groups. A dataset that cannot be read as numbers is
/// skipped rather than fatal: a real file holds strings and metadata too, and
/// only some of it is a column.
fn walk(group: &hdf5::Group, prefix: &str, out: &mut Vec<Column>) {
    let Ok(names) = group.member_names() else {
        return;
    };
    for name in names {
        let path = format!("{prefix}/{name}");
        if let Ok(sub) = group.group(&name) {
            walk(&sub, &path, out);
            continue;
        }
        let Ok(ds) = group.dataset(&name) else {
            continue;
        };
        // Only one- and two-dimensional data has obvious columns; anything more
        // is an image cube or worse, and guessing would be wrong.
        let shape = ds.shape();
        if shape.is_empty() || shape.len() > 2 {
            continue;
        }
        if let Some(values) = numbers(&ds) {
            match shape.as_slice() {
                [_] => out.push(Column::new(path, values)),
                // Whole, as a field a mesh can take, and also split into
                // columns -- a 2-D dataset is a table about as often as it is an
                // image, and the file does not say which. The split is capped so
                // a 512-pixel-wide image does not bury the field itself.
                [rows, cols] => {
                    out.push(Column::field(path.clone(), values.clone(), *cols, *rows));
                    if *cols <= crate::npy::MAX_SPLIT {
                        for c in 0..*cols {
                            out.push(Column::new(
                                format!("{path}[{c}]"),
                                (0..*rows).map(|r| values[r * cols + c]).collect(),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Read a dataset as `f64`, whatever width it was stored at.
///
/// `read_raw` requires the requested type to match the file's exactly, so every
/// numeric width is tried rather than hoping for doubles. Anything else -- a
/// string, a compound type, an enum -- returns `None` and is skipped.
fn numbers(ds: &hdf5::Dataset) -> Option<Vec<f64>> {
    macro_rules! try_as {
        ($($t:ty),*) => {
            $(
                if let Ok(v) = ds.read_raw::<$t>() {
                    return Some(v.into_iter().map(|x| x as f64).collect());
                }
            )*
        };
    }
    try_as!(f64, f32, i64, i32, i16, i8, u64, u32, u16, u8);
    None
}
