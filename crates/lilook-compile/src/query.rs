//! Asking the compiler about a file the document does not mention yet.
//!
//! "What columns are in `run.csv`?" cannot go through `probe.rs`. A probe
//! re-evaluates source text the user already wrote, injected into the diagram's
//! own argument list -- so it needs a diagram to inject into, and the first thing
//! anyone does with a data file is link it to a document that has none yet.
//! Appending `#metadata(..)` at document level instead is not layout-neutral on
//! an `auto`-sized page, which would break the invariant that probes cannot
//! change the rendering.
//!
//! So a query is its own thing: a throwaway one-line document, compiled under a
//! file id of its own so the editing buffer's parsed `Source` is untouched, with
//! no lilaq import and nothing to lay out. Measured at ~9 ms on top of a warm
//! recompile, against ~120 ms if the next compile had gone cold. It reads files
//! through the same loader the figure does, which is what makes it work
//! identically in the browser -- where "the file system" is whatever the user
//! dropped onto the page.

use lilook_core::data::Answer;
use lilook_core::string_literal;
use typst::foundations::Value;
use typst_syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

/// The label a query's answer comes back under.
pub const QUERY_LABEL: &str = "lilook-query";

/// The id a query document compiles as. Deliberately not `main_id()`: sharing
/// that slot would rewrite the document's cached source twice per query.
pub fn query_id() -> FileId {
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new("<lilook-query>.typ").unwrap(),
    )
    .intern()
}

/// Wrap an expression in the smallest document that can answer with it.
///
/// `metadata` is inert content: it lays out to nothing, so the page it is on
/// costs nothing to typeset. There is no `lq` import because a query never
/// mentions lilaq -- which also means a query still works in a project where the
/// package is missing.
pub fn document(expr: &str) -> String {
    format!("#metadata({expr})<{QUERY_LABEL}>\n")
}

/// Read the answer out of a compiled query document.
pub fn answer(doc: &typst_layout::PagedDocument) -> Option<Answer> {
    use typst::introspection::Introspector as _;
    let found = doc
        .introspector()
        .query_first(&crate::probe::label(QUERY_LABEL))?;
    Some(classify(&found.field_by_name("value").ok()?))
}

/// Reduce a typst value to the shapes the shells understand.
fn classify(value: &Value) -> Answer {
    match value {
        Value::Str(s) => Answer::Text(s.to_string()),
        Value::Int(i) => Answer::Int(*i),
        Value::Array(a) => {
            // All strings, or all numbers, or neither: a CSV header row is the
            // first, a column of measurements the second.
            let strings: Option<Vec<String>> = a
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect();
            if let Some(s) = strings {
                return Answer::Strings(s);
            }
            // (name, outer length, inner length) per entry: what a keyed file
            // answers with, since a 2-D value is a field rather than a column.
            let fields: Option<Vec<(String, usize, usize)>> = a
                .iter()
                .map(|v| match v {
                    Value::Array(t) => match t.iter().collect::<Vec<_>>()[..] {
                        [Value::Str(k), Value::Int(n), Value::Int(m)] => {
                            Some((k.to_string(), (*n).max(0) as usize, (*m).max(0) as usize))
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            if let Some(f) = fields.filter(|f| !f.is_empty()) {
                return Answer::Fields(f);
            }
            let numbers: Option<Vec<f64>> = a
                .iter()
                .map(|v| match v {
                    Value::Int(i) => Some(*i as f64),
                    Value::Float(f) => Some(*f),
                    _ => None,
                })
                .collect();
            numbers.map_or(Answer::Other, Answer::Numbers)
        }
        _ => Answer::Other,
    }
}

/// The expression that reads a delimited file's first row.
///
/// `csv()` gives every cell as a string whether or not it holds a number, so
/// this says nothing about whether that row *is* a header. `columns_of` decides
/// that by looking at what came back.
pub fn header_expr(path: &str) -> String {
    format!("csv({}).at(0, default: ())", string_literal(path))
}

/// The expression that counts a delimited file's rows, header included.
pub fn row_count_expr(path: &str) -> String {
    format!("csv({}).len()", string_literal(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_document_mentions_nothing_but_the_expression() {
        let d = document(&row_count_expr("run.csv"));
        assert!(d.contains(QUERY_LABEL));
        assert!(!d.contains("lilaq"), "a query must not need the package");
        assert!(!d.contains("set page"), "nothing to lay out");
        // And it has to parse as the expression it wraps.
        assert!(lilook_core::check_expr(&row_count_expr("run.csv")).is_ok());
    }

    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_out_of_the_expression() {
        let e = header_expr(r#"a"; panic()"#);
        assert!(e.contains(r#"\""#), "{e}");
        assert!(lilook_core::check_expr(&e).is_ok());
    }
}
