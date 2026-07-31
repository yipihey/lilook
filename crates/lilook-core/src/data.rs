//! Writing measured values back into Typst source.
//!
//! This is deliberately separate from `lilook-ui`'s `num()`, which formats
//! numbers a *gesture* produced -- a dragged `xlim`, a resized `width` -- where
//! six decimal places is the right answer because the extra digits are mouse
//! jitter and a human has to read the result.
//!
//! Data is the opposite problem. `num()` would write a photon flux of `1.234e-9`
//! as `0`, and every non-finite value as `0` too, so a masked sample would become
//! a real measurement of zero. Both are silent. So values that came from a file
//! or from an evaluated series go through this instead: shortest round-trip
//! formatting, non-finite values named rather than flattened, and a cap, because
//! embedding is a *user-visible* action that must refuse loudly rather than make
//! the buffer unusable.

/// Which root a file a compile read came from.
///
/// A package's own sources show up in the dependency list alongside the user's
/// data -- 50-odd `.typ` files for lilaq and its dependencies -- so the
/// distinction has to survive into the shell rather than be guessed there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum FileRoot {
    /// The directory the document lives in.
    Project,
    /// `namespace/name/version`, e.g. `preview/lilaq/0.6.0`.
    Package(String),
}

/// A file the last compile read.
///
/// Recorded whether or not the read succeeded: a figure that says
/// `csv("run.csv")` with no such file is precisely the case a Data panel exists
/// to explain, and typst's file store reports those too.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct DataFile {
    /// Path as typst resolves it, relative to the root, without a leading slash.
    pub path: String,
    pub root: FileRoot,
    /// Whether the bytes were actually there.
    pub loaded: bool,
}

impl DataFile {
    /// Is this the user's own data, rather than a package's source?
    ///
    /// Two things are excluded: anything from a package, and `.typ` files. A
    /// figure *can* read a `.typ` -- that is what `include` does -- but it is
    /// source, not data, and listing it beside the CSVs would only bury them.
    pub fn is_data(&self) -> bool {
        self.root == FileRoot::Project && !self.path.ends_with(".typ")
    }

    /// The extension, lowercased, for choosing a decoder or an icon.
    pub fn extension(&self) -> Option<String> {
        let (_, ext) = self.path.rsplit_once('.')?;
        (!ext.is_empty() && !ext.contains('/')).then(|| ext.to_ascii_lowercase())
    }
}

/// What asking the compiler about a file came back with.
///
/// Deliberately small. A query exists to *describe* a file, and names, counts and
/// numbers describe one; handing typst `Value`s to the shells would make them
/// depend on the compiler, which is the line `lilook-ui` and `lilook-editor` do
/// not cross.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Answer {
    Strings(Vec<String>),
    Numbers(Vec<f64>),
    /// A keyed file described entry by entry: name, outer length, and inner
    /// length -- the last being zero unless the value is an array of arrays.
    ///
    /// Names alone were enough while every linkable value was a column. A 2-D
    /// array is a *field*, which can only be linked to a mesh's `z`, and nothing
    /// about the name says which one it is.
    Fields(Vec<(String, usize, usize)>),
    Int(i64),
    Text(String),
    /// Something came back, in a shape lilook did not ask for.
    Other,
}

/// The columns of a delimited file, as read from its first row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Columns {
    pub names: Vec<String>,
    /// Whether the first row named the columns rather than being data.
    pub has_header: bool,
    /// Each entry's shape as `(columns, rows)` when it is two-dimensional, in
    /// the same order as `names`.
    ///
    /// Empty for a delimited file, whose first row cannot describe anything but
    /// columns. A keyed file -- which is what lilook writes a FITS image or a
    /// 2-D HDF5 dataset out as -- can hold a field, and a field is linkable only
    /// to a mesh.
    pub grids: Vec<Option<(usize, usize)>>,
}

impl Columns {
    /// The shape of entry `i`, if it is a field rather than a column.
    pub fn grid(&self, i: usize) -> Option<(usize, usize)> {
        *self.grids.get(i)?
    }

    /// The indices of the entries that are fields, and of those that are not.
    pub fn split_fields(&self) -> (Vec<usize>, Vec<usize>) {
        (0..self.names.len()).partition(|i| self.grid(*i).is_some())
    }
}

/// Work out a delimited file's columns from its first row.
///
/// A first row of `t,y` names them; a first row of `0,1.5` is data and they get
/// positional names instead. The test is whether *every* cell parses as a number
/// -- one numeric cell among words is still a header, because `#,name,value` is
/// a real thing people write.
pub fn columns_of(first_row: &[String]) -> Columns {
    let all_numeric = !first_row.is_empty()
        && first_row
            .iter()
            .all(|c| c.trim().parse::<f64>().is_ok() && !c.trim().is_empty());
    if all_numeric || first_row.is_empty() {
        return Columns {
            names: (0..first_row.len())
                .map(|i| format!("column {}", i + 1))
                .collect(),
            has_header: false,
            grids: vec![],
        };
    }
    Columns {
        names: first_row
            .iter()
            .enumerate()
            .map(|(i, c)| match c.trim() {
                "" => format!("column {}", i + 1),
                name => name.to_string(),
            })
            .collect(),
        has_header: true,
        grids: vec![],
    }
}

/// A string as a Typst string literal.
///
/// Paths and column names come from files, so they can contain anything a file
/// name or a CSV header can -- including the two characters that would end the
/// literal early.
pub fn string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Is this a name a Typst field access can use, as in `row.flux`?
///
/// Hyphens are fine -- typst is a kebab-case language and `d.x-ray` lexes as one
/// field, verified against the compiler. Spaces, brackets and a leading digit are
/// not, and CSV headers are full of all three, so those go through
/// `row.at("...")` instead.
pub fn is_plain_identifier(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| (c.is_alphanumeric() && c.is_ascii()) || c == '_' || c == '-')
}

/// How a linked file is shaped, and so how typst reads it.
///
/// Two shapes, because typst's readers come in two kinds. A delimited file is
/// rows of strings, so a column is a `map` and a `float` per cell. A keyed file
/// -- CBOR, JSON, YAML, TOML -- is already a dictionary of arrays of real
/// numbers, so a column is a lookup and nothing is converted.
///
/// The second is what a transcoded HDF5, npz, FITS or descriptor-ASCII file
/// becomes: lilook writes a CBOR sidecar and the document links *that*, since
/// typst cannot read any of those four itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    #[default]
    Delimited,
    Keyed,
}

impl SourceKind {
    /// Which kind a path is, by extension. Delimited is the default because a
    /// file lilook did not write is far more likely to be a CSV.
    pub fn of(path: &str) -> SourceKind {
        let ext = path
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "cbor" | "json" | "yaml" | "yml" | "toml" => SourceKind::Keyed,
            _ => SourceKind::Delimited,
        }
    }

    /// The typst function that reads it.
    pub fn reader(self, path: &str) -> &'static str {
        match self {
            SourceKind::Delimited => "csv",
            SourceKind::Keyed => match path.rsplit_once('.').map(|(_, e)| e) {
                Some("json") => "json",
                Some("yaml") | Some("yml") => "yaml",
                Some("toml") => "toml",
                _ => "cbor",
            },
        }
    }

    /// The expression that asks the compiler what columns the file has.
    pub fn columns_expr(self, path: &str) -> String {
        let p = string_literal(path);
        match self {
            SourceKind::Delimited => format!("csv({p}).at(0, default: ())"),
            // Not `.keys()`: a keyed file can hold a 2-D array, and the shape is
            // what decides whether an entry can be linked at all. Each entry
            // comes back as (name, outer length, inner length).
            SourceKind::Keyed => format!(
                "{}({p}).pairs().map(((k, v)) => {{ \
                 let n = if type(v) == array {{ v.len() }} else {{ 0 }}; \
                 let m = if n > 0 and type(v.at(0)) == array {{ v.at(0).len() }} else {{ 0 }}; \
                 (k, n, m) }})",
                self.reader(path)
            ),
        }
    }
}

/// The `#let` that links a file.
///
/// For a delimited file, `row-type: dictionary` when it names its columns,
/// because then the slot expressions read `r.flux` and say what they mean; plain
/// rows otherwise, where there is nothing to name them with.
pub fn binding_source(path: &str, kind: SourceKind, has_header: bool) -> String {
    let p = string_literal(path);
    match kind {
        SourceKind::Delimited if has_header => format!("csv({p}, row-type: dictionary)"),
        SourceKind::Delimited => format!("csv({p})"),
        SourceKind::Keyed => format!("{}({p})", kind.reader(path)),
    }
}

/// The `#let` that links a delimited file. See [`binding_source`].
pub fn csv_binding_source(path: &str, has_header: bool) -> String {
    binding_source(path, SourceKind::Delimited, has_header)
}

/// The slot expression that reads one column, whatever kind of file it is in.
pub fn column_source(
    binding: &str,
    kind: SourceKind,
    columns: &Columns,
    index: usize,
) -> Option<String> {
    match kind {
        SourceKind::Delimited => csv_column_source(binding, columns, index),
        SourceKind::Keyed => {
            let name = columns.names.get(index)?;
            // No `float()`: a keyed file already holds numbers. That is the whole
            // reason a transcoded sidecar is CBOR and not CSV.
            Some(if is_plain_identifier(name) {
                format!("{binding}.{name}")
            } else {
                format!("{binding}.at({})", string_literal(name))
            })
        }
    }
}

/// The slot expression that reads one column out of a linked delimited file.
///
/// `float()` per cell because `csv()` yields strings -- measured at no
/// detectable cost even at 100k rows (`docs/findings.md`). Bound to a name
/// rather than inlined into the series call: the probe recovers data by
/// re-evaluating the slot's text, so a name is free where an inlined `map`
/// converts everything twice.
pub fn csv_column_source(binding: &str, columns: &Columns, index: usize) -> Option<String> {
    let access = if columns.has_header {
        let name = columns.names.get(index)?;
        if is_plain_identifier(name) {
            format!("r.{name}")
        } else {
            format!("r.at({})", string_literal(name))
        }
    } else {
        if index >= columns.names.len() {
            return None;
        }
        format!("r.at({index})")
    };
    Some(format!("{binding}.map(r => float({access}))"))
}

/// A Typst binding name for a file, avoiding names already taken.
///
/// `runs/flux.csv` becomes `flux`, or `flux2` if something already answers to
/// `flux`. Identifiers cannot start with a digit, so `2026.csv` becomes
/// `data2026`. `is_taken` is asked rather than given a list because the document
/// answers that question directly, through `Document::binding_of`.
pub fn binding_name_for(path: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or("data");
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() && c.is_ascii() {
                c
            } else {
                '-'
            }
        })
        .collect();
    base = base.trim_matches('-').to_string();
    if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
        base = format!("data{base}");
    }
    if !is_taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|c| !is_taken(c))
        .expect("some suffix is free")
}

/// The most values lilook will write into a document.
///
/// Not a compile limit -- a 100k-point literal array compiles in the same time
/// as the file it came from (`docs/findings.md`). It is an *editing* limit: the
/// source pane reparses the whole buffer, so past roughly this many values the
/// document stops behaving like text you can type in. Beyond it the data stays
/// where it is and lilook says so.
pub const MAX_EMBEDDED_VALUES: usize = 20_000;

/// A refusal to embed, carrying the numbers a message needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyValues {
    pub found: usize,
    pub limit: usize,
}

impl std::fmt::Display for TooManyValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} values is more than lilook will embed ({}); the data stays in the file",
            self.found, self.limit
        )
    }
}

impl std::error::Error for TooManyValues {}

/// Format one measured value so that reading it back gives the same `f64`.
///
/// Rust's `{}` is already shortest-round-trip, but it never uses an exponent, so
/// `1e-300` would come out as three hundred zeros and a one. `{:e}` covers that
/// end. Both round-trip, so the only question is which one a person would rather
/// find in their document -- and the answer is *usually* the plain one: picking
/// the shorter of the two unconditionally turns `1000` into `1e3`.
///
/// So plain notation wins unless it has run away. Shortest-round-trip needs at
/// most 17 significant digits, so past 20 characters what is left is padding
/// zeros, and that is where the exponent earns its keep.
pub fn data_num(v: f64) -> String {
    if v.is_nan() {
        return "float.nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "float.inf".into()
        } else {
            "-float.inf".into()
        };
    }
    // `-0.0` formats as `-0`, which typst reads as an integer and so loses the
    // sign. Nothing plots differently for it, and the alternative is writing
    // `-0.0` for every zero in the file.
    let plain = format!("{v}");
    if plain.len() <= 20 {
        return plain;
    }
    let exp = format!("{v:e}");
    if exp.len() < plain.len() {
        exp
    } else {
        plain
    }
}

/// A data value a gesture produced: an axis limit from a pan, a dragged point.
///
/// Six *significant figures*, not six decimal places. `lilook-ui`'s `num()` does
/// the latter, which is right for a width or a margin -- the extra digits there
/// are mouse jitter -- but wrong for anything on a data axis: it writes `3e-9` as
/// `0`, and panning a log axis produces limits like that legitimately. lilaq then
/// refuses the figure, "value must be strictly positive", which is exactly how
/// this was found.
///
/// Six significant figures keeps an ordinary pan tidy (`10.1234`, not seventeen
/// digits) while never rounding a small number to nothing.
pub fn gesture_num(v: f64) -> String {
    if !v.is_finite() || v == 0.0 {
        return data_num(v);
    }
    let magnitude = v.abs().log10().floor();
    let factor = 10f64.powf(5.0 - magnitude);
    if !factor.is_finite() || factor == 0.0 {
        // Near the extremes of the range, scaling would overflow. The exact form
        // is short there anyway.
        return data_num(v);
    }
    let rounded = (v * factor).round() / factor;
    data_num(if rounded == 0.0 { v } else { rounded })
}

/// Render measured values as a Typst array literal.
///
/// The one-element case is why this is not a `join`: `(1)` is a parenthesised
/// integer in Typst, not an array, so a single-point series would come back as a
/// scalar and the next compile would fail on it.
pub fn data_array_source(values: &[f64]) -> Result<String, TooManyValues> {
    if values.len() > MAX_EMBEDDED_VALUES {
        return Err(TooManyValues {
            found: values.len(),
            limit: MAX_EMBEDDED_VALUES,
        });
    }
    Ok(match values {
        [] => "()".to_string(),
        [one] => format!("({},)", data_num(*one)),
        many => {
            let items: Vec<String> = many.iter().copied().map(data_num).collect();
            format!("({})", items.join(", "))
        }
    })
}

/// Split `8cm` into (8.0, "cm"); returns None when the value is not a bare
/// numeric literal (`calc.pi * 2`, a binding, ...).
pub fn split_numeric(s: &str) -> Option<(f64, String)> {
    let s = s.trim();
    // An exponent must not be read as the start of a unit: `1e-4` is a number,
    // `1em` is a length.
    let bytes = s.as_bytes();
    let mut cut = s.len();
    for (i, c) in s.char_indices() {
        if c.is_ascii_alphabetic() || c == '%' {
            let is_exponent = (c == 'e' || c == 'E')
                && i + 1 < s.len()
                && matches!(bytes[i + 1], b'0'..=b'9' | b'+' | b'-')
                && s[..i].parse::<f64>().is_ok();
            if !is_exponent {
                cut = i;
                break;
            }
        }
    }
    let (num, unit) = s.split_at(cut);
    num.trim()
        .parse::<f64>()
        .ok()
        .map(|v| (v, unit.to_string()))
}

/// `[Time]` -> `Time`. Content the user wrote as a variable or an expression
/// returns None.
pub fn parse_content(s: &str) -> Option<String> {
    let s = s.trim();
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    // Nested brackets are fine, unbalanced ones are not this control's problem.
    (!inner.contains(']') || inner.matches('[').count() == inner.matches(']').count())
        .then(|| inner.to_string())
}
/// Which of typst's two spellings of "some words" a value is written in.
///
/// lilaq takes either almost everywhere -- `title: [Flux]` and `title: "Flux"`
/// are both fine -- and the difference matters when writing back: a value the
/// user wrote as a string has to stay a string, or lilook has silently changed
/// the shape of their source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextShape {
    /// `[some words]` -- typst content, which can hold markup.
    Content,
    /// `"some words"` -- a plain string.
    Str,
}
/// Read a value that is words: content, a string, or nothing at all.
///
/// `None` means this is not a textual value (an expression, a function call), so
/// the caller must leave it to the source editor. `Some((shape, text))` gives the
/// words with the quoting removed, ready for a plain text field -- which is the
/// whole point: the schema already says this parameter takes words, so nobody
/// should have to type `[..]` or `".."` to say so again.
pub fn parse_text(s: &str) -> Option<(TextShape, String)> {
    let s = s.trim();
    if let Some(inner) = parse_content(s) {
        return Some((TextShape::Content, inner));
    }
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    // An escaped quote is fine; a bare one means this is not one string literal.
    let mut chars = inner.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(e) => out.push(e),
                None => return None,
            },
            '"' => return None,
            _ => out.push(c),
        }
    }
    Some((TextShape::Str, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::check_expr;

    /// The property that matters: nothing is lost on the way to the document.
    #[test]
    fn every_finite_value_survives_the_round_trip_exactly() {
        let cases = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.1,
            1.0 / 3.0,
            1.234e-9,
            1e-300,
            5e-324, // the smallest subnormal
            2.2250738585072014e-308,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            std::f64::consts::PI,
            6.02214076e23,
            -7.29735e-3,
        ];
        for v in cases {
            let s = data_num(v);
            let back: f64 = s.parse().unwrap_or_else(|e| panic!("{s:?}: {e}"));
            assert_eq!(
                back.to_bits(),
                v.to_bits(),
                "{v} emitted as {s:?} and read back as {back}"
            );
        }
    }

    /// The defect this replaces: `num()` wrote six decimal places, so anything
    /// smaller than a microunit became exactly zero.
    #[test]
    fn small_magnitudes_are_not_truncated_to_zero() {
        for v in [1.234e-9, 1e-30, 4.9e-324] {
            let s = data_num(v);
            assert_ne!(s, "0", "{v} was flattened to zero");
            assert_eq!(s.parse::<f64>().unwrap(), v);
        }
    }

    /// The other silent one: a masked sample must not become a measurement.
    #[test]
    fn non_finite_values_are_named_rather_than_zeroed() {
        assert_eq!(data_num(f64::NAN), "float.nan");
        assert_eq!(data_num(f64::INFINITY), "float.inf");
        assert_eq!(data_num(f64::NEG_INFINITY), "-float.inf");
    }

    /// Extreme exponents must stay short enough to live in a document.
    #[test]
    fn extreme_exponents_use_exponent_notation() {
        assert_eq!(data_num(1e-300), "1e-300");
        assert_eq!(data_num(f64::MAX), "1.7976931348623157e308");
        assert_eq!(data_num(6.02214076e23), "6.02214076e23");
        assert!(data_num(5e-324).len() < 12);
    }

    #[test]
    fn ordinary_values_stay_readable() {
        assert_eq!(data_num(0.0), "0");
        assert_eq!(data_num(1.0), "1");
        assert_eq!(data_num(1.5), "1.5");
        assert_eq!(data_num(-2.25), "-2.25");
        // Plain notation, not `1e3`: a scale of thousands is ordinary data.
        assert_eq!(data_num(1000.0), "1000");
        assert_eq!(data_num(1.234e-9), "0.000000001234");
        assert_eq!(data_num(1.0 / 3.0), "0.3333333333333333");
    }

    /// `(1)` is a scalar; `(1,)` is an array. A one-point series has to be the
    /// second one or the document it lands in stops compiling.
    #[test]
    fn a_single_value_is_still_an_array() {
        assert_eq!(data_array_source(&[1.0]).unwrap(), "(1,)");
        assert_eq!(data_array_source(&[]).unwrap(), "()");
        assert_eq!(
            data_array_source(&[0.0, 1.5, -2.25]).unwrap(),
            "(0, 1.5, -2.25)"
        );
    }

    /// Everything this emits has to survive the parser lilook validates with.
    #[test]
    fn content_is_read_out_of_its_brackets() {
        assert_eq!(super::parse_content("[Time]").as_deref(), Some("Time"));
        assert_eq!(super::parse_content("my-label"), None);
    }

    #[test]
    fn emitted_arrays_reparse() {
        let arrays: Vec<Vec<f64>> = vec![
            vec![],
            vec![1.0],
            vec![0.0, 1.5, -2.25],
            vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY],
            vec![1e-300, f64::MAX, 5e-324],
            vec![1.234e-9, 6.02214076e23],
        ];
        for a in arrays {
            let s = data_array_source(&a).unwrap();
            assert!(check_expr(&s).is_ok(), "{s:?}: {:?}", check_expr(&s));
        }
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_header_row_is_told_from_a_data_row() {
        assert_eq!(columns_of(&row(&["t", "y"])).names, ["t", "y"]);
        assert!(columns_of(&row(&["t", "y"])).has_header);

        let data = columns_of(&row(&["0", "1.5"]));
        assert_eq!(data.names, ["column 1", "column 2"]);
        assert!(!data.has_header);
        assert!(!columns_of(&row(&["-3", "1e5"])).has_header);

        // One numeric cell among names is still a header: `#,name,value` is a
        // real thing people write.
        assert!(columns_of(&row(&["#", "flux"])).has_header);
        assert!(columns_of(&row(&["time", "0"])).has_header);
        // An empty cell in a header row still needs a name.
        assert_eq!(columns_of(&row(&["t", ""])).names, ["t", "column 2"]);
        assert!(columns_of(&[]).names.is_empty());
    }

    #[test]
    fn field_access_is_only_offered_where_it_works() {
        assert!(is_plain_identifier("flux"));
        assert!(is_plain_identifier("t_0"));
        // Verified against the compiler: typst is kebab-case and `d.x-ray`
        // lexes as one field, not a subtraction.
        assert!(is_plain_identifier("x-ray"));
        assert!(!is_plain_identifier("flux (mJy)"));
        assert!(!is_plain_identifier("2theta"));
        assert!(!is_plain_identifier(""));
        assert!(!is_plain_identifier("#"));
        assert!(!is_plain_identifier("-lead"));
    }

    #[test]
    fn paths_survive_being_quoted() {
        assert_eq!(string_literal("run.csv"), r#""run.csv""#);
        assert_eq!(string_literal(r#"a"b.csv"#), r#""a\"b.csv""#);
        assert_eq!(string_literal(r"dir\run.csv"), r#""dir\\run.csv""#);
    }

    /// Everything a link writes has to reparse, since that is the gate every
    /// value lilook writes goes through.
    #[test]
    fn a_link_writes_source_that_reparses() {
        let named = columns_of(&row(&["t", "flux (mJy)"]));
        assert_eq!(
            csv_binding_source("run.csv", named.has_header),
            r#"csv("run.csv", row-type: dictionary)"#
        );
        assert_eq!(
            csv_column_source("run", &named, 0).unwrap(),
            "run.map(r => float(r.t))"
        );
        // A header no field access can reach goes through `at`.
        assert_eq!(
            csv_column_source("run", &named, 1).unwrap(),
            r#"run.map(r => float(r.at("flux (mJy)")))"#
        );
        assert_eq!(csv_column_source("run", &named, 2), None);

        let headerless = columns_of(&row(&["0", "1.5", "2"]));
        assert_eq!(csv_binding_source("run.dat", false), r#"csv("run.dat")"#);
        assert_eq!(
            csv_column_source("run", &headerless, 2).unwrap(),
            "run.map(r => float(r.at(2)))"
        );
        assert_eq!(csv_column_source("run", &headerless, 3), None);

        for s in [
            csv_binding_source("run.csv", true),
            csv_binding_source("run.csv", false),
            csv_column_source("run", &named, 0).unwrap(),
            csv_column_source("run", &named, 1).unwrap(),
            csv_column_source("run", &headerless, 0).unwrap(),
        ] {
            assert!(check_expr(&s).is_ok(), "{s:?}");
        }
    }

    /// A transcoded sidecar is linked the same way a CSV is, but with no
    /// per-cell conversion: CBOR already holds numbers.
    #[test]
    fn a_keyed_file_is_linked_by_lookup_rather_than_by_map() {
        assert_eq!(SourceKind::of("run.csv"), SourceKind::Delimited);
        assert_eq!(SourceKind::of("run.dat"), SourceKind::Delimited);
        assert_eq!(SourceKind::of(".lilook/run.cbor"), SourceKind::Keyed);
        assert_eq!(SourceKind::of("run.json"), SourceKind::Keyed);
        assert_eq!(SourceKind::of("noextension"), SourceKind::Delimited);

        let k = SourceKind::Keyed;
        assert_eq!(
            binding_source(".lilook/run.cbor", k, true),
            r#"cbor(".lilook/run.cbor")"#
        );
        assert_eq!(binding_source("run.json", k, true), r#"json("run.json")"#);
        assert_eq!(binding_source("run.yml", k, true), r#"yaml("run.yml")"#);

        let cols = Columns {
            grids: vec![],
            names: vec!["t".into(), "flux (mJy)".into()],
            has_header: true,
        };
        assert_eq!(column_source("d", k, &cols, 0).unwrap(), "d.t");
        assert_eq!(
            column_source("d", k, &cols, 1).unwrap(),
            r#"d.at("flux (mJy)")"#
        );
        assert_eq!(column_source("d", k, &cols, 2), None);
        for s in [
            binding_source(".lilook/run.cbor", k, true),
            column_source("d", k, &cols, 0).unwrap(),
            column_source("d", k, &cols, 1).unwrap(),
        ] {
            assert!(check_expr(&s).is_ok(), "{s:?}");
        }

        // And the expression that asks what is in each kind.
        assert!(SourceKind::Delimited
            .columns_expr("run.csv")
            .starts_with("csv("));
        // Not `.keys()`: a keyed file can hold a 2-D array, and the shape is
        // what decides whether an entry is a column or a mesh's field. It has to
        // be a real expression, since it is compiled rather than parsed.
        let keyed = SourceKind::Keyed.columns_expr("run.cbor");
        assert!(keyed.starts_with(r#"cbor("run.cbor").pairs()"#), "{keyed}");
        assert!(check_expr(&keyed).is_ok(), "{keyed:?}");
    }

    #[test]
    fn a_binding_name_comes_from_the_file_and_avoids_collisions() {
        let free = |_: &str| false;
        assert_eq!(binding_name_for("run.csv", free), "run");
        assert_eq!(
            binding_name_for("data/2026-flux.csv", free),
            "data2026-flux"
        );
        assert_eq!(binding_name_for("flux.tar.gz", free), "flux");
        // Identifiers cannot start with a digit.
        assert_eq!(binding_name_for("2026.csv", free), "data2026");
        // Nor be empty, whatever the file was called.
        assert_eq!(binding_name_for("...csv", free), "data");
        assert_eq!(binding_name_for("a b c.csv", free), "a-b-c");

        assert_eq!(binding_name_for("run.csv", |n| n == "run"), "run2");
        assert_eq!(
            binding_name_for("run.csv", |n| ["run", "run2"].contains(&n)),
            "run3"
        );
    }

    fn file(path: &str, root: FileRoot) -> DataFile {
        DataFile {
            path: path.into(),
            root,
            loaded: true,
        }
    }

    /// The filter the Data panel lives or dies by: every compile reads dozens of
    /// package sources, and the user's one CSV has to be findable among them.
    #[test]
    fn a_packages_sources_are_not_the_users_data() {
        assert!(file("run.csv", FileRoot::Project).is_data());
        assert!(file("data/flux.npz", FileRoot::Project).is_data());
        // Source, not data -- a figure can `include` a .typ, but listing it
        // beside the CSVs would only bury them.
        assert!(!file("helpers.typ", FileRoot::Project).is_data());
        assert!(!file("<lilook>.typ", FileRoot::Project).is_data());
        assert!(!file(
            "src/lilaq.typ",
            FileRoot::Package("preview/lilaq/0.6.0".into())
        )
        .is_data());
        // Data shipped *inside* a package is still the package's business.
        assert!(!file(
            "assets/table.csv",
            FileRoot::Package("preview/lilaq/0.6.0".into())
        )
        .is_data());
    }

    #[test]
    fn extensions_choose_the_decoder() {
        assert_eq!(
            file("run.CSV", FileRoot::Project).extension().as_deref(),
            Some("csv")
        );
        assert_eq!(
            file("a/b/run.fits", FileRoot::Project)
                .extension()
                .as_deref(),
            Some("fits")
        );
        assert_eq!(file("README", FileRoot::Project).extension(), None);
        // A dot in a directory name is not an extension on the file.
        assert_eq!(file("v1.2/data", FileRoot::Project).extension(), None);
    }

    /// Six significant figures, and never a zero where the value was not one.
    #[test]
    fn a_gesture_value_keeps_its_significant_figures() {
        assert_eq!(gesture_num(0.0), "0");
        assert_eq!(gesture_num(10.0), "10");
        assert_eq!(gesture_num(1.5), "1.5");
        // Tidy where `num()` was tidy.
        assert_eq!(gesture_num(10.123456789), "10.1235");
        assert_eq!(gesture_num(-0.333333333), "-0.333333");
        // And correct where `num()` wrote `0`, which is what broke a log pan.
        for v in [3e-9, 1.234e-12, 5e-300, f64::MIN_POSITIVE] {
            let s = gesture_num(v);
            assert_ne!(s, "0", "{v} was flattened");
            let back: f64 = s.parse().unwrap();
            assert!(back > 0.0, "{v} -> {s} -> {back}");
            assert!(
                (back / v - 1.0).abs() < 1e-5,
                "{v} -> {s} lost too much precision"
            );
        }
        // Large magnitudes stay short rather than becoming a wall of zeros.
        assert!(gesture_num(6.02214076e23).len() < 16);
        for v in [3e-9, -1.0, 1e300, f64::MAX, f64::MIN_POSITIVE] {
            assert!(check_expr(&gesture_num(v)).is_ok(), "{v}");
        }
    }

    #[test]
    fn embedding_refuses_loudly_past_the_cap() {
        let big = vec![0.0; MAX_EMBEDDED_VALUES + 1];
        let err = data_array_source(&big).expect_err("past the cap");
        assert_eq!(err.found, MAX_EMBEDDED_VALUES + 1);
        assert!(err.to_string().contains("stays in the file"));
        // The cap itself is allowed, so the boundary is not off by one.
        assert!(data_array_source(&vec![0.0; MAX_EMBEDDED_VALUES]).is_ok());
    }
}
