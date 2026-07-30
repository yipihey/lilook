//! Reading the data formats a figure cannot read for itself.
//!
//! Typst can read `csv`, `json`, `toml`, `yaml`, `cbor` and `xml`, and a figure
//! that links one of those needs nothing from this crate: the document says
//! `csv("run.csv")` and the compiler does the rest. HDF5, npz and FITS it cannot
//! read at all, and Veusz's descriptor ASCII it cannot read *usefully* -- doing
//! it in typst would mean a five-line interpreted parser inside the user's
//! manuscript.
//!
//! So those four are decoded here and transcoded to a **CBOR sidecar** the
//! document links instead. That keeps the link live where it matters: the
//! document reads a file, so refreshing is a recompile rather than an edit to the
//! buffer, and the undo history never has to know. It also slices -- a 2 GB HDF5
//! file yields a few KB of the two columns actually plotted, where typst's
//! `read()` would have to load and hash the whole thing on every compile.
//!
//! Every decoder takes `&[u8]`. Nothing here opens a file, so the whole crate
//! builds for `wasm32-unknown-unknown` with no feature gate and the browser gets
//! the same formats the desktop does -- except HDF5, which is C.

#![forbid(unsafe_code)]

#[cfg(feature = "ascii")]
pub mod ascii;
pub mod cbor;
#[cfg(feature = "fits")]
pub mod fits;
#[cfg(all(feature = "hdf5", not(target_arch = "wasm32")))]
pub mod hdf5;
#[cfg(feature = "npz")]
pub mod npy;
#[cfg(feature = "npz")]
pub mod zip;

/// One named array of numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub values: Vec<f64>,
    /// `Some((columns, rows))` when this is a two-dimensional *field* rather
    /// than a column, with `values` flattened row-major.
    ///
    /// A FITS image and a 2-D HDF5 dataset are both fields, and a field is only
    /// linkable to a mesh -- so the shape has to survive decoding. Without it an
    /// image arrives as one very long column and the user is left to reshape it
    /// by hand in the manuscript.
    pub grid: Option<(usize, usize)>,
}

impl Column {
    pub fn new(name: impl Into<String>, values: Vec<f64>) -> Column {
        Column {
            name: name.into(),
            values,
            grid: None,
        }
    }

    /// The same, shaped as `columns` by `rows`.
    pub fn field(name: impl Into<String>, values: Vec<f64>, cols: usize, rows: usize) -> Column {
        Column {
            grid: Some((cols, rows)),
            ..Column::new(name, values)
        }
    }
}

/// What a decoder found: named columns, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dataset {
    pub columns: Vec<Column>,
}

impl Dataset {
    pub fn names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The sidecar the document will link, in CBOR: a map of name to array of
    /// native floats. Chosen over CSV because typst reads it back without a
    /// per-cell `float()` and without deciding anything about quoting.
    pub fn to_cbor(&self) -> Vec<u8> {
        cbor::map_of_arrays(&self.columns)
    }

    /// Just these columns, in the order asked for. This is the slicing that
    /// makes a sidecar smaller than its origin.
    pub fn select(&self, names: &[String]) -> Dataset {
        Dataset {
            columns: names
                .iter()
                .filter_map(|n| self.column(n).cloned())
                .collect(),
        }
    }
}

/// The formats lilook decodes itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Npy,
    Npz,
    Fits,
    /// Veusz's descriptor ASCII, and plain numeric columns.
    Ascii,
    Hdf5,
}

impl Format {
    /// The extension a file of this format usually has.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Npy => &["npy"],
            Format::Npz => &["npz"],
            Format::Fits => &["fits", "fit", "fts"],
            Format::Ascii => &["dat", "txt", "tsv", "asc"],
            Format::Hdf5 => &["h5", "hdf5", "he5", "hdf"],
        }
    }

    /// Is this format available in this build? HDF5 needs a C library that does
    /// not exist for wasm32, so the honest answer differs per target.
    pub fn available(self) -> bool {
        match self {
            Format::Npy | Format::Npz => cfg!(feature = "npz"),
            Format::Fits => cfg!(feature = "fits"),
            Format::Ascii => cfg!(feature = "ascii"),
            Format::Hdf5 => cfg!(all(feature = "hdf5", not(target_arch = "wasm32"))),
        }
    }

    /// Why a format is not available, in words a panel can show.
    pub fn unavailable_because(self) -> &'static str {
        match self {
            Format::Hdf5 if cfg!(target_arch = "wasm32") => {
                "HDF5 needs libhdf5, which is a C library with no wasm build. \
                 In a browser, convert the file first -- npz, FITS and CSV all work here."
            }
            Format::Hdf5 => "this build was made without the `hdf5` feature",
            _ => "this build was made without that format's feature",
        }
    }
}

/// Which format a file is, from its contents first and its name second.
///
/// Contents first because an extension is a claim and a magic number is
/// evidence: `.dat` is used for everything, and a FITS file is often `.fit`.
pub fn sniff(bytes: &[u8], name: &str) -> Option<Format> {
    if bytes.starts_with(b"\x93NUMPY") {
        return Some(Format::Npy);
    }
    // A zip. Could be anything zipped, but in this context it is an npz.
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some(Format::Npz);
    }
    if bytes.starts_with(b"\x89HDF\r\n\x1a\n") {
        return Some(Format::Hdf5);
    }
    // FITS' first card is always `SIMPLE  =` for a primary header.
    if bytes.starts_with(b"SIMPLE  =") || bytes.starts_with(b"XTENSION=") {
        return Some(Format::Fits);
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    [
        Format::Npy,
        Format::Npz,
        Format::Fits,
        Format::Ascii,
        Format::Hdf5,
    ]
    .into_iter()
    .find(|f| f.extensions().contains(&ext.as_str()))
}

/// Decode a file into named columns.
pub fn decode(bytes: &[u8], format: Format) -> Result<Dataset, DataError> {
    if !format.available() {
        return Err(DataError::Unavailable(format));
    }
    match format {
        #[cfg(feature = "npz")]
        Format::Npy => npy::read_one(bytes),
        #[cfg(feature = "npz")]
        Format::Npz => npy::read_archive(bytes),
        #[cfg(feature = "fits")]
        Format::Fits => fits::read(bytes),
        #[cfg(feature = "ascii")]
        Format::Ascii => ascii::read(core::str::from_utf8(bytes).map_err(|_| {
            DataError::Malformed("this is not text, so it is not an ASCII table".into())
        })?),
        #[cfg(all(feature = "hdf5", not(target_arch = "wasm32")))]
        Format::Hdf5 => hdf5::read(bytes),
        #[allow(unreachable_patterns)]
        other => Err(DataError::Unavailable(other)),
    }
}

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq)]
pub enum DataError {
    /// The format is real, but this build cannot read it.
    Unavailable(Format),
    /// The bytes are not what they claim to be.
    Malformed(String),
    /// Readable, but nothing in it is a column of numbers.
    NoNumericColumns,
    /// A shape lilook has no way to plot, named so the message can say which.
    Unsupported(String),
}

impl core::fmt::Display for DataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DataError::Unavailable(fmt) => {
                write!(
                    f,
                    "{:?} is not available: {}",
                    fmt,
                    fmt.unavailable_because()
                )
            }
            DataError::Malformed(why) => write!(f, "{why}"),
            DataError::NoNumericColumns => {
                write!(f, "no columns of numbers in this file")
            }
            DataError::Unsupported(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for DataError {}

/// Read `n` bytes at `at`, or say the file is truncated.
pub(crate) fn take(bytes: &[u8], at: usize, n: usize) -> Result<&[u8], DataError> {
    bytes
        .get(at..at + n)
        .ok_or_else(|| DataError::Malformed(format!("wanted {n} bytes at {at}, the file ends")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_are_believed_before_extensions() {
        assert_eq!(sniff(b"\x93NUMPY\x01\x00", "run.dat"), Some(Format::Npy));
        assert_eq!(sniff(b"PK\x03\x04rest", "run.dat"), Some(Format::Npz));
        assert_eq!(
            sniff(b"\x89HDF\r\n\x1a\nrest", "run.dat"),
            Some(Format::Hdf5)
        );
        assert_eq!(
            sniff(b"SIMPLE  =                    T", "x"),
            Some(Format::Fits)
        );

        // No magic to go on: fall back to the name.
        assert_eq!(sniff(b"0 1\n2 3\n", "run.dat"), Some(Format::Ascii));
        assert_eq!(sniff(b"anything", "run.FITS"), Some(Format::Fits));
        assert_eq!(sniff(b"anything", "run.h5"), Some(Format::Hdf5));
        assert_eq!(sniff(b"anything", "run.weird"), None);
        assert_eq!(sniff(b"anything", "noextension"), None);
    }

    #[test]
    fn a_missing_format_says_which_and_why() {
        let e = DataError::Unavailable(Format::Hdf5);
        assert!(e.to_string().contains("Hdf5"));
        // Whatever the build, the reason names something actionable.
        assert!(!Format::Hdf5.unavailable_because().is_empty());
    }

    #[test]
    fn selecting_columns_is_what_makes_a_sidecar_small() {
        let d = Dataset {
            columns: vec![
                Column::new("t", vec![0.0, 1.0]),
                Column::new("big", (0..1000).map(f64::from).collect()),
                Column::new("y", vec![2.0, 3.0]),
            ],
        };
        let picked = d.select(&["t".into(), "y".into()]);
        assert_eq!(picked.names(), ["t", "y"]);
        assert!(picked.to_cbor().len() * 20 < d.to_cbor().len());
        // Asking for something absent drops it rather than inventing it.
        assert!(d.select(&["nope".into()]).columns.is_empty());
    }
}
