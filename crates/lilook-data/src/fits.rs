//! FITS: the format most of astronomy's data is in.
//!
//! Hand-rolled, because the mature readers wrap cfitsio and cfitsio is C -- which
//! rules out the browser. The format itself is unusually kind to this: 2880-byte
//! header blocks of eighty-column `KEYWORD = value` cards, then big-endian data
//! whose layout the cards fully describe.
//!
//! What is read: image HDUs of any `BITPIX`, and `BINTABLE` columns of numeric
//! `TFORM`. `BSCALE`/`BZERO` are applied, which matters -- unsigned 16-bit data
//! is conventionally stored as signed with `BZERO = 32768`, so ignoring it would
//! halve every value and wrap the top half negative.

use crate::{take, Column, DataError, Dataset};

const BLOCK: usize = 2880;
const CARD: usize = 80;

/// Every HDU's numeric contents, as columns.
pub fn read(bytes: &[u8]) -> Result<Dataset, DataError> {
    let mut columns = vec![];
    let mut at = 0;
    let mut hdu = 0;
    while at + BLOCK <= bytes.len() {
        let (cards, data_at) = header(bytes, at)?;
        let naxis = int(&cards, "NAXIS").unwrap_or(0).max(0) as usize;
        let dims: Vec<usize> = (1..=naxis)
            .map(|i| int(&cards, &format!("NAXIS{i}")).unwrap_or(0).max(0) as usize)
            .collect();
        let bitpix = int(&cards, "BITPIX").unwrap_or(0);
        // `NAXIS = 0` is a header with no data -- the usual shape of a primary
        // header before an extension. The product of no dimensions is 1, not 0,
        // so this cannot be left to `product()`.
        let data_len = if naxis == 0 || dims.contains(&0) {
            0
        } else {
            dims.iter().product::<usize>() * (bitpix.unsigned_abs() as usize / 8)
        };

        let kind = text(&cards, "XTENSION").unwrap_or_default();
        if kind == "BINTABLE" || kind == "TABLE" {
            if kind == "BINTABLE" {
                columns.extend(bintable(bytes, data_at, &cards, &dims)?);
            }
        } else if data_len > 0 {
            // An image. A 1-D one is a spectrum and reads as a column; a 2-D one
            // is a field and keeps its shape, so it can be linked to a mesh
            // rather than arriving as one very long column. Anything higher --
            // a cube -- is flattened, because guessing which plane was wanted
            // would be worse than saying nothing.
            let raw = take(bytes, data_at, data_len)?;
            let scale = float(&cards, "BSCALE").unwrap_or(1.0);
            let zero = float(&cards, "BZERO").unwrap_or(0.0);
            let values = read_values(raw, bitpix)?
                .into_iter()
                .map(|v| v * scale + zero)
                .collect();
            let name = text(&cards, "EXTNAME").unwrap_or_else(|| format!("hdu{hdu}"));
            // `NAXIS1` varies fastest, so the bytes are already row-major with
            // `NAXIS1` columns -- the reading lilook uses everywhere else.
            columns.push(match dims[..] {
                [cols, rows] => Column::field(name, values, cols, rows),
                _ => Column::new(name, values),
            });
        }

        // Every section is padded to a whole number of 2880-byte blocks.
        at = data_at + data_len.div_ceil(BLOCK) * BLOCK;
        hdu += 1;
        if data_len == 0 && kind.is_empty() && hdu > 1 {
            break; // Nothing further to find.
        }
    }
    if columns.is_empty() {
        return Err(DataError::NoNumericColumns);
    }
    Ok(Dataset { columns })
}

/// One HDU's cards, and where its data starts.
fn header(bytes: &[u8], at: usize) -> Result<(Vec<(String, String)>, usize), DataError> {
    let mut cards = vec![];
    let mut pos = at;
    loop {
        let block = take(bytes, pos, BLOCK)?;
        for card in block.chunks_exact(CARD) {
            let card = String::from_utf8_lossy(card);
            let key = card[..8.min(card.len())].trim().to_string();
            if key == "END" {
                return Ok((cards, pos + BLOCK));
            }
            if key.is_empty() || !card[8..].starts_with('=') {
                continue; // A comment, or a continuation.
            }
            // Strip the trailing comment, but not a `/` inside a quoted string.
            let value = card[9..].trim();
            let mut cut = value.len();
            let mut in_string = false;
            for (i, c) in value.char_indices() {
                match c {
                    '\'' => in_string = !in_string,
                    '/' if !in_string => {
                        cut = i;
                        break;
                    }
                    _ => {}
                }
            }
            cards.push((key, value[..cut].trim().to_string()));
        }
        pos += BLOCK;
    }
}

fn card<'a>(cards: &'a [(String, String)], key: &str) -> Option<&'a str> {
    cards
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn int(cards: &[(String, String)], key: &str) -> Option<i64> {
    card(cards, key)?.parse().ok()
}

fn float(cards: &[(String, String)], key: &str) -> Option<f64> {
    // FITS allows Fortran's `D` exponent.
    card(cards, key)?.replace(['D', 'd'], "e").parse().ok()
}

fn text(cards: &[(String, String)], key: &str) -> Option<String> {
    let v = card(cards, key)?.trim();
    Some(v.trim_matches('\'').trim().to_string())
}

/// Read a block of big-endian values of the width `BITPIX` names.
fn read_values(raw: &[u8], bitpix: i64) -> Result<Vec<f64>, DataError> {
    let width = bitpix.unsigned_abs() as usize / 8;
    if width == 0 || !raw.len().is_multiple_of(width) {
        return Err(DataError::Malformed(format!(
            "BITPIX {bitpix} does not divide {} bytes",
            raw.len()
        )));
    }
    Ok(raw
        .chunks_exact(width)
        .map(|c| one(c, bitpix))
        .collect::<Vec<f64>>())
}

/// One value, big-endian, as `BITPIX` describes it. 8-bit is unsigned; every
/// other integer width is signed; negative `BITPIX` means floating point.
fn one(c: &[u8], bitpix: i64) -> f64 {
    match bitpix {
        8 => c[0] as f64,
        16 => i16::from_be_bytes([c[0], c[1]]) as f64,
        32 => i32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64,
        64 => i64::from_be_bytes(c.try_into().unwrap()) as f64,
        -32 => f32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64,
        -64 => f64::from_be_bytes(c.try_into().unwrap()),
        _ => f64::NAN,
    }
}

/// A `BINTABLE`'s numeric columns.
///
/// `NAXIS1` is the byte width of a row and `NAXIS2` the number of rows, so each
/// column is a fixed offset into every row.
fn bintable(
    bytes: &[u8],
    data_at: usize,
    cards: &[(String, String)],
    dims: &[usize],
) -> Result<Vec<Column>, DataError> {
    let (row_bytes, rows) = match dims {
        [w, h, ..] => (*w, *h),
        _ => return Ok(vec![]),
    };
    let fields = int(cards, "TFIELDS").unwrap_or(0).max(0) as usize;
    let mut out = vec![];
    let mut offset = 0usize;
    for f in 1..=fields {
        let form = text(cards, &format!("TFORM{f}")).unwrap_or_default();
        let name = text(cards, &format!("TTYPE{f}")).unwrap_or_else(|| format!("col{f}"));
        let scale = float(cards, &format!("TSCAL{f}")).unwrap_or(1.0);
        let zero = float(cards, &format!("TZERO{f}")).unwrap_or(0.0);
        let (repeat, code) = parse_tform(&form);
        let width = tform_width(code);
        let field_bytes = repeat * width;

        // Only single-valued numeric fields become columns. A 10-character name
        // field is data, but it is not a number, and a vector field has no one
        // value per row to plot.
        if width > 0 && repeat == 1 {
            let bitpix = tform_bitpix(code);
            let mut values = Vec::with_capacity(rows);
            for r in 0..rows {
                let at = data_at + r * row_bytes + offset;
                let c = take(bytes, at, width)?;
                values.push(one(c, bitpix) * scale + zero);
            }
            out.push(Column::new(name, values));
        }
        offset += field_bytes;
    }
    Ok(out)
}

/// `TFORM` is `rT`: an optional repeat count then a one-letter type code.
fn parse_tform(form: &str) -> (usize, char) {
    let digits: String = form.chars().take_while(|c| c.is_ascii_digit()).collect();
    let code = form[digits.len()..].chars().next().unwrap_or('?');
    let repeat = digits.parse().unwrap_or(1);
    (repeat, code)
}

/// Bytes per element for a `TFORM` code; 0 for anything not read as a number.
fn tform_width(code: char) -> usize {
    match code {
        'B' | 'L' => 1,
        'I' => 2,
        'J' | 'E' => 4,
        'K' | 'D' => 8,
        _ => 0, // A: characters, X: bits, C/M: complex, P/Q: array descriptors.
    }
}

/// The `BITPIX` that decodes a `TFORM` code.
fn tform_bitpix(code: char) -> i64 {
    match code {
        'B' | 'L' => 8,
        'I' => 16,
        'J' => 32,
        'K' => 64,
        'E' => -32,
        'D' => -64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a FITS file the way the standard describes, so the reader is tested
    /// against the layout rather than against itself.
    fn fits(cards: &[&str], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in cards {
            out.extend_from_slice(format!("{c:<80}").as_bytes());
        }
        out.extend_from_slice(format!("{:<80}", "END").as_bytes());
        while !out.len().is_multiple_of(BLOCK) {
            out.push(b' ');
        }
        out.extend_from_slice(data);
        while !out.len().is_multiple_of(BLOCK) {
            out.push(0);
        }
        out
    }

    #[test]
    fn a_one_dimensional_image_reads_as_a_column() {
        let data: Vec<u8> = [1.5f64, -2.5, 3.0]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let file = fits(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                  -64",
                "NAXIS   =                    1",
                "NAXIS1  =                    3",
            ],
            &data,
        );
        let d = read(&file).unwrap();
        assert_eq!(d.columns.len(), 1);
        assert_eq!(d.columns[0].values, [1.5, -2.5, 3.0]);
    }

    /// The convention that makes unsigned 16-bit data readable at all. Getting
    /// this wrong halves every value and wraps the top half negative.
    #[test]
    fn bscale_and_bzero_are_applied() {
        let data: Vec<u8> = [(-32768i16), 0, 32767]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let file = fits(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                   16",
                "NAXIS   =                    1",
                "NAXIS1  =                    3",
                "BZERO   =                32768",
                "BSCALE  =                    1",
            ],
            &data,
        );
        let d = read(&file).unwrap();
        assert_eq!(d.columns[0].values, [0.0, 32768.0, 65535.0]);
    }

    #[test]
    fn a_bintables_numeric_columns_are_read_by_name() {
        // Three rows of: J (i32) time, E (f32) flux, 4A name.
        let mut data = Vec::new();
        for (t, f, n) in [
            (1i32, 1.5f32, b"aaaa"),
            (2, 2.5, b"bbbb"),
            (3, 3.5, b"cccc"),
        ] {
            data.extend_from_slice(&t.to_be_bytes());
            data.extend_from_slice(&f.to_be_bytes());
            data.extend_from_slice(n);
        }
        // A primary header with no data, then the extension.
        let mut file = fits(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                    8",
                "NAXIS   =                    0",
            ],
            &[],
        );
        file.extend_from_slice(&fits(
            &[
                "XTENSION= 'BINTABLE'",
                "BITPIX  =                    8",
                "NAXIS   =                    2",
                "NAXIS1  =                   12",
                "NAXIS2  =                    3",
                "TFIELDS =                    3",
                "TFORM1  = 'J       '",
                "TTYPE1  = 'time    '",
                "TFORM2  = 'E       '",
                "TTYPE2  = 'flux    '",
                "TFORM3  = '4A      '",
                "TTYPE3  = 'name    '",
            ],
            &data,
        ));

        let d = read(&file).unwrap();
        assert_eq!(
            d.names(),
            ["time", "flux"],
            "the text column is not a number"
        );
        assert_eq!(d.column("time").unwrap().values, [1.0, 2.0, 3.0]);
        assert_eq!(d.column("flux").unwrap().values, [1.5, 2.5, 3.5]);
    }

    #[test]
    fn a_comment_containing_a_slash_in_a_string_is_not_cut_short() {
        let file = fits(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                    8",
                "NAXIS   =                    1",
                "NAXIS1  =                    2",
                "EXTNAME = 'a/b'    / a name with a slash in it",
            ],
            &[1, 2],
        );
        let d = read(&file).unwrap();
        assert_eq!(d.columns[0].name, "a/b");
        assert_eq!(d.columns[0].values, [1.0, 2.0]);
    }

    #[test]
    fn tform_is_parsed_into_a_repeat_and_a_code() {
        assert_eq!(parse_tform("J"), (1, 'J'));
        assert_eq!(parse_tform("1D"), (1, 'D'));
        assert_eq!(parse_tform("16A"), (16, 'A'));
        assert_eq!(parse_tform(""), (1, '?'));
        assert_eq!(tform_width('A'), 0);
        assert_eq!(tform_width('D'), 8);
    }

    #[test]
    fn a_file_with_nothing_numeric_in_it_says_so() {
        let file = fits(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                    8",
                "NAXIS   =                    0",
            ],
            &[],
        );
        assert_eq!(read(&file), Err(DataError::NoNumericColumns));
        assert!(read(b"too short").is_err());
    }
}
