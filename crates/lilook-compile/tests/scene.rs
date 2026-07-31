//! Scene recovery: probe injection, the data<->page transform, and the
//! evaluated points of each series.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::compile::AxisScale;
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

/// A log axis is not a straight line, and the recovered transform has to know it.
///
/// The truth used here needs no probe of its own: on an axis spanning `min..max`,
/// the *middle of the data area in page terms* is the arithmetic mean of the
/// limits if the axis is linear, and their geometric mean if it is logarithmic.
/// Fitting a line through two probe points gives the chord, so a log axis came
/// out linear -- wrong everywhere between the probes, which is hit-testing as well
/// as panning.
#[test]
fn a_log_axis_maps_logarithmically() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#let x = lq.linspace(1, 100, num: 40)
#lq.diagram(
  width: 9cm, height: 5cm,
  yscale: "log",
  lq.plot(x, x.map(n => n * n), mark: none),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = lilook_compile::backend::Hints::new();
    let doc = Document::new(src);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());
    let s = &scenes[0];

    // x is linear here, y is log; both are recovered from the same compile, so
    // this checks the detection as much as the arithmetic.
    let (x0, x1) = (s.transform.x.min, s.transform.x.max);
    let (y0, y1) = (s.transform.y.min, s.transform.y.max);
    assert!(y0 > 0.0, "a log axis cannot include zero: {y0}");

    // The middle of the data area, in page points.
    let mid_x_page = (s.area.0 + s.area.2) / 2.0;
    let mid_y_page = (s.area.1 + s.area.3) / 2.0;
    let mid_x = s.transform.x.to_data(mid_x_page);
    let mid_y = s.transform.y.to_data(mid_y_page);

    let arithmetic = (x0 + x1) / 2.0;
    assert!(
        (mid_x - arithmetic).abs() < (x1 - x0) * 0.02,
        "x is linear, so the middle of the frame is the arithmetic mean: \
         got {mid_x}, expected {arithmetic}"
    );

    let geometric = (y0 * y1).sqrt();
    let arithmetic_y = (y0 + y1) / 2.0;
    assert!(
        (mid_y / geometric - 1.0).abs() < 0.05,
        "y is a log axis, so the middle of the frame is the geometric mean \
         {geometric:.3}, not the arithmetic mean {arithmetic_y:.3}; got {mid_y:.3}"
    );

    // And the round trip holds across the whole axis, which a chord fit does not.
    for f in [0.0, 0.1, 0.25, 0.5, 0.75, 1.0] {
        let page = s.area.1 + f * (s.area.3 - s.area.1);
        let data = s.transform.y.to_data(page);
        let back = s.transform.y.to_page(data);
        assert!(
            (back - page).abs() < 0.01,
            "y did not round trip at {f}: {page} -> {data} -> {back}"
        );
        assert!(data > 0.0, "a log axis produced {data} at {f}");
    }
}

/// A datetime axis is not numbers, and lilook says so instead of guessing.
///
/// lilaq plots `datetime` coordinates. Everything lilook does in data space
/// assumes numbers, so the probe recovers none of them -- and a pan would write
/// `xlim: (0, 100)`, which *compiles* and silently swaps a calendar axis for a
/// numeric one. Compiling is not the same as being right, so the scene records
/// that the axis is not numeric and the canvas declines the data pan.
#[test]
fn a_datetime_axis_is_marked_non_numeric() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#let days = (
  datetime(year: 2026, month: 1, day: 1),
  datetime(year: 2026, month: 2, day: 1),
  datetime(year: 2026, month: 3, day: 1),
)
#lq.diagram(width: 7cm, height: 4cm, lq.plot(days, (3, 5, 4)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = lilook_compile::backend::Hints::new();
    let doc = Document::new(src);

    // The user's document is fine, and lilook's probes must not break it.
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(
        !r.failed(),
        "probing a datetime figure broke it: {:?}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    // The figure still *draws*: the retry drops the scale probes, so the layout is
    // the user's own and the page rasterises. Before that it was 0x0 -- the page
    // had grown too large to allocate, and `typst_render` panicked inside an
    // `unwrap` that no caller can catch.
    assert!(
        r.pages.iter().all(|p| p.image.width > 0),
        "a datetime figure has to render"
    );

    let s = &scenes[0];
    // Neither axis is usable in data space: without the scale probes there is no
    // transform to solve for either. Coarser than "x is dates, y is numbers", and
    // honest -- what matters is that nothing offers a gesture it cannot express.
    assert_eq!(s.numeric, (false, false));
    assert!(s.series[0].points.is_empty(), "no numbers were recovered");
    // The frame is still known, so the diagram can be selected and resized.
    assert!(s.area.2 > s.area.0 && s.area.3 > s.area.1, "{:?}", s.area);

    // An ordinary numeric figure is unaffected.
    let plain = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#lq.diagram(width: 7cm, height: 4cm, lq.plot((0, 1, 2), (3, 5, 4)))
"#;
    let (_, scenes) = b.render_scenes(&Document::new(plain), 1.0, &mut hints);
    assert_eq!(scenes[0].numeric, (true, true));
    assert_eq!(scenes[0].series[0].points.len(), 3);
}

/// A mesh's field, recovered the same way whether it was written as a function
/// or as an explicit array.
///
/// The orientation is the thing under test. lilaq documents `z` as m rows by n
/// columns for n x-values and m y-values, and lilook flattens row-major so that
/// `hit_mesh`'s single index names one cell. The axes here are deliberately
/// *unequal* -- 5 columns against 3 rows -- because a transposed convention
/// would not merely read the wrong value, it would fail to compile at all, which
/// is a much better failure than a plausible wrong number.
#[test]
fn a_meshs_field_is_recovered_row_major_over_y() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let xs = (0, 1, 2, 3, 4)
#let ys = (10, 20, 30)
#let f = (x, y) => x + y
#lq.diagram(width: 5cm, height: 4cm,
  lq.colormesh(xs, ys, f),
)
#lq.diagram(width: 5cm, height: 4cm,
  lq.colormesh(xs, ys, ys.map(yy => xs.map(xx => f(xx, yy)))),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(SRC);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    // If the array orientation were the other way round, this is where it ends.
    assert!(!render.failed(), "{:?}", render.diagnostics);
    assert_eq!(scenes.len(), 2);

    for (which, scene) in ["function", "array"].iter().zip(&scenes) {
        let g = &scene.series[0];
        assert_eq!(g.grid, Some((5, 3)), "{which}");
        let z = g
            .channel("z")
            .unwrap_or_else(|| panic!("{which}: no field"));
        assert_eq!(z.len(), 15, "{which}: one value per cell");
        for row in 0..3 {
            for col in 0..5 {
                let want = col as f64 + (10 * (row + 1)) as f64;
                assert_eq!(z[row * 5 + col], want, "{which} at ({col},{row})");
                assert_eq!(g.field_at(row * 5 + col), Some(want), "{which}");
            }
        }
    }

    // Past the last cell there is no value, rather than a wrapped one.
    assert_eq!(scenes[0].series[0].field_at(15), None);
}

/// `lq.mesh` is a data helper, not a plot, whatever its name suggests.
///
/// It lives in lilaq's `math.typ`, evaluates a function over a grid and returns
/// the field; it puts no ink on the page. Listing it among the mesh-shaped
/// *series* made it a phantom: the idiomatic `#let z = lq.mesh(xs, ys, f)` fed
/// to a colormesh showed **two** entries under one diagram, and the extra one
/// reported a plausible "6x4 grid" because the probe read its first two slots --
/// which happen to be the same axes. Plausible is the dangerous part; clicking it
/// selected a call that draws nothing.
#[test]
fn the_mesh_helper_is_not_a_series() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let xs = lq.linspace(-2, 2, num: 6)
#let ys = lq.linspace(-1, 1, num: 4)
#let zs = lq.mesh(xs, ys, (x, y) => x * y)
#lq.diagram(width: 6cm, height: 4cm, lq.colormesh(xs, ys, zs))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(SRC);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(!render.failed(), "{:?}", render.diagnostics);

    let mesh = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "mesh")
        .expect("the helper is still in the document");
    assert!(!mesh.is_xy_series(), "a helper is not a series");

    // One diagram, one thing drawn in it.
    let figure = &doc.figures()[0];
    assert_eq!(figure.series.len(), 1, "only the colormesh draws");
    let drawn = doc
        .calls()
        .iter()
        .find(|c| c.id == figure.series[0])
        .unwrap();
    assert_eq!(drawn.short_name(), "colormesh");

    // And the field still arrives, since `lq.mesh` builds it the same way the
    // probe reads it -- rows over y, columns over x.
    let g = &scenes[0].series[0];
    assert_eq!(g.grid, Some((6, 4)));
    let z = g.channel("z").expect("the field");
    let (xs, ys) = (g.channel("x").unwrap(), g.channel("y").unwrap());
    for row in 0..4 {
        for col in 0..6 {
            let want = xs[col] * ys[row];
            assert!(
                (z[row * 6 + col] - want).abs() < 1e-12,
                "({col},{row}): {} vs {want}",
                z[row * 6 + col]
            );
        }
    }
}

/// A scale lilook cannot model is declined, not guessed at.
///
/// lilook knows two scales. lilaq ships symlog as well, and `lq.scale.scale` lets
/// anyone define one — so "does the midpoint bend?" is a coin flip between the
/// only two answers available, and it came up wrong.
///
/// The scale here squashes: `v / (1 + |v|)`, defined for every real so the probes
/// can be placed anywhere, and nothing like either of lilook's. Before the guard
/// it was recovered as **linear with `min 0.846` and `max -1.749`** — a maximum
/// below the minimum. Nothing failed and nothing was reported; every pan and drag
/// on that figure would have written nonsense into the user's document.
///
/// What catches it is the cheapest invariant available, and one that needs no
/// knowledge of which scales lilaq has: an axis whose recovered maximum is below
/// its minimum is not a transform. The figure then degrades to frame-only, the
/// path a datetime axis already takes — it still draws, selects and resizes, and
/// the gestures fall back to moving the view instead of rewriting limits.
#[test]
fn a_scale_lilook_cannot_model_is_declined_rather_than_guessed() {
    const SQUASH: &str =
        r#"yscale: lq.scale.scale(v => v / (1 + calc.abs(v)), u => u / (1 - calc.abs(u))),"#;
    let src = |scale: &str| {
        format!(
            r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 6cm, height: 4cm,
  {scale}
  lq.plot((1, 2, 3, 4), (1, 40, 900, 40000)),
)
"#
        )
    };
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();

    let doc = Document::new(src(SQUASH));
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    // The figure still draws. Only lilook's data-space arithmetic declines.
    assert!(!render.failed(), "{:?}", render.diagnostics);
    assert_eq!(scenes.len(), 1, "the frame is still recovered");
    assert!(
        !scenes[0].numeric.1,
        "an unmodelled y scale must not be reported as usable in data space"
    );
    assert!(
        !scenes[0].area.0.is_nan(),
        "and the frame itself is still usable, so the diagram can be resized"
    );

    // The two scales lilook does model are unaffected, so the guard has not
    // simply switched data-space editing off.
    for (scale, want) in [
        ("", AxisScale::Linear),
        (r#"yscale: "log","#, AxisScale::Log),
    ] {
        let doc = Document::new(src(scale));
        let mut h = Hints::new();
        let (r, s) = b.render_scenes(&doc, 1.0, &mut h);
        assert!(!r.failed(), "{scale}: {:?}", r.diagnostics);
        assert!(
            s[0].numeric.0 && s[0].numeric.1,
            "{scale} should be numeric"
        );
        assert_eq!(s[0].transform.y.kind, want, "{scale}");
        assert!(
            s[0].transform.y.min < s[0].transform.y.max,
            "{scale}: {} .. {}",
            s[0].transform.y.min,
            s[0].transform.y.max
        );
    }
}
