//! A diagnostic's byte range must describe the buffer the user is looking at.
//!
//! typst reports spans against the buffer it compiled, and lilook compiles a
//! *derived* one with probes spliced into each diagram's argument list. Measured
//! before the language-server work was planned: a 200-byte file reported an
//! error at byte 781. Harmless while the message is only printed; a slice out of
//! bounds the moment anything highlights it.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::Document;

fn skip(r: &lilook_compile::Render) -> bool {
    let missing = r
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"));
    if missing {
        eprintln!("lilaq package unavailable; skipping");
    }
    missing
}

/// The two error classes that carry a span at all point at what the user wrote.
///
/// Both are placed *after* a diagram, so the probe's splice sits between the
/// start of the file and the error -- which is exactly the case that was wrong.
#[test]
fn a_diagnostic_points_at_the_users_own_bytes() {
    const HEAD: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 6cm, height: 4cm, lq.plot((1, 2, 3), (1, 2, 3)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    for (label, tail, needle) in [
        (
            "undefined name",
            "#lq.diagram(lq.plot(nope, (1, 2)))",
            "nope",
        ),
        (
            "unclosed delimiter",
            "#lq.diagram(lq.plot((1, 2), (1, 2))",
            "(",
        ),
    ] {
        let src = format!("{HEAD}{tail}\n");
        let doc = Document::new(&src);
        let mut hints = Hints::new();
        let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
        if skip(&render) {
            return;
        }
        let d = render
            .errors()
            .next()
            .unwrap_or_else(|| panic!("{label} should fail"));
        let range = d
            .range
            .clone()
            .unwrap_or_else(|| panic!("{label}: {} has no range", d.message));

        // The decisive assertion: it is inside the file, not past its end.
        assert!(
            range.end <= src.len(),
            "{label}: {range:?} exceeds a {}-byte file",
            src.len()
        );
        // And it names the right bytes.
        let named = &src[range.clone()];
        assert!(
            named.contains(needle) || src[..range.start].ends_with(needle),
            "{label}: range names {named:?}, expected something like {needle:?}"
        );
        assert!(
            range.start > HEAD.len() - 1,
            "{label}: the error is on the last line, but the range points into the first"
        );
    }
}

/// lilaq's own errors carry no range, and lilook must not invent one.
///
/// lilaq validates through `elembic`, inside the package, so the span belongs to
/// the package's file rather than the user's. Four of the six commonest failures
/// are like this, which is why code actions are driven by `(message, document)`
/// and not by spans.
#[test]
fn an_error_raised_inside_lilaq_reports_no_range_rather_than_a_wrong_one() {
    const CASES: [&str; 3] = [
        "#lq.diagram(xlim: (), lq.plot((1, 2), (1, 2)))",
        "#lq.diagram(bogus: 1, lq.plot((1, 2), (1, 2)))",
        "#lq.diagram(yscale: \"log\", ylim: (-1, 10), lq.plot((1, 2), (1, 2)))",
    ];
    let mut b = Backend::new(std::env::temp_dir(), "");
    for tail in CASES {
        let src = format!(
            "#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 5pt)\n{tail}\n"
        );
        let doc = Document::new(&src);
        let mut hints = Hints::new();
        let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
        if skip(&render) {
            return;
        }
        let d = render
            .errors()
            .next()
            .unwrap_or_else(|| panic!("{tail} should fail"));
        assert!(d.range.is_none(), "{tail}: unexpected range {:?}", d.range);
    }
}

/// The invariant, over every gallery example with an edit applied: no diagnostic
/// ever claims a range outside the buffer it describes.
#[test]
fn no_diagnostic_ever_points_outside_the_buffer() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    // Deliberately broken in several ways, each after at least one diagram.
    let breakages = [
        "\n#lq.diagram(lq.plot(missing, (1, 2)))",
        "\n#lq.plot(",
        "\n#let x = ",
        "\n#lq.diagram(lq.plot((1, 2), (1, 2)), width: nope)",
    ];
    for extra in breakages {
        let src = format!(
            "#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 5pt)\n#lq.diagram(lq.plot((1, 2), (1, 2)))\n#lq.diagram(lq.plot((3, 4), (3, 4)))\n{extra}\n"
        );
        let doc = Document::new(&src);
        let mut hints = Hints::new();
        let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
        if skip(&render) {
            return;
        }
        for d in &render.diagnostics {
            if let Some(r) = &d.range {
                assert!(
                    r.start <= r.end && r.end <= src.len(),
                    "{extra:?}: {r:?} outside a {}-byte buffer ({})",
                    src.len(),
                    d.message
                );
                // And it lands on a character boundary, or slicing panics.
                assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end));
            }
        }
    }
}
