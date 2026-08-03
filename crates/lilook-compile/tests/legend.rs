//! Legend placement and styling, driven exactly as the GUI or an MCP agent
//! drives it: through `Session`, over a real compile.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::{Schema, Session};

fn skip(r: &lilook_compile::Render) -> bool {
    let missing = r
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"));
    if missing {
        eprintln!("lilaq package unavailable; skipping");
    }
    missing
}

fn session(src: &str) -> Session {
    let schema = Schema::from_json(lilook_core::schema::BUNDLED).expect("bundled schema");
    Session::new(src, schema)
}

fn recompile(b: &mut Backend<typst_kit::files::SystemFiles>, s: &mut Session) -> bool {
    let doc = lilook_core::Document::new(s.doc.text());
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return false;
    }
    assert!(
        !render.failed(),
        "{:?}",
        render.errors().collect::<Vec<_>>()
    );
    s.scenes = scenes;
    true
}

/// All the data sits in a tight cluster near the top-right corner, with the
/// axis limits pinned wide open around it -- so the bottom-left quadrant is
/// unambiguously the emptiest, not merely the least dense.
#[test]
fn auto_position_avoids_the_data() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(
  width: 8cm,
  height: 6cm,
  xlim: (0, 10),
  ylim: (0, 10),
  lq.plot((9, 9.2, 9.4, 9.5), (9, 9.2, 9.4, 9.5), label: [corner]),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut s = session(SRC);
    if !recompile(&mut b, &mut s) {
        return;
    }
    let figure = s.scenes[0].figure;

    s.auto_position_legend(figure);
    assert!(
        s.doc.text().contains("legend:"),
        "auto-position must write a legend value: {}",
        s.doc.text()
    );
    assert!(
        s.doc.text().contains("bottom + left"),
        "the far corner from the data is the only truly empty one: {}",
        s.doc.text()
    );

    // What was written still compiles -- the check every gesture test here
    // makes, because a value that merely round-trips through `check_expr`
    // has been wrong before.
    assert!(recompile(&mut b, &mut s));
}

/// Placing the legend a second time must not erase styling the first
/// placement (or the user) wrote -- `legend:` used to be overwritten
/// wholesale on every write.
#[test]
fn repositioning_preserves_other_fields() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(
  width: 8cm,
  height: 6cm,
  legend: (position: top + left, fill: black.transparentize(30%)),
  lq.plot((0, 1, 2), (0, 1, 4), label: [a]),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut s = session(SRC);
    if !recompile(&mut b, &mut s) {
        return;
    }
    let figure = s.scenes[0].figure;

    s.auto_position_legend(figure);
    assert!(
        s.doc.text().contains("black.transparentize(30%)"),
        "fill must survive a reposition: {}",
        s.doc.text()
    );
    assert!(recompile(&mut b, &mut s));
}

/// A title and axis labels are recovered as decorations exactly like a
/// legend -- same `Scene.decorations`, same selection call -- which is what
/// the tree and the canvas highlight both need to make them as pickable as
/// a plot already is.
#[test]
fn title_and_axis_labels_are_selectable_decorations() {
    use lilook_core::scene::Decoration;

    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(
  width: 8cm,
  height: 6cm,
  title: [Growth],
  xlabel: [Time],
  ylabel: [Count],
  lq.plot((0, 1, 2), (0, 1, 4), label: [a]),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut s = session(SRC);
    if !recompile(&mut b, &mut s) {
        return;
    }
    let figure = s.scenes[0].figure;
    let kinds: Vec<Decoration> = s.scenes[0].decorations.iter().map(|(k, _)| *k).collect();
    for want in [
        Decoration::Title,
        Decoration::XLabel,
        Decoration::YLabel,
        Decoration::Legend,
    ] {
        assert!(kinds.contains(&want), "missing {want:?} in {kinds:?}");
    }

    for kind in [Decoration::Title, Decoration::XLabel, Decoration::YLabel] {
        s.select_decoration(figure, kind);
        assert_eq!(s.selected, figure);
        assert_eq!(s.selected_decoration, Some((figure, kind)));
    }
}
