//! Getting a figure out: PDF, SVG and PNG.
//!
//! The blocker for a first release. Until this existed the only ways out of
//! lilook were a screenshot and the clipboard, and no journal takes a raster
//! figure.
//!
//! Asserted on the bytes rather than on "it returned Ok": a function that hands
//! back an empty vector is also Ok, and every one of these has a header that says
//! what it is.

use lilook_compile::export::{export, Format};
use lilook_compile::{backend::Hints, Backend};
use lilook_core::Document;

const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#lq.diagram(width: 8cm, height: 5cm,
  xlabel: [$t$], ylabel: [flux],
  lq.plot((1, 2, 3, 4), (2, 4, 9, 16)),
)
"#;

fn compiled() -> Option<Backend<typst_kit::files::SystemFiles>> {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(SRC);
    let mut hints = Hints::new();
    let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
    if render
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"))
    {
        eprintln!("lilaq unavailable; skipping");
        return None;
    }
    assert!(!render.failed(), "{:?}", render.diagnostics);
    Some(b)
}

#[test]
fn every_format_produces_a_file_of_that_format() {
    let Some(b) = compiled() else { return };
    let doc = b.document().expect("a compiled document");

    let pdf = export(doc, Format::Pdf, 300.0).expect("pdf");
    assert_eq!(&pdf[..5], b"%PDF-", "a PDF header");
    assert!(pdf.len() > 2000, "{} bytes is too small", pdf.len());
    // The fonts have to travel with it, or the figure reflows on a machine that
    // does not have them -- which is most of them.
    assert!(
        pdf.windows(9).any(|w| w == b"/FontFile"),
        "the PDF must embed its fonts"
    );

    let svg = export(doc, Format::Svg, 300.0).expect("svg");
    let text = String::from_utf8(svg).expect("SVG is text");
    assert!(text.starts_with("<svg"), "{}", &text[..60.min(text.len())]);
    assert!(text.contains("</svg>"));
    // Vector, not a raster wrapped in an SVG tag: the plot line is a path.
    assert!(text.contains("<path"), "no paths in the SVG");

    let png = export(doc, Format::Png, 300.0).expect("png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "a PNG header");
    assert!(png.len() > 10_000, "{} bytes is too small", png.len());

    // Resolution is the one thing PNG has and the vector formats do not.
    let small = export(doc, Format::Png, 72.0).expect("png at 72");
    assert!(
        small.len() * 4 < png.len(),
        "300 ppi should be much larger than 72: {} vs {}",
        png.len(),
        small.len()
    );
}

/// A resolution nobody meant is refused rather than allowed to panic inside the
/// renderer, which unwraps its own allocation.
#[test]
fn an_absurd_resolution_is_declined() {
    let Some(b) = compiled() else { return };
    let doc = b.document().expect("a compiled document");
    let err = export(doc, Format::Png, 100_000.0).expect_err("should decline");
    assert!(err.contains("megapixels"), "{err}");
    // And the vector formats do not care about ppi at all.
    assert!(export(doc, Format::Pdf, 100_000.0).is_ok());
    assert!(export(doc, Format::Svg, 100_000.0).is_ok());
}

#[test]
fn a_format_is_recognised_from_a_filename() {
    assert_eq!(Format::of_path("fig.pdf"), Some(Format::Pdf));
    assert_eq!(Format::of_path("a/b/Fig.SVG"), Some(Format::Svg));
    assert_eq!(Format::of_path("plot.png"), Some(Format::Png));
    assert_eq!(Format::of_path("notes.txt"), None);
    assert_eq!(Format::of_path("noextension"), None);
    // The order a chooser offers them in: what a paper needs first.
    assert_eq!(Format::ALL[0], Format::Pdf);
}
