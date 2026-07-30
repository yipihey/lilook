//! Veusz's descriptor ASCII, and plain numeric columns.
//!
//! Veusz's own text format puts a *descriptor* before the data naming what each
//! column is:
//!
//! ```text
//! descriptor t flux +- bg +,-
//! 0  1.5  0.1  0.3  0.05  0.02
//! 1  2.5  0.2  0.4  0.06  0.03
//! ```
//!
//! `+-` is a symmetric error on the dataset before it, `+` and `-` are the two
//! halves of an asymmetric one, and `+,-` is both in that order. A dataset called
//! `flux` with a `+-` after it therefore spans two columns, and its error lands on
//! lilaq's `yerr:` -- which is why the scene had to grow named channels.
//!
//! Deliberately *not* done in typst. `read()` plus `split` plus `float` would
//! work, but it is an interpreted parser inside the user's manuscript, running
//! twice per compile, and it puts five lines of functional pipeline into a
//! document someone has to read. This decodes here and links a CBOR sidecar
//! instead.

use crate::{Column, DataError, Dataset};

/// Read a descriptor-ASCII or plain-columns table.
pub fn read(text: &str) -> Result<Dataset, DataError> {
    let mut names: Option<Vec<Name>> = None;
    let mut rows: Vec<Vec<f64>> = vec![];

    for line in text.lines() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        // A row is a row if every field is a number. The first line that is not
        // becomes the descriptor -- which also means a file with no descriptor
        // needs no special case.
        let numeric: Option<Vec<f64>> = fields(line).map(|f| f.parse::<f64>().ok()).collect();
        match numeric {
            Some(values) => rows.push(values),
            None if names.is_none() && rows.is_empty() => {
                names = Some(descriptor(line));
            }
            // A non-numeric line after data has started: a second descriptor, or
            // a stray. Veusz allows several blocks; taking the first is enough,
            // and silently mixing them would be worse.
            None => break,
        }
    }

    if rows.is_empty() {
        return Err(DataError::NoNumericColumns);
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let names = names.unwrap_or_else(|| {
        (0..width)
            .map(|i| Name {
                text: format!("column {}", i + 1),
            })
            .collect()
    });
    // Ragged rows are padded rather than refused: a trailing short line is
    // common, and dropping the whole file over it would be unhelpful.
    let columns = (0..width)
        .map(|c| Column {
            grid: None,
            name: names
                .get(c)
                .map(|n| n.text.clone())
                .unwrap_or_else(|| format!("column {}", c + 1)),
            values: rows
                .iter()
                .map(|r| r.get(c).copied().unwrap_or(f64::NAN))
                .collect(),
        })
        .collect();
    Ok(Dataset { columns })
}

struct Name {
    text: String,
}

/// Expand a descriptor line into one name per column.
///
/// The error markers attach to the dataset before them, so `flux +-` is two
/// columns -- `flux` and `flux_err` -- and `bg +,-` is three.
fn descriptor(line: &str) -> Vec<Name> {
    let line = line
        .trim()
        .strip_prefix("descriptor")
        .map_or(line.trim(), str::trim);
    let mut out: Vec<Name> = vec![];
    let mut last: Option<String> = None;
    // Split on commas as well as whitespace, exactly as the data rows are. That
    // covers both meanings a comma has here with no special case: `t,y` is two
    // names, `flux,+-` is a name and its error, and `+,-` is two markers.
    for token in fields(line) {
        if is_error_marker(token) {
            let owner = last.clone().unwrap_or_else(|| "column".into());
            for suffix in suffixes(token) {
                out.push(Name {
                    text: format!("{owner}{suffix}"),
                });
            }
        } else {
            last = Some(token.to_string());
            out.push(Name {
                text: token.to_string(),
            });
        }
    }
    out
}

/// One line's fields. Whitespace or commas, and never an empty one, so
/// `0,,1` and `0  1` both read as two.
fn fields(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|f| !f.is_empty())
}

fn is_error_marker(token: &str) -> bool {
    matches!(token, "+-" | "-+" | "+" | "-")
}

/// What a marker names, in column order.
fn suffixes(marker: &str) -> Vec<&'static str> {
    match marker {
        "+-" | "-+" => vec!["_err"],
        "+" => vec!["_perr"],
        "-" => vec!["_nerr"],
        _ => vec![],
    }
}

/// Drop a `#` or `!` comment, which are the two conventions in the wild.
fn strip_comment(line: &str) -> &str {
    let cut = line.find(['#', '!']).unwrap_or(line.len());
    line[..cut].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_descriptor_names_the_columns() {
        let d = read("descriptor t flux\n0 1.5\n1 2.5\n").unwrap();
        assert_eq!(d.names(), ["t", "flux"]);
        assert_eq!(d.column("flux").unwrap().values, [1.5, 2.5]);
    }

    /// The point of mirroring Veusz's descriptor: an error column is named, so it
    /// can be linked to `yerr:` rather than mistaken for another series.
    #[test]
    fn error_markers_attach_to_the_dataset_before_them() {
        let d = read("t flux +-\n0 1.5 0.1\n1 2.5 0.2\n").unwrap();
        assert_eq!(d.names(), ["t", "flux", "flux_err"]);
        assert_eq!(d.column("flux_err").unwrap().values, [0.1, 0.2]);

        // Asymmetric, as two separate columns.
        let d = read("t y + -\n0 1 0.5 0.25\n").unwrap();
        assert_eq!(d.names(), ["t", "y", "y_perr", "y_nerr"]);
        assert_eq!(d.column("y_perr").unwrap().values, [0.5]);
        assert_eq!(d.column("y_nerr").unwrap().values, [0.25]);

        // And the comma forms Veusz also accepts.
        let d = read("t y,+- \n0 1 0.5\n").unwrap();
        assert_eq!(d.names(), ["t", "y", "y_err"]);
        let d = read("t y +,-\n0 1 0.5 0.25\n").unwrap();
        assert_eq!(d.names(), ["t", "y", "y_perr", "y_nerr"]);
        // `-+` is the same as `+-`.
        assert_eq!(read("t y -+\n0 1 0.1\n").unwrap().names()[2], "y_err");
    }

    #[test]
    fn a_file_with_no_descriptor_is_still_columns() {
        let d = read("0 1.5\n1 2.5\n2 3.5\n").unwrap();
        assert_eq!(d.names(), ["column 1", "column 2"]);
        assert_eq!(d.column("column 2").unwrap().values, [1.5, 2.5, 3.5]);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let text =
            "# a run from 2026-07-30\n\ndescriptor t y\n\n0 1  # first\n1 2\n! a trailing note\n";
        let d = read(text).unwrap();
        assert_eq!(d.names(), ["t", "y"]);
        assert_eq!(d.column("y").unwrap().values, [1.0, 2.0]);
    }

    #[test]
    fn whitespace_or_commas_separate_fields() {
        let d = read("t,y\n0,1\n1,2\n").unwrap();
        assert_eq!(d.names(), ["t", "y"]);
        let d = read("t\ty\n0\t1\n").unwrap();
        assert_eq!(d.names(), ["t", "y"]);
        // Several spaces, as a hand-aligned table has.
        let d = read("t     y\n0     1\n").unwrap();
        assert_eq!(d.names(), ["t", "y"]);
    }

    #[test]
    fn a_short_row_is_padded_rather_than_dropping_the_file() {
        let d = read("t y z\n0 1 2\n3 4\n").unwrap();
        assert_eq!(d.column("z").unwrap().values.len(), 2);
        assert!(d.column("z").unwrap().values[1].is_nan());
    }

    #[test]
    fn scientific_notation_and_signs_read_as_numbers() {
        let d = read("1e-9 -2.5E3 +4\n").unwrap();
        assert_eq!(d.columns[0].values, [1e-9]);
        assert_eq!(d.columns[1].values, [-2500.0]);
        assert_eq!(d.columns[2].values, [4.0]);
    }

    #[test]
    fn nothing_numeric_says_so() {
        assert_eq!(read(""), Err(DataError::NoNumericColumns));
        assert_eq!(read("# only a comment\n"), Err(DataError::NoNumericColumns));
        assert_eq!(read("descriptor t y\n"), Err(DataError::NoNumericColumns));
    }
}
