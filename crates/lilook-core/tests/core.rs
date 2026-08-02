use lilook_core::{AppliedEdit, CoalesceKey, Document, Editability, History, Intent, Schema};

const DOC: &str = r##"#import "@preview/lilaq:0.6.0" as lq

// a hand-written comment that must survive every edit
#let accent = rgb("#4c72b0")
#let xs = lq.linspace(0, 10)

#figure(
  lq.diagram(
    width: 8cm,   // trailing comment, odd    spacing
    height: 5cm,
    lq.plot(xs, xs.map(x => calc.sin(x)), stroke: red, mark: "o"),
    lq.plot(xs, xs.map(x => calc.cos(x)), stroke: accent),
    ..range(3).map(i => lq.plot(xs, xs.map(x => x + i))),
  ),
  caption: [A figure.],
)
"##;

/// Deterministic xorshift, so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn indexes_lilaq_call_sites() {
    let doc = Document::new(DOC);
    let names: Vec<_> = doc.calls().iter().map(|c| c.callee.as_str()).collect();
    assert!(names.contains(&"lq.diagram"));
    assert_eq!(names.iter().filter(|n| **n == "lq.plot").count(), 3);
}

#[test]
fn distinguishes_builtin_from_user_binding() {
    let doc = Document::new(DOC);
    let mut seen = vec![];
    for c in doc.calls() {
        for a in &c.named {
            if a.name == "stroke" {
                seen.push((a.text.clone(), a.editability));
            }
        }
    }
    // `red` is a Typst builtin; `accent` is a user #let. Both are Idents.
    assert!(seen.contains(&("red".to_string(), Editability::Builtin)));
    assert!(seen.contains(&("accent".to_string(), Editability::Binding)));
}

#[test]
fn marks_loop_generated_calls() {
    let doc = Document::new(DOC);
    let generated = doc.calls().iter().filter(|c| c.generated).count();
    assert_eq!(
        generated, 1,
        "the spread/map-generated plot should be flagged"
    );
}

#[test]
fn edit_preserves_everything_else() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .unwrap()
        .id;
    doc.apply(Intent::SetNamedArg {
        node: id,
        param: "stroke".into(),
        value: "blue.darken(20%)".into(),
    })
    .unwrap();

    let t = doc.text();
    assert!(t.contains("blue.darken(20%)"));
    assert!(t.contains("// a hand-written comment that must survive every edit"));
    assert!(t.contains("odd    spacing"));
    assert!(t.contains(r#"caption: [A figure.]"#));
}

#[test]
fn insert_named_arg() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;
    doc.apply(Intent::InsertNamedArg {
        node: id,
        param: "xlabel".into(),
        value: "[Time]".into(),
    })
    .unwrap();
    assert!(doc.text().contains("xlabel: [Time]"));
    doc.undo();
    assert_eq!(doc.text(), DOC);
}

#[test]
fn drag_coalesces_into_one_undo_step() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;

    doc.begin("drag width");
    for w in ["8.1cm", "8.4cm", "9.0cm", "9.6cm", "10cm"] {
        doc.apply(Intent::SetNamedArg {
            node: id,
            param: "width".into(),
            value: w.into(),
        })
        .unwrap();
    }
    doc.commit();

    assert!(doc.text().contains("width: 10cm"));
    assert_eq!(doc.history_depth().0, 1, "five drag events, one undo entry");
    assert!(doc.undo());
    assert_eq!(doc.text(), DOC, "one undo returns to the start");
}

/// A pan drives `xlim` and `ylim` together. Coalescing against "the last edit"
/// collapses nothing when two targets interleave, so this asserts the edit
/// *count*, not just the undo depth: the bug it guards is a transaction that
/// grows by two edits per frame and is only visible as memory and slow undo.
#[test]
fn a_two_parameter_drag_stays_two_edits() {
    let mut h = History::default();
    h.begin("pan");
    // `height` sits after `width` in the buffer, and the values change length,
    // so the later slot has to be shifted as the earlier one grows.
    let (mut w, mut hgt) = ("8cm".to_string(), "5cm".to_string());
    let (w_at, h_at) = (DOC.find("8cm").unwrap(), DOC.find("5cm").unwrap());
    let mut delta = 0isize;
    for i in 0..40 {
        let nw = format!("{}.{}cm", 8 + i / 10, i % 10);
        h.record(
            AppliedEdit {
                range: w_at..w_at + w.len(),
                before: w.clone(),
                after: nw.clone(),
            },
            Some(CoalesceKey {
                node: 0,
                param: "width".into(),
            }),
        );
        delta += nw.len() as isize - w.len() as isize;
        w = nw;

        let nh = format!("{}.{}cm", 5 + i / 10, i % 10);
        let at = (h_at as isize + delta) as usize;
        h.record(
            AppliedEdit {
                range: at..at + hgt.len(),
                before: hgt.clone(),
                after: nh.clone(),
            },
            Some(CoalesceKey {
                node: 0,
                param: "height".into(),
            }),
        );
        hgt = nh;
    }
    h.commit();

    let tx = h.take_undo().expect("one transaction");
    assert_eq!(tx.edits.len(), 2, "eighty intents, two targets, two edits");
    assert_eq!(tx.edits[0].before, "8cm");
    assert_eq!(tx.edits[0].after, "11.9cm");
    assert_eq!(tx.edits[1].before, "5cm");
    assert_eq!(tx.edits[1].after, "8.9cm");
}

/// The same gesture through the document, where the chain has to survive being
/// replayed backwards.
#[test]
fn interleaved_two_parameter_drag_undoes_exactly() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;

    doc.begin("pan");
    for i in 0..30 {
        // Height first, so the parameter that moves in the buffer is the one
        // edited *earlier* in the transaction -- the harder direction.
        for (param, base) in [("height", 5), ("width", 8)] {
            doc.apply(Intent::SetNamedArg {
                node: id,
                param: param.into(),
                value: format!("{}.{}cm", base + i / 10, i % 10),
            })
            .unwrap();
        }
    }
    doc.commit();

    assert!(doc.text().contains("width: 10.9cm"));
    assert!(doc.text().contains("height: 7.9cm"));
    assert_eq!(doc.history_depth().0, 1);
    assert!(doc.undo());
    assert_eq!(doc.text(), DOC);
}

/// An intent with no coalesce key is a boundary: slots opened before it have to
/// be materialised, or the chain comes out in the wrong order.
#[test]
fn an_unkeyed_intent_splits_the_slot_group() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;

    doc.begin("gesture");
    doc.apply(Intent::SetNamedArg {
        node: id,
        param: "width".into(),
        value: "12cm".into(),
    })
    .unwrap();
    doc.apply(Intent::InsertNamedArg {
        node: id,
        param: "xlabel".into(),
        value: "[t]".into(),
    })
    .unwrap();
    doc.apply(Intent::SetNamedArg {
        node: id,
        param: "width".into(),
        value: "13cm".into(),
    })
    .unwrap();
    doc.commit();

    assert!(doc.text().contains("width: 13cm"));
    assert!(doc.text().contains("xlabel: [t]"));
    assert_eq!(doc.history_depth().0, 1);
    assert!(doc.undo());
    assert_eq!(doc.text(), DOC);
}

/// A drag that ends where it started should leave no trace at all.
#[test]
fn a_drag_returning_to_its_origin_records_nothing() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;

    doc.begin("drag");
    for w in ["9cm", "11cm", "8cm"] {
        doc.apply(Intent::SetNamedArg {
            node: id,
            param: "width".into(),
            value: w.into(),
        })
        .unwrap();
    }
    doc.commit();

    assert_eq!(doc.text(), DOC);
    assert_eq!(doc.history_depth().0, 0, "nothing changed, nothing to undo");
}

#[test]
fn anchors_survive_edits_and_undo() {
    let mut doc = Document::new(DOC);
    let caption_at = DOC.find("caption").unwrap();
    let a = doc.anchor(caption_at);

    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;
    doc.apply(Intent::SetNamedArg {
        node: id,
        param: "width".into(),
        value: "16cm".into(),
    })
    .unwrap();

    let moved = doc.anchor_offset(a).unwrap();
    assert_eq!(&doc.text()[moved..moved + 7], "caption");

    doc.undo();
    assert_eq!(doc.anchor_offset(a).unwrap(), caption_at);
}

/// The invariant that matters: any sequence of intents, fully undone, must
/// return the buffer byte-for-byte.
///
/// The generator opens transactions of random length as well as applying loose
/// intents, because coalescing rewrites edits that are already recorded -- the
/// chain it produces is a different thing to verify than a chain of untouched
/// edits, and only the transactional path exercises it.
#[test]
fn random_intents_fully_undo() {
    let values = ["1cm", "2.5cm", "3cm", "42pt", "10%"];
    let colors = ["red", "blue", "green", "accent", "rgb(\"#123456\")"];
    let inserts = [
        ("xlabel", "[t]"),
        ("ylabel", "[y]"),
        ("xlim", "(0, 10)"),
        ("ylim", "(-1, 1)"),
        ("smooth", "true"),
    ];
    let numbers = ["0", "1.5", "-2", "3.25"];

    // A generator that quietly stops producing an intent still passes, so count
    // what actually landed and assert every kind was exercised.
    let mut coverage: std::collections::BTreeMap<&str, usize> = Default::default();

    // Both fixtures: DOC has computed data and a generated call, POINTS has the
    // literal arrays that make `SetArrayElement` reachable.
    for source in [DOC, POINTS] {
        for seed in 1..40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15));
            let mut doc = Document::new(source);
            let mut applied = 0usize;
            let mut open = 0usize;

            for _ in 0..12 {
                // A third of steps are a gesture: several intents inside one
                // transaction, which is where coalescing happens.
                if open == 0 && rng.pick(3) == 0 {
                    doc.begin("gesture");
                    open = 1 + rng.pick(6);
                }

                let calls: Vec<(usize, Vec<String>, usize, Vec<usize>)> = doc
                    .calls()
                    .iter()
                    .map(|c| {
                        (
                            c.id,
                            c.named.iter().map(|a| a.name.clone()).collect(),
                            c.positional.len(),
                            c.positional.iter().map(|p| p.elements.len()).collect(),
                        )
                    })
                    .collect();
                if calls.is_empty() {
                    break;
                }
                let (id, params, npos, nelem) = &calls[rng.pick(calls.len())];

                let intent = match rng.pick(12) {
                    0 => {
                        let (param, value) = inserts[rng.pick(inserts.len())];
                        Intent::InsertNamedArg {
                            node: *id,
                            param: param.into(),
                            value: value.into(),
                        }
                    }
                    1 if !params.is_empty() => Intent::RemoveNamedArg {
                        node: *id,
                        param: params[rng.pick(params.len())].clone(),
                    },
                    2 => Intent::RemoveNode { node: *id },
                    7 => Intent::InsertPositionalArg {
                        node: *id,
                        value: "(9, 9)".into(),
                    },
                    3 if *npos > 0 => Intent::SetPositionalArg {
                        node: *id,
                        index: rng.pick(*npos),
                        value: "(0, 1)".into(),
                    },
                    4..=6 if nelem.iter().any(|n| *n > 0) => {
                        let arg = rng.pick(npos.max(&1).to_owned());
                        let n = nelem.get(arg).copied().unwrap_or(0);
                        Intent::SetArrayElement {
                            node: *id,
                            arg,
                            element: if n == 0 { 0 } else { rng.pick(n) },
                            value: numbers[rng.pick(numbers.len())].into(),
                        }
                    }
                    _ if !params.is_empty() => {
                        let param = params[rng.pick(params.len())].clone();
                        let value = if param == "stroke" || param == "color" {
                            colors[rng.pick(colors.len())]
                        } else {
                            values[rng.pick(values.len())]
                        };
                        Intent::SetNamedArg {
                            node: *id,
                            param,
                            value: value.into(),
                        }
                    }
                    _ => continue,
                };

                let kind = match &intent {
                    Intent::InsertPositionalArg { .. } => "insert-positional",
                    Intent::SetNamedArg { .. } => "set",
                    Intent::InsertNamedArg { .. } => "insert",
                    Intent::RemoveNamedArg { .. } => "remove-arg",
                    Intent::SetPositionalArg { .. } => "set-positional",
                    Intent::SetArrayElement { .. } => "set-element",
                    Intent::RemoveNode { .. } => "remove-node",
                    Intent::ReplaceRange { .. } => "replace-range",
                };
                if doc.apply(intent).is_ok() {
                    applied += 1;
                    *coverage.entry(kind).or_default() += 1;
                }

                if open > 0 {
                    open -= 1;
                    if open == 0 {
                        doc.commit();
                    }
                }
            }
            doc.commit();

            assert!(applied > 0, "seed {seed} applied nothing");
            while doc.undo() {}
            assert_eq!(
                doc.text(),
                source,
                "seed {seed}: {applied} intents did not fully undo"
            );
        }
    }

    for kind in [
        "set",
        "insert",
        "remove-arg",
        "set-positional",
        "set-element",
        "remove-node",
        "insert-positional",
    ] {
        assert!(
            coverage.get(kind).copied().unwrap_or(0) > 0,
            "the generator never applied `{kind}`: {coverage:?}"
        );
    }
}

#[test]
fn redo_replays() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;
    doc.apply(Intent::SetNamedArg {
        node: id,
        param: "height".into(),
        value: "7cm".into(),
    })
    .unwrap();
    let after = doc.text().to_string();
    assert!(doc.undo());
    assert_eq!(doc.text(), DOC);
    assert!(doc.redo());
    assert_eq!(doc.text(), after);
}

#[test]
fn schema_covers_the_call_sites_we_index() {
    let raw = lilook_core::schema::BUNDLED;
    let schema = Schema::from_json(raw).expect("schema parses");
    assert_eq!(schema.lilaq_version, "0.6.0");

    let doc = Document::new(DOC);
    for call in doc.calls() {
        let f = schema
            .function_for_callee(&call.callee)
            .unwrap_or_else(|| panic!("no schema entry for {}", call.callee));
        for arg in &call.named {
            assert!(
                f.params.iter().any(|p| p.name == arg.name),
                "{} has no schema param `{}`",
                call.callee,
                arg.name
            );
        }
    }
}

// ------------------------------------------------- M5: the editing gestures

const POINTS: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#lq.diagram(
  width: 8cm,
  lq.plot((0, 1, 2), (0, 1, 4), stroke: red),
  lq.plot((0, 1, 2), (2, 3, 1)),
)
"#;

#[test]
fn dragging_a_point_edits_one_array_element() {
    let mut doc = Document::new(POINTS);
    let plot = doc.calls().iter().find(|c| c.callee == "lq.plot").unwrap();
    let id = plot.id;
    assert!(plot.has_literal_points(), "literal arrays are draggable");
    assert_eq!(plot.positional[0].elements.len(), 3);

    // One frame of a drag: x and y of the same point, two targets.
    doc.begin("drag point");
    for (x, y) in [(0.4, 1.4), (0.8, 1.9), (1.25, 2.5)] {
        doc.apply(Intent::SetArrayElement {
            node: id,
            arg: 0,
            element: 1,
            value: x.to_string(),
        })
        .unwrap();
        doc.apply(Intent::SetArrayElement {
            node: id,
            arg: 1,
            element: 1,
            value: y.to_string(),
        })
        .unwrap();
    }
    doc.commit();

    assert!(
        doc.text().contains("(0, 1.25, 2), (0, 2.5, 4)"),
        "{}",
        doc.text()
    );
    assert_eq!(doc.history_depth().0, 1, "a drag is one undo step");
    assert!(doc.undo());
    assert_eq!(doc.text(), POINTS);
}

#[test]
fn computed_data_is_not_draggable() {
    let doc = Document::new(
        "#import \"@preview/lilaq:0.6.0\" as lq\n#lq.diagram(lq.plot(xs, xs.map(f)))\n",
    );
    let plot = doc.calls().iter().find(|c| c.callee == "lq.plot").unwrap();
    assert!(!plot.has_literal_points());
    assert!(plot.positional[0].elements.is_empty());

    let mut doc = Document::new(doc.text());
    let err = doc
        .apply(Intent::SetArrayElement {
            node: plot.id,
            arg: 0,
            element: 0,
            value: "1".into(),
        })
        .unwrap_err();
    assert!(err.contains("not a literal array"), "{err}");
}

#[test]
fn removing_a_series_takes_its_separating_comma() {
    let mut doc = Document::new(POINTS);
    let second = doc
        .calls()
        .iter()
        .filter(|c| c.callee == "lq.plot")
        .nth(1)
        .unwrap()
        .id;
    doc.apply(Intent::RemoveNode { node: second }).unwrap();

    let t = doc.text();
    assert!(!t.contains("(2, 3, 1)"));
    assert!(!t.contains(",\n  ,"), "a stray comma was left behind:\n{t}");
    assert!(!t.contains(",,"), "a stray comma was left behind:\n{t}");
    // Still one call inside the diagram, and the document still parses to it.
    assert_eq!(
        doc.calls().iter().filter(|c| c.callee == "lq.plot").count(),
        1
    );
    doc.undo();
    assert_eq!(doc.text(), POINTS);
}

#[test]
fn removing_a_named_argument_takes_its_comma() {
    let mut doc = Document::new(POINTS);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .unwrap()
        .id;
    doc.apply(Intent::RemoveNamedArg {
        node: id,
        param: "stroke".into(),
    })
    .unwrap();
    assert!(!doc.text().contains("stroke"));
    assert!(!doc.text().contains(",,"), "{}", doc.text());
    assert!(
        doc.text().contains("lq.plot((0, 1, 2), (0, 1, 4))"),
        "{}",
        doc.text()
    );
    doc.undo();
    assert_eq!(doc.text(), POINTS);
}

/// Insertion used to always put the new argument on the last argument's line.
/// Valid, and ugly enough that it was worth fixing once the GUI started adding
/// `xlim` and `ylim` on every pan.
#[test]
fn insertion_follows_the_surrounding_layout() {
    let mut doc = Document::new(POINTS);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;
    doc.apply(Intent::InsertNamedArg {
        node: id,
        param: "xlim".into(),
        value: "(0, 2)".into(),
    })
    .unwrap();
    assert!(
        doc.text().contains("\n  xlim: (0, 2)"),
        "one argument per line should stay that way:\n{}",
        doc.text()
    );

    // A single-line call keeps its single line.
    let mut inline = Document::new("#import \"x\" as lq\n#lq.plot((0,), (1,))\n");
    let id = inline.calls()[0].id;
    inline
        .apply(Intent::InsertNamedArg {
            node: id,
            param: "mark".into(),
            value: "none".into(),
        })
        .unwrap();
    assert!(
        inline.text().contains("(1,), mark: none)"),
        "{}",
        inline.text()
    );
}

#[test]
fn a_pan_writes_both_limits_as_one_undo_step() {
    let mut doc = Document::new(POINTS);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.diagram")
        .unwrap()
        .id;

    // The first frame inserts the arguments, later frames set them -- which is
    // exactly the mixed keyed/unkeyed case the slot group has to survive.
    doc.begin("pan");
    doc.apply(Intent::InsertNamedArg {
        node: id,
        param: "xlim".into(),
        value: "(0, 2)".into(),
    })
    .unwrap();
    doc.apply(Intent::InsertNamedArg {
        node: id,
        param: "ylim".into(),
        value: "(0, 4)".into(),
    })
    .unwrap();
    for i in 1..20 {
        let d = i as f64 * 0.1;
        doc.apply(Intent::SetNamedArg {
            node: id,
            param: "xlim".into(),
            value: format!("({:.2}, {:.2})", d, 2.0 + d),
        })
        .unwrap();
        doc.apply(Intent::SetNamedArg {
            node: id,
            param: "ylim".into(),
            value: format!("({:.2}, {:.2})", d, 4.0 + d),
        })
        .unwrap();
    }
    doc.commit();

    assert!(doc.text().contains("xlim: (1.90, 3.90)"), "{}", doc.text());
    assert!(doc.text().contains("ylim: (1.90, 5.90)"), "{}", doc.text());
    assert_eq!(doc.history_depth().0, 1, "the whole pan is one undo step");
    assert!(doc.undo());
    assert_eq!(doc.text(), POINTS);
}

#[test]
fn values_that_would_not_reparse_are_refused() {
    // Everything a widget or an agent might legitimately produce.
    for ok in [
        "red",
        "2.5cm",
        "10%",
        "true",
        "none",
        "auto",
        "\"o\"",
        "(0, 10)",
        "(1,)",
        "rgb(\"#4c72b0\")",
        "blue.darken(20%)",
        "1pt + red",
        "[Time]",
        "(paint: red, dash: \"dashed\")",
        "calc.pi * 2",
        "xs.map(x => x + 1)",
        "-3",
        "1e-4",
        "left + top",
        "(:)",
    ] {
        assert!(
            lilook_core::check_expr(ok).is_ok(),
            "{ok} should be accepted"
        );
    }
    // `let x = 1` is *not* here: typst accepts it as an argument value. The
    // check is syntactic on purpose -- lilaq's type rules stay lilaq's.
    for bad in ["", "  ", "rgb(\"#4c72b0\"", "(0, 10", "1 2", "(a: )", ")"] {
        assert!(
            lilook_core::check_expr(bad).is_err(),
            "{bad:?} should be refused"
        );
    }
}

/// The point of validating in the core rather than in each frontend: an intent
/// that would break the buffer must fail before it is applied, leaving the
/// document exactly as it was.
#[test]
fn a_bad_value_leaves_the_document_untouched() {
    let mut doc = Document::new(DOC);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .unwrap()
        .id;
    let err = doc
        .apply(Intent::SetNamedArg {
            node: id,
            param: "stroke".into(),
            value: "rgb(\"#abc\"".into(),
        })
        .unwrap_err();
    assert!(err.contains("not a valid argument value"), "{err}");
    assert_eq!(doc.text(), DOC);
    assert_eq!(doc.history_depth().0, 0, "a refused intent records nothing");
}

/// `rgb("#4c72b0")` is a colour a swatch can edit; `calc.sin(x)` is a program.
/// Both are `FuncCall`, so the builtin table has to tell them apart here too.
#[test]
fn builtin_constructors_are_editable_but_arbitrary_calls_are_not() {
    let doc = Document::new(
        "#import \"@preview/lilaq:0.6.0\" as lq\n\
         #lq.diagram(lq.plot((0,), (1,), color: rgb(\"#4c72b0\"), stroke: luma(20%)),\n\
         lq.plot((0,), (1,), color: calc.max(1, 2), stroke: lq.mix(red, blue)))\n",
    );
    let by = |nth: usize, name: &str| {
        doc.calls()
            .iter()
            .filter(|c| c.callee == "lq.plot")
            .nth(nth)
            .unwrap()
            .named
            .iter()
            .find(|a| a.name == name)
            .unwrap()
            .editability
    };
    assert_eq!(by(0, "color"), Editability::Builtin);
    assert_eq!(by(0, "stroke"), Editability::Builtin);
    assert_eq!(by(1, "color"), Editability::Opaque);
    assert_eq!(by(1, "stroke"), Editability::Opaque);
}

// ------------------------------------------------------------ M7: set rules

const STYLED: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#show: lq.set-tick(stroke: red)

#lq.diagram(lq.plot((0, 1), (0, 1)))

#[
  #show: lq.set-legend(fill: white)
  #lq.diagram(lq.plot((0, 1), (1, 0)))
]

#lq.diagram(lq.plot((0, 1), (0, 2)))
"#;

#[test]
fn indexes_set_rules_with_the_scope_they_govern() {
    let doc = Document::new(STYLED);
    let rules = doc.set_rules();
    assert_eq!(rules.len(), 2, "{rules:?}");

    let tick = &rules[0];
    assert_eq!(tick.element, "tick");
    assert!(
        tick.document_level,
        "a top-level rule governs the rest of the file"
    );
    assert_eq!(tick.scope.end, STYLED.len());

    let legend = &rules[1];
    assert_eq!(legend.element, "legend");
    assert!(
        !legend.document_level,
        "a rule inside a block stops at the end of that block"
    );
    assert!(
        legend.scope.end < STYLED.len(),
        "scope {:?} should end before the file does",
        legend.scope
    );
    // The third diagram is outside the legend rule's scope, and inside the
    // tick rule's -- which is exactly the distinction the panel has to show.
    let last = STYLED.rfind("lq.diagram").unwrap();
    assert!(!legend.scope.contains(&last));
    assert!(tick.scope.contains(&last));
}

/// A set rule is an ordinary indexed call, so it is edited by the ordinary
/// intents -- no second edit path, no second undo story.
#[test]
fn a_set_rule_is_edited_like_any_other_call() {
    let mut doc = Document::new(STYLED);
    let rule = doc.set_rules()[0].clone();
    doc.apply(Intent::SetNamedArg {
        node: rule.node,
        param: "stroke".into(),
        value: "blue".into(),
    })
    .unwrap();
    assert!(doc.text().contains("lq.set-tick(stroke: blue)"));
    doc.undo();
    assert_eq!(doc.text(), STYLED);
}

#[test]
fn element_fields_render_as_a_function_for_the_inspector() {
    let raw = lilook_core::schema::BUNDLED;
    let schema = Schema::from_json(raw).unwrap();
    assert!(schema.elements.len() >= 15, "{}", schema.elements.len());

    let tick = schema
        .element_as_function("lq.set-tick")
        .expect("tick element");
    assert!(tick.params.iter().any(|p| p.name == "stroke"));
    // Element fields carry no `kind`; they default to named, which is what the
    // inspector needs to lay them out.
    assert!(tick.params.iter().all(|p| p.kind == "named"));
    assert!(schema.element_for_callee("lq.plot").is_none());
}

// ------------------------------------------------------- M8: copy and paste

const CAPTURED: &str = r##"#import "@preview/lilaq:0.6.0" as lq
#let xs = lq.linspace(0, 10)
#let accent = rgb("#4c72b0")
#let f(t) = calc.sin(t) * 2
#lq.diagram(
  lq.plot(xs, xs.map(t => f(t) + 1), stroke: accent, mark: "o"),
  lq.plot((0, 1), (0, 1)),
)
"##;

#[test]
fn free_identifiers_are_what_a_paste_would_have_to_carry() {
    let doc = Document::new(CAPTURED);
    let plot = doc.calls().iter().find(|c| c.callee == "lq.plot").unwrap();
    let free = doc.free_identifiers(plot.range.clone());

    // `xs`, `accent` and `f` come from outside; `lq` does too.
    assert!(free.contains(&"xs".to_string()), "{free:?}");
    assert!(free.contains(&"accent".to_string()), "{free:?}");
    assert!(free.contains(&"f".to_string()), "{free:?}");
    assert!(free.contains(&"lq".to_string()), "{free:?}");
    // `t` is bound by the closure, `plot`/`map` are field names, `calc` and
    // `mark`/`stroke` are not names in scope at all.
    for not_free in ["t", "plot", "map", "calc", "stroke", "mark", "o"] {
        assert!(
            !free.contains(&not_free.to_string()),
            "`{not_free}` should not be free: {free:?}"
        );
    }

    // A series with only literals needs nothing but the module.
    let literal = doc
        .calls()
        .iter()
        .filter(|c| c.callee == "lq.plot")
        .nth(1)
        .unwrap();
    assert_eq!(doc.free_identifiers(literal.range.clone()), vec!["lq"]);
}

#[test]
fn a_local_let_inside_the_fragment_is_not_free() {
    let doc = Document::new(
        "#import \"@preview/lilaq:0.6.0\" as lq\n\
         #lq.diagram(lq.plot(..{ let a = (1, 2); (a, a) }))\n",
    );
    let plot = doc.calls().iter().find(|c| c.callee == "lq.plot").unwrap();
    let free = doc.free_identifiers(plot.range.clone());
    assert!(!free.contains(&"a".to_string()), "{free:?}");
}

#[test]
fn bindings_are_locatable_so_a_paste_can_carry_them() {
    let doc = Document::new(CAPTURED);
    let xs = doc.binding_of("xs").expect("xs is bound");
    assert_eq!(&doc.text()[xs], "#let xs = lq.linspace(0, 10)");
    let f = doc.binding_of("f").expect("f is bound");
    assert_eq!(&doc.text()[f], "#let f(t) = calc.sin(t) * 2");
    assert!(doc.binding_of("nope").is_none());
}

/// Two targets that nest: replacing a whole data slot and replacing one element
/// inside it. Coalescing slots assume disjoint ranges, so an overlapping arrival
/// has to materialise the group rather than shift text it does not own. Found by
/// the random-intent test; kept as its own case because a seed is a poor
/// explanation of a bug.
#[test]
fn nested_targets_in_one_transaction_still_undo_exactly() {
    let mut doc = Document::new(POINTS);
    let id = doc
        .calls()
        .iter()
        .find(|c| c.callee == "lq.plot")
        .unwrap()
        .id;

    doc.begin("mixed");
    doc.apply(Intent::SetArrayElement {
        node: id,
        arg: 0,
        element: 1,
        value: "1.5".into(),
    })
    .unwrap();
    // Rewrites the whole array the element above lives in.
    doc.apply(Intent::SetPositionalArg {
        node: id,
        index: 0,
        value: "(0, 5, 10)".into(),
    })
    .unwrap();
    doc.apply(Intent::SetArrayElement {
        node: id,
        arg: 0,
        element: 2,
        value: "11".into(),
    })
    .unwrap();
    doc.commit();

    assert!(doc.text().contains("(0, 5, 11)"), "{}", doc.text());
    assert_eq!(doc.history_depth().0, 1);
    assert!(doc.undo());
    assert_eq!(doc.text(), POINTS);
}

/// The source pane types straight into the buffer. That is a direct text edit,
/// not model regeneration -- but it still has to compose with everything else:
/// one undo step per burst, and anchors that survive.
#[test]
fn typing_into_the_source_is_a_minimal_edit_that_undoes() {
    let mut doc = Document::new(DOC);
    let caption_at = DOC.find("caption").unwrap();
    let anchor = doc.anchor(caption_at);

    // What the pane does per keystroke: diff, then replace the difference.
    let mut buf = doc.text().to_string();
    doc.begin("type");
    for insert in ["/", "/", " ", "n", "o", "t", "e"] {
        let at = buf.find("#figure").unwrap();
        buf.insert_str(at, insert);
        let (range, value) =
            lilook_core::minimal_replacement(doc.text(), &buf).expect("something changed");
        assert!(
            value.len() <= 1,
            "a keystroke should not rewrite the document: {value:?}"
        );
        doc.apply(Intent::ReplaceRange { range, value }).unwrap();
    }
    doc.commit();

    assert!(doc.text().contains("// note#figure"));
    assert_eq!(doc.history_depth().0, 1, "the burst is one undo step");
    let moved = doc.anchor_offset(anchor).unwrap();
    assert_eq!(&doc.text()[moved..moved + 7], "caption");
    assert!(doc.undo());
    assert_eq!(doc.text(), DOC);
}

/// A negative number is still a number.
///
/// `-1` parses as a unary negation of `1`, not as a literal token, so the
/// classifier called it opaque: every coordinate below zero in a lilaq call got a
/// raw source editor instead of a number control. Found via `lq.ellipse(7.85, -1)`
/// refusing to be draggable, but it reaches much further than annotations --
/// `margin: -2pt`, `aspect-ratio: -1`, any negative argument at all.
#[test]
fn a_signed_number_is_editable_as_a_number() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#lq.diagram(
  lq.place(1, -1, [a]),
  lq.place(-2.5, +3, [b]),
  lq.rect(-1pt, 2, width: -0.5),
  lq.place(x, -y, [c]),
)
"#;
    let doc = Document::new(src);
    let slot = |call: usize, i: usize| {
        doc.calls()
            .iter()
            .filter(|c| c.short_name() == "place" || c.short_name() == "rect")
            .nth(call)
            .and_then(|c| c.positional.get(i).cloned())
            .unwrap_or_else(|| panic!("call {call} slot {i}"))
    };

    for (call, i, text) in [(0, 1, "-1"), (1, 0, "-2.5"), (1, 1, "+3"), (2, 0, "-1pt")] {
        let s = slot(call, i);
        assert_eq!(s.text, text);
        assert_eq!(
            s.editability,
            Editability::Literal,
            "{text} should be editable as a number"
        );
    }

    // A negated *expression* is still opaque: `-y` is a program, not a number.
    assert_eq!(slot(3, 1).text, "-y");
    assert_eq!(slot(3, 1).editability, Editability::Opaque);

    // And the annotation is movable now, which is what surfaced this.
    let places: Vec<_> = doc
        .calls()
        .iter()
        .filter(|c| c.short_name() == "place")
        .collect();
    assert!(places[0].has_literal_anchor(), "place(1, -1)");
    assert!(places[1].has_literal_anchor(), "place(-2.5, +3)");
    assert!(
        !places[2].has_literal_anchor(),
        "place(x, -y) is not literal"
    );
}

/// A frontend with no toolkit at all can drive the whole editing vocabulary.
///
/// This is the assertion the Session extraction exists for, and it is in
/// `lilook-core`'s own test suite deliberately: this crate cannot depend on
/// egui, so if any of it compiles here, none of it needed a GUI. Swift through
/// the FFI and the MCP server reach exactly this surface.
#[test]
fn a_session_is_driveable_without_a_gui() {
    use lilook_core::{CanvasEvent, Session};

    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
// a comment that must survive every operation below
#lq.diagram(width: 6cm, height: 4cm, lq.plot((0, 1, 2), (0, 1, 4)))
"#;
    let schema = Schema::from_json(lilook_core::schema::BUNDLED).expect("bundled schema");
    let mut s = Session::new(SRC, schema);
    let series = s
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .expect("a series");
    let figure = s
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "diagram")
        .map(|c| c.id)
        .expect("the diagram");

    // A gesture, in the same vocabulary the canvas speaks.
    s.handle_canvas(vec![
        CanvasEvent::Begin,
        CanvasEvent::MovePoint {
            node: series,
            index: 1,
            to: (1.5, 2.5),
        },
        CanvasEvent::Commit,
    ]);
    assert!(s.doc.text().contains("1.5"), "{}", s.doc.text());

    // A resize.
    s.handle_canvas(vec![
        CanvasEvent::Begin,
        CanvasEvent::SetSize {
            figure,
            width_pt: Some(200.0),
            height_pt: None,
        },
        CanvasEvent::Commit,
    ]);

    // Themes: apply, fork, rename -- none of which existed outside the GUI crate
    // before the split.
    s.set_theme(Some("ocean"));
    assert_eq!(s.active_theme().map(|t| t.name), Some("ocean".into()));
    assert!(s.fork_theme("house"));
    assert!(s.doc.text().contains("show: lq.theme.ocean"));
    assert!(s.rename_theme("press"));
    assert!(s.doc.text().contains("#show: press"));

    // Selecting and duplicating a series.
    s.selected = series;
    s.duplicate_selection();
    assert_eq!(
        s.doc.calls().iter().filter(|c| c.is_xy_series()).count(),
        2,
        "duplicate did not add a series"
    );

    // Starting a link is a *request* the frontend fulfils -- the session never
    // reads a file itself, which is what keeps it portable.
    s.begin_link("run.csv");
    assert!(s.queued_query.is_some(), "a link asks for a query");

    // And every step undoes byte-for-byte.
    while s.doc.history_depth().0 > 0 {
        s.doc.undo();
    }
    assert_eq!(s.doc.text(), SRC, "the session did not fully undo");
}

/// `merge_dict_field` is what makes rewriting one field of `legend: (..)` --
/// or any other dict-shaped argument -- safe: every other field survives
/// exactly as written.
#[test]
fn merge_dict_field_keeps_every_other_entry() {
    use lilook_core::merge_dict_field;

    assert_eq!(
        merge_dict_field("(position: top + left)", "position", "bottom + right"),
        "(position: bottom + right)"
    );
    // A field elsewhere in the dict, including one with its own commas and
    // parens, must not be disturbed by the rewrite.
    assert_eq!(
        merge_dict_field(
            "(position: top + left, fill: rgb(1, 2, 3, 200))",
            "position",
            "bottom + right"
        ),
        "(position: bottom + right, fill: rgb(1, 2, 3, 200))"
    );
    // Inserting a field that was not there yet is appended, not replaced.
    assert_eq!(
        merge_dict_field("(fill: white)", "position", "top + left"),
        "(fill: white, position: top + left)"
    );
    // No existing dict at all: a fresh one, same as before this existed.
    assert_eq!(
        merge_dict_field("", "position", "top + left"),
        "(position: top + left)"
    );
    assert_eq!(
        merge_dict_field("(:)", "position", "top + left"),
        "(position: top + left)"
    );
}

/// Every byte offset resolves to the call a human would name.
///
/// The second addressing axis. Everything else lilook does takes a call-site id;
/// every question a source pane asks is at a caret. The innermost call wins, so
/// a cursor inside `lq.diagram(lq.plot(..))` names the plot.
#[test]
fn a_byte_offset_resolves_to_the_call_around_it() {
    const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#lq.diagram(width: 6cm, title: "a plot", lq.plot((1, 2), (3, 4)))
"#;
    let doc = Document::new(SRC);
    let id = |name: &str| {
        doc.calls()
            .iter()
            .find(|c| c.short_name() == name)
            .map(|c| c.id)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let (diagram, plot) = (id("diagram"), id("plot"));
    let at = |needle: &str| SRC.find(needle).unwrap_or_else(|| panic!("no {needle:?}"));

    // Outside any call.
    assert_eq!(doc.at(0).call, None, "the import line is not a call");

    // Inside the diagram, on a named argument's value.
    let c = doc.at(at("6cm") + 1);
    assert_eq!(c.call, Some(diagram));
    assert_eq!(c.argument.as_deref(), Some("width"));
    assert!(!c.on_name && c.slot.is_none());

    // Between arguments is where a *name* goes.
    let c = doc.at(at("title") - 1);
    assert_eq!(c.call, Some(diagram));
    assert!(
        c.on_name,
        "between arguments, a completion offers parameters"
    );

    // Inside a string literal nothing should be offered.
    let c = doc.at(at("a plot") + 2);
    assert!(c.in_string, "inside a string, offer nothing");

    // The innermost call wins, and its positional slots are addressable.
    let c = doc.at(at("(1, 2)") + 2);
    assert_eq!(c.call, Some(plot), "the plot, not the diagram around it");
    assert_eq!(c.slot, Some(0));
    let c = doc.at(at("(3, 4)") + 2);
    assert_eq!(c.call, Some(plot));
    assert_eq!(c.slot, Some(1));

    // And it never panics, at any offset in the file or one past the end.
    for i in 0..=SRC.len() {
        if SRC.is_char_boundary(i) {
            let _ = doc.at(i);
        }
    }
}
