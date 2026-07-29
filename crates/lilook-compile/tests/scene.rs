//! Scene recovery: probe injection, the data<->page transform, and the
//! evaluated points of each series.

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

const DECLARED: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 14cm, height: 10cm, margin: 10pt)
#lq.diagram(
  width: 6cm, height: 4cm, xlim: (-3, 7), ylim: (100, 400),
  lq.plot((-2, 0, 5), (150, 200, 380)),
)
"#;

#[test]
fn recovers_declared_limits_and_the_transform() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(DECLARED);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(!render.failed(), "{:?}", render.diagnostics);
    assert_eq!(scenes.len(), 1, "one diagram, one scene");

    let s = &scenes[0];
    let t = &s.transform;
    assert!((t.x.min - -3.0).abs() < 0.01, "xmin {}", t.x.min);
    assert!((t.x.max - 7.0).abs() < 0.01, "xmax {}", t.x.max);
    assert!((t.y.min - 100.0).abs() < 0.05, "ymin {}", t.y.min);
    assert!((t.y.max - 400.0).abs() < 0.05, "ymax {}", t.y.max);

    // y grows upward in data space and downward on the page.
    assert!(t.y.scale < 0.0, "y scale should be negative: {}", t.y.scale);

    // The data area is 6cm x 4cm = 170.08 x 113.39 pt.
    let (w, h) = (s.area.2 - s.area.0, s.area.3 - s.area.1);
    assert!((w - 170.08).abs() < 1.0, "area width {w}");
    assert!((h - 113.39).abs() < 1.0, "area height {h}");

    // Round trip through the transform.
    for p in [(-2.0, 150.0), (0.0, 200.0), (5.0, 380.0)] {
        let back = t.to_data(t.to_page(p));
        assert!((back.0 - p.0).abs() < 1e-6 && (back.1 - p.1).abs() < 1e-6);
    }
}

#[test]
fn a_click_on_a_curve_names_the_call_site_that_drew_it() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(DECLARED);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    let s = &scenes[0];

    let plot = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .expect("a plot call");
    assert_eq!(s.series.len(), 1);
    assert_eq!(s.series[0].node, plot.id, "the probe carries the call site");
    assert_eq!(
        s.series[0].points,
        vec![(-2.0, 150.0), (0.0, 200.0), (5.0, 380.0)]
    );

    // Click two points away from a vertex, in page space.
    let near = s.transform.to_page((0.0, 200.0));
    let hit = s.hit((near.0 + 2.0, near.1 + 2.0), 8.0).expect("a hit");
    assert_eq!((hit.node, hit.index), (plot.id, 1));
    assert!(
        s.hit((near.0 + 500.0, near.1), 8.0).is_none(),
        "tolerance must be respected"
    );
}

/// The point of the series probe: figures are written with computed data, and
/// hit-testing has to work on them too.
#[test]
fn recovers_series_data_that_the_document_never_contained() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let x = lq.linspace(0, 10, num: 200)
#let y = x.map(t => calc.sin(t))
#lq.diagram(width: 6cm, height: 4cm, lq.plot(x, y, mark: none))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert_eq!(scenes.len(), 1);
    let pts = &scenes[0].series[0].points;
    assert_eq!(pts.len(), 200, "every point, not just the literal ones");
    assert!((pts[0].0 - 0.0).abs() < 1e-9 && (pts[0].1 - 0.0).abs() < 1e-9);
    let last = pts.last().unwrap();
    assert!((last.0 - 10.0).abs() < 1e-9);
    assert!((last.1 - (10f64).sin()).abs() < 1e-9);
}

#[test]
fn two_figures_in_one_document_stay_separate() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 16cm, height: 20cm, margin: 10pt)
#lq.diagram(width: 5cm, height: 3cm, xlim: (0, 1), ylim: (0, 1),
  lq.plot((0, 1), (0, 1)))

#lq.diagram(width: 8cm, height: 6cm, xlim: (-100, 100), ylim: (0, 5000),
  lq.plot((-50, 50), (1000, 4000)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert_eq!(scenes.len(), 2);

    let small = &scenes[0];
    let big = &scenes[1];
    assert!((small.transform.x.max - 1.0).abs() < 0.01);
    assert!(
        (big.transform.x.min - -100.0).abs() < 0.5,
        "{}",
        big.transform.x.min
    );
    assert!(
        (big.transform.y.max - 5000.0).abs() < 5.0,
        "{}",
        big.transform.y.max
    );
    // Different diagrams, different call sites, different areas on the page.
    assert_ne!(small.figure, big.figure);
    assert_ne!(small.series[0].node, big.series[0].node);
    assert!(big.area.1 > small.area.3, "the second figure is lower down");
}

/// If the injected markers changed the layout, everything above would be
/// measuring a figure the user never sees. This is the check that lets the
/// probe pass and the render pass be the same compile.
#[test]
fn probes_do_not_perturb_the_render() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(DECLARED);

    let clean = b.render(doc.text(), 2.0);
    if skip(&clean) {
        return;
    }
    let mut hints = Hints::new();
    let (probed, scenes) = b.render_scenes(&doc, 2.0, &mut hints);
    assert!(!scenes.is_empty());

    assert_eq!(clean.pages.len(), probed.pages.len());
    for (a, b) in clean.pages.iter().zip(&probed.pages) {
        assert_eq!(a.size_pt, b.size_pt, "page size changed");
        assert_eq!(
            a.image, b.image,
            "the injected probes changed the rendered pixels"
        );
    }
}

/// Auto limits are the common case, and lilook cannot know them in advance --
/// the first pass has to discover them and place its probes again.
#[test]
fn auto_limits_far_from_the_origin_still_resolve() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 14cm, height: 10cm, margin: 10pt)
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot((1000, 2000, 3000), (5e5, 7e5, 6e5)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    let t = &scenes[0].transform;
    // Whatever padding lilaq chose, the data has to sit inside the limits and
    // the transform has to put it inside the data area.
    assert!(
        t.x.min <= 1000.0 && t.x.max >= 3000.0,
        "x {:?}",
        (t.x.min, t.x.max)
    );
    assert!(
        t.y.min <= 5e5 && t.y.max >= 7e5,
        "y {:?}",
        (t.y.min, t.y.max)
    );
    let area = scenes[0].area;
    for p in [(1000.0, 5e5), (3000.0, 6e5)] {
        let q = t.to_page(p);
        assert!(
            q.0 >= area.0 - 1.0
                && q.0 <= area.2 + 1.0
                && q.1 >= area.1 - 1.0
                && q.1 <= area.3 + 1.0,
            "{p:?} -> {q:?} outside {area:?}"
        );
    }

    // Second time round the hint is in place, so the probes land first try --
    // and must recover the same axes. (Not bit-equal: the probes sit at
    // different data coordinates, so the arithmetic differs in the last ulp.)
    let t = *t;
    let (_, again) = b.render_scenes(&doc, 1.0, &mut hints);
    let u = &again[0].transform;
    assert!((u.x.min - t.x.min).abs() < 1e-6 && (u.x.max - t.x.max).abs() < 1e-6);
    assert!((u.y.min - t.y.min).abs() < 1e-3 && (u.y.max - t.y.max).abs() < 1e-3);
}
