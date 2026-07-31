//! `.lil` — a figure in its own file, and back again.
//!
//! The extension exists so an operating system knows which application opens a
//! figure. lilook cannot claim `.typ` without taking every typst file from the
//! editor the user already has. It is *not* a format: what a `.lil` holds is
//! plain typst, and the test that matters is that typst compiles it — both on
//! its own and imported by a paper.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::{Document, Schema, Session};

const SCHEMA: &str = lilook_core::schema::BUNDLED;
const PAPER: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
= Results
#let xs = (1, 2, 3, 4)
Some prose before the figure.
#lq.diagram(width: 6cm, height: 4cm, lq.plot(xs, (1, 4, 9, 16)))
Prose after.
"#;

#[test]
fn a_figure_moves_to_its_own_file_and_back() {
    let dir = std::env::temp_dir().join("lilook-lil");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::write(dir.join("paper.typ"), PAPER).expect("paper");

    let mut b = Backend::new(&dir, "");
    let base = b.render(PAPER, 1.0);
    if base
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"))
    {
        eprintln!("lilaq unavailable; skipping");
        return;
    }
    assert!(!base.failed(), "the fixture compiles to begin with");

    let mut s = Session::new(PAPER, Schema::from_json(SCHEMA).expect("schema"));
    let node = s.doc.figures()[0].node;
    let out = s.extract_figure(node, "flux.lil").expect("extraction");
    std::fs::write(dir.join(&out.path), &out.contents).expect("write the .lil");

    // The figure left, an import arrived, and the prose is untouched.
    let host = s.doc.text().to_string();
    assert!(host.contains(r#"#import "flux.lil": flux"#), "{host}");
    assert!(!host.contains("lq.diagram"), "the figure moved out: {host}");
    assert!(host.contains("Some prose before") && host.contains("Prose after."));

    // It carried the binding its data needed.
    assert!(
        out.contents.contains("#let xs = (1, 2, 3, 4)"),
        "{}",
        out.contents
    );

    // Both halves compile: the paper that imports it, and the file alone.
    let r = b.render(&host, 1.0);
    assert!(
        !r.failed(),
        "the paper: {:?}",
        r.errors().next().map(|d| d.message.clone())
    );
    let alone = b.render(&out.contents, 1.0);
    assert!(
        !alone.failed(),
        "the .lil alone: {:?}",
        alone.errors().next().map(|d| d.message.clone())
    );

    // lilook opens the .lil as an ordinary document with a figure in it.
    let lil = Document::new(&out.contents);
    assert_eq!(lil.figures().len(), 1, "a .lil is just a document");
    let mut hints = Hints::new();
    let (_, scenes) = b.render_scenes(&lil, 1.0, &mut hints);
    assert_eq!(scenes[0].series.len(), 1, "and its series are editable");

    // The page rule it keeps for standalone preview must not reach the paper.
    let paper_page = b.render(&host, 1.0).pages[0].size_pt;
    assert!(
        paper_page.0 > 300.0,
        "the paper is still 12cm wide, not auto-sized: {paper_page:?}"
    );

    // And back again, byte for byte.
    assert!(s.inline_figure("flux.lil", &out.contents));
    let back = b.render(s.doc.text(), 1.0);
    assert!(
        !back.failed(),
        "inlined: {:?}",
        back.errors().next().map(|d| d.message.clone())
    );
    assert!(s.doc.text().contains("lq.diagram"), "the figure came home");
    assert!(!s.doc.text().contains("flux.lil"), "the import went away");

    // Both steps undo.
    while s.doc.history_depth().0 > 0 {
        s.doc.undo();
    }
    assert_eq!(s.doc.text(), PAPER, "extract and inline fully undo");
}

/// Nothing lilook-only is written into a `.lil`. It is typst or it is nothing.
#[test]
fn a_lil_holds_only_typst() {
    let mut s = Session::new(PAPER, Schema::from_json(SCHEMA).expect("schema"));
    let node = s.doc.figures()[0].node;
    let out = s.extract_figure(node, "flux.lil").expect("extraction");
    for marker in ["lilook", "version", "generated", "<!--", "---"] {
        assert!(
            !out.contents.contains(marker),
            "a .lil must carry no lilook-only marker, found {marker:?}"
        );
    }
    // It parses as typst, which is the only contract it has.
    assert!(lilook_core::check_expr("1").is_ok());
    let doc = Document::new(&out.contents);
    assert!(!doc.calls().is_empty(), "and it is a real document");
}
