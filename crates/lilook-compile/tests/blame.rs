//! Locating an error that names no location.
//!
//! Most lilaq failures arrive with no span, because lilaq validates inside its
//! own package. The compiler can still answer: remove one thing, see whether the
//! error survives. Measured at ~4 ms a variant.

use lilook_compile::{backend::Hints, blame, Backend};
use lilook_core::Document;

fn setup(src: &str) -> Option<(Backend<typst_kit::files::SystemFiles>, Document, String)> {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
    if render
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"))
    {
        eprintln!("lilaq unavailable; skipping");
        return None;
    }
    let msg = render.errors().next().map(|d| d.message.clone())?;
    assert!(
        render.errors().next().unwrap().range.is_none(),
        "this fixture exists because the error has no range"
    );
    Some((b, doc, msg))
}

/// The blamed argument is the one at fault, and the innocent series are spared.
#[test]
fn a_spanless_error_is_traced_to_the_argument_that_causes_it() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 8cm, height: 5cm, xlim: (),
  lq.plot((1, 2, 3), (1, 2, 3)),
  lq.plot((1, 2, 3), (4, 5, 6)),
  lq.scatter((1, 2, 3), (7, 8, 9)),
)
"#;
    let Some((mut b, doc, msg)) = setup(SRC) else {
        return;
    };
    assert!(msg.contains("Limit arrays"), "{msg}");

    let blames = blame::locate(&mut b, &doc, &msg);
    assert_eq!(blames.len(), 1, "exactly one thing is at fault: {blames:?}");
    let it = &blames[0];
    assert_eq!(it.argument.as_deref(), Some("xlim"));

    // The point of the exercise: a range the diagnostic did not have, in the
    // user's own bytes.
    assert_eq!(&SRC[it.range.clone()], "()");
    assert!(it.label.starts_with("xlim:"), "{}", it.label);
}

/// When two things only fail *together*, both are named rather than one guessed.
#[test]
fn an_error_that_needs_two_causes_names_both() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 8cm, height: 5cm, yscale: "log", ylim: (-1, 100),
  lq.plot((1, 2, 3), (1, 10, 100)),
)
"#;
    let Some((mut b, doc, msg)) = setup(SRC) else {
        return;
    };
    let blames = blame::locate(&mut b, &doc, &msg);
    let named: Vec<&str> = blames
        .iter()
        .filter_map(|b| b.argument.as_deref())
        .collect();
    assert!(
        named.contains(&"yscale") && named.contains(&"ylim"),
        "a log scale with a negative limit fails because of the pair: {named:?}"
    );
    // Every range points at real bytes.
    for b in &blames {
        assert!(b.range.end <= SRC.len() && SRC.is_char_boundary(b.range.start));
    }
}

/// A healthy document blames nothing, and costs nothing to ask about.
#[test]
fn nothing_is_blamed_when_nothing_is_wrong() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(lq.plot((1, 2, 3), (1, 2, 3)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(SRC);
    let mut hints = Hints::new();
    let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
    if render.failed() {
        return; // package unavailable
    }
    assert!(blame::locate(&mut b, &doc, "an error nobody had").is_empty());
}
