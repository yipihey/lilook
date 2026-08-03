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
    let kinds: Vec<Decoration> = s.scenes[0].decorations.iter().map(|(k, _, _)| *k).collect();
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

/// The legend's box is measured, not assumed -- and a longer label makes a
/// wider box, which no fixed-constant guess could ever reflect.
#[test]
fn a_legends_extent_is_measured_and_grows_with_its_label() {
    use lilook_core::scene::Decoration;

    fn legend_extent(src: &str) -> (f64, f64) {
        let mut b = Backend::new(std::env::temp_dir(), "");
        let mut s = session(src);
        assert!(recompile(&mut b, &mut s), "lilaq must be available");
        s.scenes[0]
            .decorations
            .iter()
            .find(|(k, _, _)| *k == Decoration::Legend)
            .and_then(|(_, _, extent)| *extent)
            .expect("a measured legend extent")
    }

    let short = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(width: 8cm, height: 6cm, lq.plot((0, 1), (0, 1), label: [a]))
"#;
    let long = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(width: 8cm, height: 6cm, lq.plot((0, 1), (0, 1), label: [a rather much longer legend entry]))
"#;

    let (sw, _) = legend_extent(short);
    let (lw, _) = legend_extent(long);
    assert!(sw > 0.0 && lw > 0.0, "both must measure to something real");
    assert!(
        lw > sw * 2.0,
        "a much longer label must measure a much wider box: short={sw} long={lw}"
    );
}

/// A click lands inside the legend's measured box, not only within a few
/// points of its anchor corner -- the fix `hit_decoration` needed once a
/// real box existed to test against.
#[test]
fn a_click_anywhere_in_the_legend_box_selects_it() {
    use lilook_core::scene::Decoration;

    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)
#lq.diagram(
  width: 8cm,
  height: 6cm,
  legend: (position: top + left),
  lq.plot((0, 1, 2), (0, 1, 4), label: [a wide enough legend entry]),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut s = session(SRC);
    if !recompile(&mut b, &mut s) {
        return;
    }
    let scene = &s.scenes[0];
    let (_, at, extent) = scene
        .decorations
        .iter()
        .find(|(k, _, _)| *k == Decoration::Legend)
        .expect("the legend");
    let (w, h) = extent.expect("a measured extent");
    assert!(w > 10.0 && h > 5.0, "a real box: {w}x{h}");

    // The far corner of the box, well past the tolerance a point hit-test
    // alone would ever reach.
    let far_corner = (at.0 + w - 1.0, at.1 + h - 1.0);
    assert_eq!(
        scene.hit_decoration(far_corner, 3.0),
        Some(Decoration::Legend),
        "the box, not just its anchor, must be clickable"
    );
}
