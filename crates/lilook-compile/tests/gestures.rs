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

/// Every value the inspector's `set` button would write has to **compile**.
///
/// Reparsing is not enough, and this test exists because of a case that proved
/// it: `set` on `xlim` wrote `()`, an empty array, which `check_expr` accepts and
/// lilaq refuses -- "Limit arrays must contain exactly two items". It reached a
/// browser. The lesson is the one `scripts/check.sh` was built on, applied to a
/// new surface: the round trip is not the gate, the compiler is.
#[test]
fn seeded_arguments_compile() {
    const SCHEMA: &str = lilook_core::schema::BUNDLED;
    let schema = lilook_core::Schema::from_json(SCHEMA).expect("bundled schema");
    let mut b = Backend::new(std::env::temp_dir(), "");

    // The two calls that carry most of lilaq's surface, and every named parameter
    // of theirs that starts out unset -- which is what `set` is for.
    let mut checked = 0;
    for (callee, extra) in [("lq.diagram", ""), ("lq.plot", "(0, 1, 2), (0, 1, 4),")] {
        let f = schema
            .function_for_callee(callee)
            .unwrap_or_else(|| panic!("{callee} in the schema"));
        for p in &f.params {
            if p.kind == "positional" || p.sentinels.is_empty() {
                continue;
            }
            use lilook_core::Editability::Literal;
            let control = lilook_ui::inspector::control_of(Some(p), Literal, &p.sentinels[0]);
            // Mirror the `Unset` arm exactly: it seeds the control the parameter
            // *would* be if it held a value, which is what makes the value the
            // right type. Getting this wrong here checked six parameters instead
            // of thirty and would have hidden the next `()`.
            let typed = match control {
                lilook_ui::Control::Unset => {
                    lilook_ui::refine(lilook_ui::control_for(Some(p)), Literal, "")
                }
                other => other,
            };
            // Only the values `set` would actually write.
            let Some(value) = lilook_ui::inspector::seed_for_test(Some(p), typed) else {
                continue;
            };
            // No width/height in the template: they are themselves parameters
            // under test, and passing one twice is a duplicate-argument error
            // rather than anything to do with the value.
            let src = format!(
                r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(
  {}
)
"#,
                if callee == "lq.diagram" {
                    format!("{}: {value},\n  lq.plot((0, 1, 2), (0, 1, 4))", p.name)
                } else {
                    format!("lq.plot({extra} {}: {value})", p.name)
                }
            );
            let r = b.render(&src, 1.0);
            if skip(&r) {
                return;
            }
            assert!(
                !r.failed(),
                "`set` on {callee}.{} writes `{value}`, which does not compile: {:?}",
                p.name,
                r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
            );
            checked += 1;
        }
    }
    eprintln!("{checked} seeded arguments compiled");
    // Twelve, at the time of writing: most sentinel parameters are arrays or
    // dictionaries, and `set` deliberately declines those. The floor is here so
    // that a change which quietly stops offering `set` at all fails.
    assert!(checked >= 10, "only {checked} seeds were checked");
}

/// Every argument the "add argument" popup offers has to **compile**.
///
/// The sibling of `seeded_arguments_compile`, for the other value-writing path:
/// `set` writes a seed, and adding an argument writes `ArgumentOffer::written`,
/// which is the seed where there is one and the documented default where there
/// is not. The two disagree exactly where it matters -- `seed` skips a default
/// that is `auto` or `none` on purpose, and for a `scale` that sentinel *is* the
/// value, so the offer must not fall through to the shape's placeholder. Both
/// panes write these: the popup at the caret and the inspector's field.
///
/// **What lilook chose, not what the user chose.** The values a row *names* --
/// `xscale`'s `log`, `symlog` -- are not checked here, because whether one suits
/// *this* data is not something any list can know and not something lilook
/// decided. `"log"` needs data above zero and `"symlog"` needs data spanning it,
/// so no single figure can hold both: measured here, on the fixtures this test
/// started with. A value the user picks is advisory; the one lilook writes on
/// their behalf when they add the argument itself is a promise, and this is the
/// promise.
#[test]
fn offered_arguments_compile() {
    const SCHEMA: &str = lilook_core::schema::BUNDLED;
    let schema = lilook_core::Schema::from_json(SCHEMA).expect("bundled schema");
    let mut b = Backend::new(std::env::temp_dir(), "");

    let mut checked = 0;
    for (callee, slots) in [("lq.diagram", ""), ("lq.plot", "(1, 2, 3), (1, 2, 4)")] {
        let f = schema
            .function_for_callee(callee)
            .unwrap_or_else(|| panic!("{callee} in the schema"));
        // A call with nothing set, so every named parameter is still on offer.
        let doc = Document::new(format!(
            "#import \"@preview/lilaq:0.6.0\" as lq\n#{callee}({slots})\n"
        ));
        let call = doc
            .calls()
            .iter()
            .find(|c| c.callee == callee)
            .unwrap_or_else(|| panic!("{callee} is a call site"));

        for o in lilook_core::argument_offers(&f.params, call) {
            let value = o.written();
            let src = format!(
                r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(
  {}
)
"#,
                match callee == "lq.diagram" {
                    true => format!("{}: {value},\n  lq.plot((1, 2, 3), (1, 2, 4))", o.param),
                    false => format!("lq.plot({slots}, {}: {value})", o.param),
                }
            );
            let r = b.render(&src, 1.0);
            if skip(&r) {
                return;
            }
            assert!(
                !r.failed(),
                "adding {callee}.{} writes `{value}`, which does not compile: {:?}",
                o.param,
                r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
            );
            checked += 1;
        }
    }
    eprintln!("{checked} offered arguments compiled");
    // One per named parameter of both calls, which is dozens: forty at the time
    // of writing. The floor is well under it so that a change which quietly
    // stops offering most of them fails here rather than passing vacuously --
    // `seeded_arguments_compile` reaches twelve, and this path reaches the ones
    // it declines.
    assert!(checked >= 30, "only {checked} offers were checked");
}

/// Panning a log-log figure as far as the pointer can go, and the result still
/// compiles.
///
/// The reported bug: lilaq refused the figure with "value must be strictly
/// positive". Two things were wrong. The recovered limits were *already* negative
/// before any drag -- a straight-line fit through two probes extrapolates below
/// them -- and the pan then subtracted a linear delta from a logarithmic axis.
/// Fixing the mapping fixes both, because a shift in log space is a ratio.
#[test]
fn panning_a_log_log_figure_stays_positive_and_compiles() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#let x = lq.linspace(1, 100, num: 20)
#lq.diagram(
  width: 8cm, height: 5cm,
  xscale: "log", yscale: "log",
  lq.plot(x, x.map(n => n * n), mark: none),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let mut doc = Document::new(src);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    // Both axes must have been recognised as logarithmic, or the rest proves
    // nothing about log panning.
    let t = scenes[0].transform;
    assert_eq!(t.x.kind, lilook_core::AxisScale::Log, "x should be log");
    assert_eq!(t.y.kind, lilook_core::AxisScale::Log, "y should be log");
    assert!(t.x.min > 0.0 && t.y.min > 0.0, "{:?}", t);

    let figure = doc.figures().first().map(|f| f.node).expect("a diagram");

    // Drags far larger than the figure, in every direction, including ones that
    // would have taken a linear pan straight through zero.
    for (dx, dy) in [
        (0.0, 0.0),
        (40.0, 0.0),
        (-40.0, 0.0),
        (0.0, 60.0),
        (0.0, -60.0),
        (500.0, 500.0),
        (-500.0, -500.0),
        (2000.0, -2000.0),
    ] {
        let (xlo, xhi) = t.x.shifted(dx);
        let (ylo, yhi) = t.y.shifted(dy);
        assert!(
            xlo > 0.0 && xhi > 0.0 && ylo > 0.0 && yhi > 0.0,
            "a drag of ({dx}, {dy}) left the log axes at ({xlo}, {xhi}) ({ylo}, {yhi})"
        );

        doc.begin("pan");
        for (param, (lo, hi)) in [("xlim", (xlo, xhi)), ("ylim", (ylo, yhi))] {
            // `gesture_num`, exactly as `Editor`'s `SetLimits` does. The local
            // `num` in this file is the *geometry* formatter, and using it here
            // reproduced the bug instead of testing the fix: it writes `3e-9` as
            // `0`, which is what lilaq was refusing.
            let value = format!(
                "({}, {})",
                lilook_core::gesture_num(lo),
                lilook_core::gesture_num(hi)
            );
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
            doc.apply(intent).expect("the limits are a valid value");
        }
        doc.commit();

        let (r, _) = b.render_scenes(&doc, 1.0, &mut hints);
        assert!(
            !r.failed(),
            "after a drag of ({dx}, {dy}) the figure stopped compiling: {:?}\n{}",
            r.errors().map(|d| d.message.clone()).collect::<Vec<_>>(),
            doc.text()
        );
    }

    // And every pan undoes, as one step each.
    while doc.undo() {}
    assert_eq!(doc.text(), src);
}

/// Dragging a threshold line moves *that* line, and nothing else.
///
/// `hlines(1.5, 2.5)` is two lines in one call, each its own positional argument
/// -- so moving one is `SetPositionalArg`, not the `SetArrayElement` a point drag
/// uses. Getting that wrong would rewrite an element of an array that is not
/// there.
#[test]
fn dragging_a_rule_rewrites_its_own_argument() {
    use lilook_core::{Axis, SeriesShape};

    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#lq.diagram(
  width: 6cm, height: 4cm,
  lq.plot((0, 1, 2, 3), (0, 2, 1, 3)),
  lq.hlines(1.5, 2.5, stroke: red),
  lq.vlines(1, stroke: blue),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let mut doc = Document::new(src);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    let hlines = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "hlines")
        .expect("the hlines");
    let vlines = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "vlines")
        .expect("the vlines");
    assert_eq!(hlines.series_shape(), SeriesShape::Rules(Axis::Y));
    assert_eq!(vlines.series_shape(), SeriesShape::Rules(Axis::X));
    // Each coordinate is its own slot, and each is a literal, so each is movable.
    assert_eq!(hlines.literal_rules(), vec![0, 1]);
    assert_eq!(vlines.literal_rules(), vec![0]);
    // But not as points: there is no pair, so nothing to drag as one.
    assert!(!hlines.has_literal_points());

    let (h_id, v_id) = (hlines.id, vlines.id);
    let scene = &scenes[0];
    let geom = |id: usize| scene.series.iter().find(|g| g.node == id).unwrap();
    assert_eq!(geom(h_id).rules(), vec![1.5, 2.5]);
    assert_eq!(geom(v_id).rules(), vec![1.0]);
    assert_eq!(geom(h_id).summary(), "2 horizontal lines");
    assert_eq!(geom(v_id).summary(), "1 vertical line");
    assert!(geom(h_id).points.is_empty(), "a rule has no points");

    // Grab the second horizontal line anywhere along its length -- the middle of
    // the frame horizontally, at its own height.
    let y_page = scene.transform.y.to_page(2.5);
    let mid_x = (scene.area.0 + scene.area.2) / 2.0;
    let hit = scene
        .hit_rule((mid_x, y_page), 4.0)
        .expect("a rule is grabbable along its whole length");
    assert_eq!(hit.node, h_id);
    assert_eq!(hit.index, 1, "the second argument, not the first");

    // And nowhere near it, nothing is grabbed.
    assert!(scene
        .hit_rule((mid_x, scene.transform.y.to_page(0.25)), 4.0)
        .is_none());

    // The edit: that slot, and only that slot.
    doc.begin("drag rule");
    doc.apply(Intent::SetPositionalArg {
        node: h_id,
        index: hit.index,
        value: lilook_core::gesture_num(2.75),
    })
    .expect("a rule coordinate is a valid value");
    doc.commit();

    assert!(
        doc.text().contains("lq.hlines(1.5, 2.75, stroke: red)"),
        "{}",
        doc.text()
    );
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());
    assert_eq!(
        scenes[0]
            .series
            .iter()
            .find(|g| g.node == h_id)
            .unwrap()
            .rules(),
        vec![1.5, 2.75],
        "the first line must not have moved"
    );

    assert_eq!(doc.history_depth().0, 1, "one drag, one undo step");
    doc.undo();
    assert_eq!(doc.text(), src);
}

/// Annotations are geometry lilook can move, and each kind stores its coordinates
/// somewhere different.
///
/// `place(x, y, ..)` keeps them as two arguments; `line(start, end)` keeps each
/// vertex as an `(x, y)` array. Both look like points on the page -- so hit-testing
/// needs no new case -- but the edit does not, and writing the wrong one would put
/// an array element where an argument belongs.
#[test]
fn annotations_recover_as_points_and_edit_by_shape() {
    use lilook_core::SeriesShape;

    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#lq.diagram(
  width: 7cm, height: 5cm,
  xlim: (0, 10), ylim: (0, 10),
  lq.plot((0, 5, 10), (1, 5, 9)),
  lq.place(2, 8, [note]),
  lq.line((1, 1), (9, 3)),
)
"#;
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut hints = Hints::new();
    let mut doc = Document::new(src);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    let find = |n: &str| {
        doc.calls()
            .iter()
            .find(|c| c.short_name() == n)
            .unwrap_or_else(|| panic!("the {n}"))
            .clone()
    };
    let (place, line) = (find("place"), find("line"));
    assert_eq!(place.series_shape(), SeriesShape::Anchor);
    assert_eq!(line.series_shape(), SeriesShape::Vertices);
    assert!(place.has_literal_anchor());
    assert_eq!(line.literal_vertices(), vec![0, 1]);

    // Both come back as ordinary points, so the canvas picks them with `hit`.
    let scene = &scenes[0];
    let geom = |id: usize| scene.series.iter().find(|g| g.node == id).unwrap();
    assert_eq!(geom(place.id).points, vec![(2.0, 8.0)]);
    assert_eq!(geom(line.id).points, vec![(1.0, 1.0), (9.0, 3.0)]);

    let at = scene.transform.to_page((9.0, 3.0));
    let hit = scene.hit(at, 4.0).expect("the line's second vertex");
    assert_eq!(hit.node, line.id);
    assert_eq!(hit.index, 1);

    // Moving the annotation rewrites its two *arguments*.
    doc.begin("move annotation");
    for (index, v) in [(0usize, 3.5), (1, 6.25)] {
        doc.apply(Intent::SetPositionalArg {
            node: place.id,
            index,
            value: lilook_core::gesture_num(v),
        })
        .expect("a coordinate is a valid value");
    }
    doc.commit();
    assert!(
        doc.text().contains("lq.place(3.5, 6.25, [note])"),
        "{}",
        doc.text()
    );

    // Moving a vertex rewrites two *elements inside one slot*.
    doc.begin("move vertex");
    for (element, v) in [(0usize, 8.0), (1, 4.5)] {
        doc.apply(Intent::SetArrayElement {
            node: line.id,
            arg: 1,
            element,
            value: lilook_core::gesture_num(v),
        })
        .expect("a vertex coordinate is a valid value");
    }
    doc.commit();
    assert!(
        doc.text().contains("lq.line((1, 1), (8, 4.5))"),
        "{}",
        doc.text()
    );

    // Both still compile, and both undo as one step each.
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());
    let moved = scenes[0].series.iter().find(|g| g.node == line.id).unwrap();
    assert_eq!(moved.points, vec![(1.0, 1.0), (8.0, 4.5)]);

    assert_eq!(doc.history_depth().0, 2);
    doc.undo();
    doc.undo();
    assert_eq!(doc.text(), src);
}

/// A completion accepted in the source pane leaves a document that compiles.
///
/// The other half of `offered_arguments_compile`: that one checks the *value*,
/// this one checks where it lands. A completion is a text edit over the word
/// being typed, so the argument list it drops into is the list's problem --
/// `width: 6cm, xscale: "log"` with a series on the next line and no comma
/// between them is a figure typst refuses, and the report is "expected comma" at
/// a byte offset. `fit_argument` is what supplies it; this is the proof.
#[test]
fn an_accepted_completion_leaves_a_list_that_compiles() {
    const SCHEMA: &str = lilook_core::schema::BUNDLED;
    let schema = lilook_core::Schema::from_json(SCHEMA).expect("bundled schema");
    let mut b = Backend::new(std::env::temp_dir(), "");

    // A caret between an argument and the series that follows it: the position
    // that needs a comma *after* whatever is accepted.
    let base = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(
  width: 6cm,
  lq.plot((1, 2, 3), (1, 2, 4)),
)
"#;
    let caret = base.find("\n  lq.plot").expect("between arguments") + 1;
    let session = lilook_core::Session::new(base, schema);
    let offers = session.completions(caret);
    assert!(offers.len() > 5, "the caret takes arguments: {offers:?}");

    let mut checked = 0;
    for c in offers.iter().take(14) {
        // Exactly what the pane does with a click: fit the offer to the list,
        // then replace.
        let (range, value) = lilook_core::fit_argument(base, caret..caret, &c.insert);
        if value.trim_end().ends_with(':') {
            continue; // a name whose value the user has still to type
        }
        let mut src = base.to_string();
        src.replace_range(range, &value);
        let r = b.render(&src, 1.0);
        if skip(&r) {
            return;
        }
        assert!(
            !r.failed(),
            "accepting `{}` leaves a document that will not compile: {:?}\n{src}",
            c.label,
            r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
        );
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} completions were accepted");

    // And the comma that was missing *before* the word being typed: mid-edit
    // source that does not compile until the offer is fitted into it.
    let mid = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(
  width: 6cm
  xsc
  lq.plot((1, 2, 3), (1, 2, 4)),
)
"#;
    let word = mid.find("xsc").expect("the word being typed");
    let (range, value) = lilook_core::fit_argument(mid, word..word + 3, "xscale: \"log\"");
    let mut src = mid.to_string();
    src.replace_range(range, &value);
    let r = b.render(&src, 1.0);
    assert!(
        !r.failed(),
        "a missing comma before the caret survived: {:?}\n{src}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
}

/// A palette out of the user's own library draws a figure.
///
/// The library stores typst, so the same rule applies to it as to everything
/// else lilook writes: it has to compile, not merely reparse. `Prefs::save`
/// refuses what will not parse; this is the other half, and it is the half that
/// caught `cycle: "petroff10"` -- a string where lilaq wanted an array, which
/// parses perfectly.
///
/// And the property that makes a library safe at all: the *document* carries the
/// colours. Rendering never reads the library, so the figure below is drawn from
/// its own source and would look the same to someone who has never seen it.
#[test]
fn a_saved_palette_draws() {
    use lilook_core::{Kind, Prefs};

    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut prefs = Prefs::default();
    // Built the way the editor builds one: colours as source text, joined.
    let parts = ["rgb(\"#4477aa\")".to_string(), "luma(30%)".to_string()];
    let value = lilook_ui::pick::cycle_array(&parts);
    prefs
        .save(Kind::Cycle, "mine", &value)
        .expect("a palette lilook would store");

    let saved = prefs.get(Kind::Cycle, "mine").expect("just saved");
    let src = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(
  cycle: {},
  lq.plot((1, 2, 3), (1, 2, 4)),
  lq.plot((1, 2, 3), (2, 3, 5)),
)
"#,
        saved.value
    );
    let r = b.render(&src, 1.0);
    if skip(&r) {
        return;
    }
    assert!(
        !r.failed(),
        "a palette from the library does not draw: {:?}\n{src}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    // A one-colour palette is an array of one, which in typst keeps its comma.
    let one = lilook_ui::pick::cycle_array(&parts[..1]);
    assert_eq!(one, "(rgb(\"#4477aa\"),)");
    let src = src.replace(&saved.value, &one);
    assert!(
        !b.render(&src, 1.0).failed(),
        "a palette of one colour does not draw:\n{src}"
    );
}

/// A colour map and a theme of the user's own, kept and used.
///
/// The same promise `a_saved_palette_draws` makes, for the two kinds that were
/// added after it -- and for the theme it is the more interesting one, because a
/// theme is a *function* rather than a value. The library stores the closure and
/// the document gets its own `#let`, so the figure below carries the whole theme
/// and would look the same to someone who has never seen the library.
#[test]
fn a_saved_map_and_a_saved_theme_draw() {
    use lilook_core::{Kind, Prefs, Schema, Session};

    let mut b = Backend::new(std::env::temp_dir(), "");
    let schema = Schema::from_json(lilook_core::schema::BUNDLED).expect("bundled schema");

    // A map built the way the editor builds one: stops as source text.
    let stops = [
        "rgb(\"#000004\")".to_string(),
        "rgb(\"#8c2981\")".to_string(),
        "rgb(\"#fcfdbf\")".to_string(),
    ];
    let map = lilook_ui::pick::cycle_array(&stops);
    let mut prefs = Prefs::default();
    prefs.save(Kind::Colormap, "mine", &map).expect("a map");

    let src = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let xs = lq.linspace(0, 4, num: 5)
#lq.diagram(lq.colormesh(xs, xs, (x, y) => x * y, map: {map}))
"#
    );
    let r = b.render(&src, 1.0);
    if skip(&r) {
        return;
    }
    assert!(
        !r.failed(),
        "a colour map from the library does not draw: {:?}\n{src}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    // A theme: forked in one document, kept, then used in another.
    let first = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(lq.plot((1, 2, 3), (1, 2, 4)))
"#;
    let mut s = Session::new(first, schema.clone());
    assert!(s.fork_theme("my-theme"), "a theme of my own");
    assert!(s.save_theme(), "kept in the library");
    let saved = s.prefs.get(Kind::Theme, "my-theme").cloned().expect("kept");
    assert!(
        !saved.value.contains("#let"),
        "the closure, not the binding: {}",
        saved.value
    );
    assert!(
        !b.render(s.doc.text(), 1.0).failed(),
        "the document it came from stopped compiling:\n{}",
        s.doc.text()
    );

    // Another document, another session, the same library.
    let mut other = Session::new(first, schema);
    other.prefs = s.prefs.clone();
    assert!(other.use_saved_theme("my-theme"), "brought in");
    let text = other.doc.text().to_string();
    assert!(text.contains("#let my-theme ="), "written in full:\n{text}");
    assert!(text.contains("#show: my-theme"), "and switched to:\n{text}");
    let r = b.render(&text, 1.0);
    assert!(
        !r.failed(),
        "a theme from the library does not draw: {:?}\n{text}",
        r.errors().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    // And it is one undo step, like every other edit lilook makes.
    other.doc.undo();
    assert_eq!(other.doc.text(), first, "one step, fully undone");
}
