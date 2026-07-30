//! End-to-end: a gesture goes through the document as intents, and the result
//! is still a document typst will compile.
//!
//! The trailing-comma insertion bug passed the round-trip test and was caught
//! only by recompiling the output, so every editing path gets this check.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::{Document, Intent};

fn skip(r: &lilook_compile::Render) -> bool {
    let missing = r
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"));
    if missing {
        eprintln!("lilaq package unavailable; skipping");
    }
    missing
}

const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 12cm, height: 9cm, margin: 8pt)

// a comment that must survive the whole gesture
#lq.diagram(
  width: 7cm,
  height: 5cm,
  lq.plot((0, 1, 2, 3), (0, 1, 4, 9), stroke: red),
)
"#;

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[test]
fn a_pan_then_a_point_drag_still_compiles_and_fully_undoes() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut doc = Document::new(SRC);
    let mut hints = Hints::new();

    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(!render.failed());
    let figure = scenes[0].figure;
    let series = scenes[0].series[0].node;
    let before = scenes[0].transform.x.min;

    // --- a pan: the first frame inserts the limits, the rest set them ---
    doc.begin("pan");
    for i in 1..=10 {
        let d = i as f64 * 0.05;
        for (param, (lo, hi)) in [
            ("xlim", (before + d, before + 3.0 + d)),
            ("ylim", (d, 9.0 + d)),
        ] {
            let value = format!("({}, {})", num(lo), num(hi));
            let present = doc
                .call(figure)
                .is_some_and(|c| c.named.iter().any(|a| a.name == param));
            let intent = if present {
                Intent::SetNamedArg {
                    node: figure,
                    param: param.into(),
                    value,
                }
            } else {
                Intent::InsertNamedArg {
                    node: figure,
                    param: param.into(),
                    value,
                }
            };
            doc.apply(intent).unwrap();
        }
    }
    doc.commit();
    assert_eq!(doc.history_depth().0, 1, "the pan is one undo step");

    // --- a point drag on the literal array ---
    doc.begin("drag point");
    for i in 1..=8 {
        let (x, y) = (1.0 + i as f64 * 0.05, 1.0 - i as f64 * 0.02);
        doc.apply(Intent::SetArrayElement {
            node: series,
            arg: 0,
            element: 1,
            value: num(x),
        })
        .unwrap();
        doc.apply(Intent::SetArrayElement {
            node: series,
            arg: 1,
            element: 1,
            value: num(y),
        })
        .unwrap();
    }
    doc.commit();
    assert_eq!(doc.history_depth().0, 2, "two gestures, two undo steps");

    // The user's comment and layout survived.
    assert!(doc
        .text()
        .contains("// a comment that must survive the whole gesture"));

    // The edited source still compiles, and the new point is where we put it.
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(
        !render.failed(),
        "the edited document no longer compiles: {:?}",
        render.errors().collect::<Vec<_>>()
    );
    let moved = &scenes[0].series[0].points[1];
    assert!(
        (moved.0 - 1.4).abs() < 1e-9 && (moved.1 - 0.84).abs() < 1e-9,
        "{moved:?}"
    );
    assert!((scenes[0].transform.x.min - (before + 0.5)).abs() < 0.01);

    // Two undos, byte for byte.
    assert!(doc.undo());
    assert!(doc.undo());
    assert_eq!(doc.text(), SRC);

    let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!render.failed(), "the undone document must compile too");
}

#[test]
fn deleting_a_series_leaves_a_document_that_compiles() {
    let src = SRC.replace(
        "  lq.plot((0, 1, 2, 3), (0, 1, 4, 9), stroke: red),",
        "  lq.plot((0, 1, 2, 3), (0, 1, 4, 9), stroke: red),\n  lq.plot((0, 3), (9, 0)),",
    );
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut doc = Document::new(&src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert_eq!(scenes[0].series.len(), 2);

    let second = doc
        .calls()
        .iter()
        .filter(|c| c.callee == "lq.plot")
        .nth(1)
        .unwrap()
        .id;
    doc.begin("delete");
    doc.apply(Intent::RemoveNode { node: second }).unwrap();
    doc.commit();

    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(
        !render.failed(),
        "removing a series broke the argument list: {:?}\n{}",
        render.errors().collect::<Vec<_>>(),
        doc.text()
    );
    assert_eq!(scenes[0].series.len(), 1);

    doc.undo();
    assert_eq!(doc.text(), src);
}

/// Materialising a computed data slot is the largest edit lilook makes, and the
/// one most able to produce a file that no longer compiles. The values come
/// from the compiler, so this asserts the round trip: recovered points ->
/// array literal -> recompiled -> the same points.
#[test]
fn materialising_computed_data_keeps_the_figure_identical() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let x = lq.linspace(0, 4, num: 9)
#lq.diagram(width: 6cm, height: 4cm, lq.plot(x, x.map(t => t * t), mark: none))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut doc = Document::new(src);
    let mut hints = Hints::new();

    let (render, scenes) = b.render_scenes(&doc, 2.0, &mut hints);
    if skip(&render) {
        return;
    }
    let series = scenes[0].series[0].node;
    let before = scenes[0].series[0].points.clone();
    assert_eq!(before.len(), 9);
    assert!(!doc.call(series).unwrap().has_literal_points());
    let pixels_before = render.pages[0].image.clone();

    doc.begin("materialise");
    for (index, values) in [
        (0usize, before.iter().map(|p| p.0).collect::<Vec<_>>()),
        (1, before.iter().map(|p| p.1).collect()),
    ] {
        let value = format!(
            "({})",
            values
                .iter()
                .map(|v| num(*v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        doc.apply(Intent::SetPositionalArg {
            node: series,
            index,
            value,
        })
        .unwrap();
    }
    doc.commit();

    // Now the points are literals, so a drag could move them.
    assert!(doc.call(series).unwrap().has_literal_points());

    let (render, scenes) = b.render_scenes(&doc, 2.0, &mut hints);
    assert!(
        !render.failed(),
        "materialised source does not compile: {:?}",
        render.errors().collect::<Vec<_>>()
    );
    assert_eq!(scenes[0].series[0].points, before, "the data changed");
    assert_eq!(
        render.pages[0].image, pixels_before,
        "the figure is not the one the user was looking at"
    );

    doc.undo();
    assert_eq!(doc.text(), src);
}

/// Set rules end to end: add one the way the panel does, edit a field, and
/// check that the figure actually changed and the document still compiles.
#[test]
fn adding_and_editing_a_set_rule_restyles_the_figure() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 5cm, height: 3cm, lq.plot((0, 1, 2), (0, 1, 4)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut doc = Document::new(src);
    let mut hints = Hints::new();

    let (render, _) = b.render_scenes(&doc, 2.0, &mut hints);
    if skip(&render) {
        return;
    }
    let before = render.pages[0].image.clone();
    assert!(doc.set_rules().is_empty());

    // The panel inserts a document-level rule after the import.
    let at = src.find('\n').unwrap();
    doc.begin("add style");
    doc.apply(Intent::ReplaceRange {
        range: at..at,
        value: "\n#show: lq.set-tick(stroke: red)".into(),
    })
    .unwrap();
    doc.commit();

    let rules = doc.set_rules();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].element, "tick");
    assert!(rules[0].document_level);

    let (render, _) = b.render_scenes(&doc, 2.0, &mut hints);
    assert!(
        !render.failed(),
        "the set rule does not compile: {:?}",
        render.errors().collect::<Vec<_>>()
    );
    assert_ne!(
        render.pages[0].image, before,
        "a tick style rule should have changed the figure"
    );

    // And it is edited through the ordinary named-argument intent.
    doc.apply(Intent::SetNamedArg {
        node: rules[0].node,
        param: "stroke".into(),
        value: "blue".into(),
    })
    .unwrap();
    let (render, _) = b.render_scenes(&doc, 2.0, &mut hints);
    assert!(!render.failed());

    while doc.undo() {}
    assert_eq!(doc.text(), src);
}

/// Copy/paste across documents, end to end: the fragment carries the binding it
/// depends on, and the pasted result compiles and draws the same series twice.
#[test]
fn a_pasted_series_carries_the_binding_it_needs() {
    let from = r##"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let xs = lq.linspace(0, 4, num: 5)
#let accent = rgb("#4c72b0")
#lq.diagram(width: 5cm, height: 3cm, lq.plot(xs, xs.map(t => t * t), stroke: accent))
"##;
    let into = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 5cm, height: 3cm, lq.plot((0, 1), (1, 0)))
"#;

    let source = Document::new(from);
    let call = source
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .unwrap();
    let fragment = source.text()[call.range.clone()].to_string();

    // What the shell computes on copy.
    let free = source.free_identifiers(call.range.clone());
    assert!(free.contains(&"xs".to_string()) && free.contains(&"accent".to_string()));
    let carried: Vec<String> = free
        .iter()
        .filter(|n| *n != "lq")
        .filter_map(|n| source.binding_of(n).map(|r| source.text()[r].to_string()))
        .collect();
    assert_eq!(carried.len(), 2, "{carried:?}");

    // What it does on paste. The call goes in first: call-site ids are indices
    // into a document-order walk, so inserting a binding above the figure would
    // renumber the figure out from under the id.
    let mut doc = Document::new(into);
    let figure = doc.figures()[0].node;
    let at = into.find('\n').unwrap();
    doc.begin("paste");
    doc.apply(Intent::InsertPositionalArg {
        node: figure,
        value: fragment,
    })
    .unwrap();
    for definition in &carried {
        doc.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!("\n{definition}"),
        })
        .unwrap();
    }
    doc.commit();

    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(
        !render.failed(),
        "the pasted document does not compile: {:?}\n{}",
        render.errors().collect::<Vec<_>>(),
        doc.text()
    );
    assert_eq!(scenes[0].series.len(), 2, "both series should be drawn");
    let pasted = &scenes[0].series[1];
    assert_eq!(pasted.points.len(), 5, "the carried binding was evaluated");
    assert_eq!(pasted.points[2], (2.0, 4.0));

    assert_eq!(doc.history_depth().0, 1, "a paste is one undo step");
    doc.undo();
    assert_eq!(doc.text(), into);
}

/// Pasting a fragment whose bindings are missing must still produce a document
/// -- with a diagnostic that names what is unresolved, not a silent failure.
#[test]
fn pasting_an_unresolved_fragment_reports_rather_than_hides() {
    let into = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 5cm, height: 3cm, lq.plot((0, 1), (1, 0)))
"#;
    let mut doc = Document::new(into);
    let figure = doc.figures()[0].node;
    doc.begin("paste");
    doc.apply(Intent::InsertPositionalArg {
        node: figure,
        value: "lq.plot(missing, other)".into(),
    })
    .unwrap();
    doc.commit();

    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let (render, _) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    let messages: Vec<String> = render.errors().map(|d| d.message.clone()).collect();
    assert!(
        messages.iter().any(|m| m.contains("missing")),
        "the error should name the unresolved binding: {messages:?}"
    );
    // And it is one undo away.
    doc.undo();
    assert_eq!(doc.text(), into);
}

/// Resizing by the frame, end to end: the recovered data area follows the
/// gesture, the unit the user wrote survives, and it is one undo step.
#[test]
fn dragging_the_frame_resizes_the_figure_in_the_users_own_unit() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 20cm, height: 14cm, margin: 10pt)
#lq.diagram(width: 6cm, height: 4cm, lq.plot((0, 1, 2), (0, 1, 4)))
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut doc = Document::new(src);
    let mut hints = Hints::new();

    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    let figure = scenes[0].figure;
    let area = scenes[0].area;
    let (w0, h0) = (area.2 - area.0, area.3 - area.1);
    // 6cm x 4cm.
    assert!(
        (w0 - 170.08).abs() < 1.0 && (h0 - 113.39).abs() < 1.0,
        "{w0} {h0}"
    );

    // What the canvas emits for a corner drag of +28.35 pt (1cm) each way, and
    // what the editor writes for it: centimetres, because that is what was there.
    doc.begin("resize");
    for (param, pt) in [("width", w0 + 28.3464567), ("height", h0 + 28.3464567)] {
        let cm = pt / 28.3464567;
        doc.apply(Intent::SetNamedArg {
            node: figure,
            param: param.into(),
            value: format!("{}cm", num(cm)),
        })
        .unwrap();
    }
    doc.commit();
    assert!(doc.text().contains("width: 7cm"), "{}", doc.text());
    assert!(doc.text().contains("height: 5cm"), "{}", doc.text());

    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(
        !render.failed(),
        "{:?}",
        render.errors().collect::<Vec<_>>()
    );
    let area = scenes[0].area;
    let (w1, h1) = (area.2 - area.0, area.3 - area.1);
    assert!(
        (w1 - (w0 + 28.35)).abs() < 1.0 && (h1 - (h0 + 28.35)).abs() < 1.0,
        "the frame did not follow the gesture: {w1} {h1}"
    );

    assert_eq!(doc.history_depth().0, 1);
    doc.undo();
    assert_eq!(doc.text(), src);
}

/// What the data emitter writes has to be something typst evaluates, not merely
/// something typst parses. `check_expr` covers the parser; only a compile covers
/// `float.nan` being a real value and `1e-300` being a real literal.
#[test]
fn emitted_data_values_survive_a_real_compile() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let awkward = [
        0.0,
        -0.0,
        1.234e-9,
        1e-300,
        5e-324,
        f64::MAX,
        f64::MIN,
        std::f64::consts::PI,
        6.02214076e23,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let array = lilook_core::data_array_source(&awkward).unwrap();
    // `#assert` makes the compile itself the assertion: a value typst read as
    // something other than a float would change the count or the type.
    let source = format!(
        r#"#set page(width: 4cm, height: 2cm)
#let vals = {array}
#assert(vals.len() == {})
#assert(vals.all(v => type(v) == float or type(v) == int))
#assert(vals.at(9) != vals.at(9), message: "nan must not compare equal to itself")
#assert(vals.at(10) > 0 and vals.at(11) < 0, message: "the infinities kept their signs")
ok
"#,
        awkward.len()
    );
    let r = b.render(&source, 1.0);
    assert!(
        !r.failed(),
        "{:?}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    // And the single-element form, which is the one `(1)` got wrong: a scalar
    // there is not a compile error, it is a *plot* of nothing.
    let one = lilook_core::data_array_source(&[2.5]).unwrap();
    assert_eq!(one, "(2.5,)");
    let source = format!(
        r#"#set page(width: 4cm, height: 2cm)
#assert(type({one}) == array and {one}.len() == 1)
ok
"#
    );
    assert!(!b.render(&source, 1.0).failed());
}

/// A one-point series is a real figure, and its point is recoverable -- the case
/// the old emitter turned into a scalar.
#[test]
fn a_single_point_series_plots_and_is_recovered() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let src = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot({}, {}, stroke: red)
)
"#,
        lilook_core::data_array_source(&[1.5]).unwrap(),
        lilook_core::data_array_source(&[2.5]).unwrap(),
    );
    let doc = Document::new(&src);
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(
        !render.failed(),
        "{:?}",
        render
            .errors()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(scenes.len(), 1);
    assert_eq!(scenes[0].series.len(), 1);
    assert_eq!(scenes[0].series[0].points.len(), 1);
    let (x, y) = scenes[0].series[0].points[0];
    assert!(
        (x - 1.5).abs() < 1e-12 && (y - 2.5).abs() < 1e-12,
        "{x} {y}"
    );
}
