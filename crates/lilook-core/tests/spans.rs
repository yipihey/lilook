//! The document as coloured spans: data, not colours.

use lilook_core::{Document, Token};

const SRC: &str = r#"// a comment
#import "@preview/lilaq:0.6.0" as lq
#let n = 42
#lq.diagram(
  lq.plot((1, 2), (3, 4)),
  lq.scatter((1, 2), (5, 6)),
)
"#;

#[test]
fn spans_do_not_overlap_and_cover_what_matters() {
    let doc = Document::new(SRC);
    let spans = doc.spans();
    assert!(!spans.is_empty());

    // In order, non-overlapping, inside the buffer, on char boundaries. This is
    // the contract a renderer walks straight into a layout without sorting.
    let mut last = 0;
    for (r, _) in &spans {
        assert!(r.start >= last, "spans out of order at {r:?}");
        assert!(r.end <= SRC.len(), "{r:?} past the end");
        assert!(SRC.is_char_boundary(r.start) && SRC.is_char_boundary(r.end));
        last = r.end;
    }

    let of = |needle: &str| {
        let at = SRC.find(needle).expect(needle);
        spans
            .iter()
            .find(|(r, _)| r.start <= at && at < r.end)
            .map(|(_, t)| *t)
    };
    assert_eq!(of("// a comment"), Some(Token::Comment));
    assert_eq!(of("\"@preview"), Some(Token::Str));
    assert_eq!(of("42"), Some(Token::Number));
    assert_eq!(of("#import"), Some(Token::Keyword));
    assert_eq!(of("n = 42"), Some(Token::Binding), "the name a #let binds");
}

/// A series call carries its ordinal, which is what indexes the colour cycle --
/// so a source pane can show each curve's own colour beside the line that drew
/// it. No parser reports this; it comes from the document's figure structure.
#[test]
fn a_series_call_knows_which_series_it_is() {
    let doc = Document::new(SRC);
    let spans = doc.spans();
    let series: Vec<(usize, &str)> = spans
        .iter()
        .filter_map(|(r, t)| match t {
            Token::Series(i) => Some((*i, &SRC[r.clone()])),
            _ => None,
        })
        .collect();
    assert_eq!(
        series,
        vec![(0, "lq.plot"), (1, "lq.scatter")],
        "each series tagged with its index in the diagram"
    );

    // The diagram itself is an ordinary call, not a series.
    let at = SRC.find("lq.diagram").unwrap();
    assert_eq!(
        spans.iter().find(|(r, _)| r.start == at).map(|(_, t)| *t),
        Some(Token::Call)
    );
}

/// A comment holding what looks like code is still one comment.
#[test]
fn nothing_is_coloured_inside_a_comment_or_a_string() {
    let src = "// #let x = 1\n#let s = \"#let y = 2\"\n";
    let doc = Document::new(src);
    let spans = doc.spans();
    let inside = |needle: &str| {
        let at = src.find(needle).expect(needle);
        spans.iter().filter(|(r, _)| r.contains(&at)).count()
    };
    assert_eq!(inside("#let x"), 1, "one span over the comment");
    assert_eq!(inside("#let y"), 1, "one span over the string");
}
