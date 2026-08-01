//! Headless inspector tests. `egui::__run_test_ui` runs a real egui pass with
//! no window, which is exactly the property that made egui the right choice for
//! the first frontend: an agent can drive and assert on it without a display.

use lilook_core::{Document, Editability, Schema};
use lilook_ui::{control_for, refine, widget_control, Control, Inspector, UiEvent};
use std::cell::RefCell;

const SCHEMA_JSON: &str = lilook_core::schema::BUNDLED;

const SRC: &str = r##"#import "@preview/lilaq:0.6.0" as lq
#let accent = rgb("#4c72b0")
#let xs = lq.linspace(0, 10)
#lq.diagram(
  width: 8cm, height: 5cm, xlabel: [Time],
  lq.plot(xs, xs.map(x => calc.sin(x)), stroke: red, smooth: true, mark: "o"),
  lq.plot(xs, xs.map(x => calc.cos(x)), stroke: accent),
  lq.plot((0, 1), (2, 3), stroke: 2pt + blue, color: rgb("#123456")),
  ..range(2).map(i => lq.plot(xs, xs.map(x => x + i))),
)
"##;

fn schema() -> Schema {
    Schema::from_json(SCHEMA_JSON).unwrap()
}

/// The M6 exit criterion. A regenerated schema that grows a widget kind this
/// frontend does not handle fails here rather than silently rendering a text
/// box for a whole family of parameters.
#[test]
fn every_widget_kind_in_the_schema_has_a_control() {
    let schema = schema();
    let mut kinds: Vec<&str> = schema
        .functions
        .values()
        .flat_map(|f| f.params.iter())
        .map(|p| p.widget.as_str())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert!(kinds.len() > 15, "schema looks empty: {kinds:?}");

    let unmapped: Vec<&&str> = kinds
        .iter()
        .filter(|w| widget_control(w).is_none())
        .collect();
    assert!(unmapped.is_empty(), "no control for {unmapped:?}");
}

#[test]
fn controls_follow_schema_and_editability() {
    let schema = schema();
    let doc = Document::new(SRC);
    let pick = |callee: &str, nth: usize, arg: &str| {
        let call = doc
            .calls()
            .iter()
            .filter(|c| c.callee == callee)
            .nth(nth)
            .unwrap();
        let a = call.named.iter().find(|a| a.name == arg).unwrap();
        let f = schema.function_for_callee(callee).unwrap();
        refine(
            control_for(f.params.iter().find(|p| p.name == arg)),
            a.editability,
            &a.text,
        )
    };

    assert_eq!(pick("lq.diagram", 0, "width"), Control::Length);
    // `xlabel` is a `variant` -- lq.label or content -- and the user wrote
    // content, so it gets the content editor rather than a source box.
    assert_eq!(pick("lq.diagram", 0, "xlabel"), Control::Text);
    assert_eq!(pick("lq.plot", 0, "smooth"), Control::Toggle);
    assert_eq!(pick("lq.plot", 0, "mark"), Control::Mark);
    // `red` is a builtin -> a real stroke control...
    assert_eq!(pick("lq.plot", 0, "stroke"), Control::Stroke);
    // ...`accent` is a user binding -> read-only with a jump affordance.
    assert_eq!(pick("lq.plot", 1, "stroke"), Control::ReadOnly);
    assert_eq!(pick("lq.plot", 2, "color"), Control::Color);
    // `2pt + blue` parses as a binary operation, so the core calls it opaque --
    // but the stroke editor writes that exact shape back, so it stays editable.
    assert_eq!(pick("lq.plot", 2, "stroke"), Control::Stroke);
}

#[test]
fn renders_headlessly_without_panicking() {
    let schema = schema();
    let doc = Document::new(SRC);
    for call in doc.calls() {
        let f = schema.function_for_callee(&call.callee);
        // `__run_test_ui` takes an `Fn`, so the inspector needs interior
        // mutability. Real frontends own it across frames instead.
        let insp = RefCell::new(Inspector::new(f));
        egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
    }
}

/// Every control must render without emitting an event when nobody touched it.
/// A widget that reports `changed()` on its first frame would rewrite the user's
/// document just by being looked at.
#[test]
fn merely_rendering_emits_no_events() {
    let schema = schema();
    let doc = Document::new(SRC);
    for call in doc.calls() {
        let f = schema.function_for_callee(&call.callee);
        let insp = RefCell::new(Inspector::new(f));
        egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
        egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
        assert!(
            insp.borrow().events.is_empty(),
            "{} emitted {:?}",
            call.callee,
            insp.borrow().events
        );
    }
}

#[test]
fn generated_calls_render_read_only() {
    let schema = schema();
    let doc = Document::new(SRC);
    let gen = doc
        .calls()
        .iter()
        .find(|c| c.generated)
        .expect("a generated call");
    let insp = RefCell::new(Inspector::new(schema.function_for_callee(&gen.callee)));
    egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, gen));
    assert!(
        insp.borrow().events.is_empty(),
        "a generated call must emit no edit events"
    );
}

/// The event stream a drag produces is what the caller maps onto one
/// transaction. Asserting its shape here keeps the coalescing contract honest
/// without needing a real pointer.
#[test]
fn event_shape_maps_onto_a_transaction() {
    let node = 1usize;
    let events = vec![
        UiEvent::Begin {
            node,
            param: "width".into(),
        },
        UiEvent::Set {
            node,
            param: "width".into(),
            value: "8.1cm".into(),
        },
        UiEvent::Set {
            node,
            param: "width".into(),
            value: "8.6cm".into(),
        },
        UiEvent::Commit,
    ];
    let mut doc = Document::new(SRC);
    let mut open = false;
    for e in events {
        match e {
            UiEvent::Begin { .. } => {
                doc.begin("drag");
                open = true;
            }
            UiEvent::Set { node, param, value } => {
                doc.apply(lilook_core::Intent::SetNamedArg { node, param, value })
                    .unwrap();
            }
            UiEvent::Commit => {
                doc.commit();
                open = false;
            }
            _ => {}
        }
    }
    assert!(!open);
    assert!(doc.text().contains("width: 8.6cm"));
    assert_eq!(doc.history_depth().0, 1, "a drag is one undo step");
    doc.undo();
    assert_eq!(doc.text(), SRC);
}

/// Every link state a data slot can be in has to render, and only a linked slot
/// may offer to unlock. The states are independent per slot: x can be fresh
/// while y is stale, because they are separate links.
#[test]
fn a_data_slot_renders_in_every_link_state() {
    use lilook_ui::{Context, SlotSource};

    let schema = schema();
    let doc = Document::new(SRC);
    let call = doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .expect("a series");
    let f = schema.function_for_callee(&call.callee);

    let linked = |file: &str| SlotSource {
        file: Some(file.into()),
        missing: false,
        stale: false,
    };
    let cases: Vec<(&str, Option<usize>, Vec<SlotSource>)> = vec![
        // Nothing known: no provenance row, no unlock button.
        ("unlinked, no data recovered", None, vec![]),
        // An expression whose values lilook has: materialise is offered.
        (
            "unlinked, data recovered",
            Some(64),
            vec![SlotSource::default(), SlotSource::default()],
        ),
        // Linked and fresh: unlock is offered, and it says what it will end.
        (
            "linked",
            Some(3),
            vec![linked("run.csv"), linked("flux.npz")],
        ),
        // x fresh, y stale -- the case a per-call flag could not express.
        (
            "one slot stale",
            Some(3),
            vec![
                linked("run.csv"),
                SlotSource {
                    stale: true,
                    ..linked("flux.npz")
                },
            ],
        ),
        (
            "one slot missing",
            Some(3),
            vec![
                SlotSource {
                    missing: true,
                    ..linked("gone.csv")
                },
                linked("flux.npz"),
            ],
        ),
    ];

    for (what, recovered_points, slot_sources) in cases {
        let insp = RefCell::new(Inspector::new(f).with_context(Context {
            recovered_points,
            slot_sources: &slot_sources,
        }));
        egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
        // Drawing a state must never itself be an edit.
        assert!(
            insp.borrow().events.is_empty(),
            "{what} emitted {:?}",
            insp.borrow().events
        );
    }
}

/// The "add argument" field has to outlive the `Inspector` that drew it.
///
/// This is the test that was missing, and the shape of the gap is worth keeping:
/// what the user was part-way through choosing used to be a field on
/// `Inspector`, while the shell builds a **fresh `Inspector` every frame**. Every
/// other test in this file holds one in a `RefCell` across frames, so the state
/// the app threw away was exactly the state the tests preserved -- they could not
/// have caught it. In the app it was dropped between the keystroke that made it
/// and the frame that would have acted on it.
#[test]
fn what_is_typed_into_add_argument_outlives_the_inspector_that_drew_it() {
    let schema = schema();
    let doc = Document::new(SRC);
    let call = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "diagram")
        .expect("a diagram");
    let f = schema
        .function_for_callee(&call.callee)
        .expect("its schema");

    // One context across frames, as a real app has; a new inspector each frame,
    // as the shell builds.
    let ctx = egui::Context::default();
    let frame = |ctx: &egui::Context| {
        let mut insp = Inspector::new(Some(f));
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| insp.ui(ui, call));
        insp.events
    };
    let id = lilook_ui::add_argument_filter_id(call.id);

    assert!(frame(&ctx).is_empty(), "nothing typed, nothing to do");

    // Type, the way the field does.
    ctx.data_mut(|d| d.insert_temp(id, "xsc".to_string()));

    // A later frame with a different inspector still sees it. Before the fix this
    // read back `None`: it had never left the inspector that was dropped.
    assert!(frame(&ctx).is_empty(), "narrowing a list is not an edit");
    let stored: Option<String> = ctx.data(|d| d.get_temp(id));
    assert_eq!(
        stored.as_deref(),
        Some("xsc"),
        "what was typed has to outlive the inspector that drew it"
    );

    // The id is derived from the call site alone -- not from the inspector, and
    // not from the enclosing `Ui`, whose hash moves as the panel above it grows.
    assert_eq!(id, lilook_ui::add_argument_filter_id(call.id));
    assert_ne!(id, lilook_ui::add_argument_filter_id(call.id + 1));
}

/// One click adds the whole argument, and the click lands where it looks.
///
/// Two claims in one, both of which need real pointer input. Clicking a row adds
/// that argument with a value the figure survives -- the behaviour the source
/// pane always had and the inspector's combo did not, where it took three acts.
/// And clicking a *value named on that row* writes that value: `interpolation`
/// shows `pixelated` and `smooth`, and clicking `smooth` wrote `pixelated` until
/// the values became targets of their own.
#[test]
fn one_click_adds_the_argument_and_the_value_that_was_clicked() {
    let schema = schema();
    let doc = Document::new(SRC);
    let call = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "diagram")
        .expect("a diagram");
    let f = schema
        .function_for_callee(&call.callee)
        .expect("its schema");
    let offers = lilook_core::argument_offers(&f.params, call);
    // `xscale`, which names its four scales on its own line.
    let wanted = offers
        .iter()
        .position(|o| o.param == "xscale")
        .expect("a scale to set");
    let log = offers[wanted]
        .choices
        .iter()
        .position(|(l, _)| l == "log")
        .expect("log is one of them");

    let ctx = egui::Context::default();
    let frame = |input: egui::RawInput| {
        let mut insp = Inspector::new(Some(f));
        let _ = ctx.run_ui(input, |ui| insp.ui(ui, call));
        insp.events
    };
    let click_at = |at: egui::Pos2| {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(at));
        for pressed in [true, false] {
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        input
    };

    // The popup is gated on the field having focus, the way the source pane's is
    // gated on the caret being in the buffer.
    let field = lilook_ui::add_argument_filter_id(call.id).with("field");
    ctx.memory_mut(|m| m.request_focus(field));

    // Where the row and its values landed. Ids are derived from the popup's own
    // so that a test can aim at a chosen part of a row. Several frames, because
    // an `Area` that has never been laid out does not know its own height and is
    // placed again once it does -- aiming at the first frame's rect misses.
    let row_id = lilook_ui::add_argument_filter_id(call.id)
        .with("popup")
        .with(("row", wanted));
    let chip_id = row_id.with(("choice", log));
    let (mut row, mut chip) = (egui::Rect::NOTHING, egui::Rect::NOTHING);
    for _ in 0..5 {
        assert!(frame(egui::RawInput::default()).is_empty(), "just looking");
        row = ctx.read_response(row_id).expect("the row").rect;
        chip = ctx.read_response(chip_id).expect("the value on it").rect;
    }
    assert!(row.contains_rect(chip), "the value sits on its row");

    // On the value: `log`, not the scale's own `auto`.
    assert_eq!(
        frame(click_at(chip.center())),
        vec![UiEvent::Insert {
            node: call.id,
            param: "xscale".into(),
            value: "\"log\"".into(),
        }],
        "the value the pointer was on"
    );

    // On the row itself, left of every value: the parameter, with what lilook
    // would choose for it.
    let name = egui::pos2(row.left() + 4.0, row.center().y);
    assert!(!chip.contains(name), "aiming at the name, not a value");
    ctx.memory_mut(|m| m.request_focus(field));
    for _ in 0..3 {
        frame(egui::RawInput::default());
    }
    assert_eq!(
        frame(click_at(name)),
        vec![UiEvent::Insert {
            node: call.id,
            param: "xscale".into(),
            value: "auto".into(),
        }],
    );
    // And the field lets go of what was typed, so the popup does not stay open
    // offering the argument that was just added.
    let left: Option<String> = ctx.data(|d| d.get_temp(lilook_ui::add_argument_filter_id(call.id)));
    assert_eq!(left.unwrap_or_default(), "");
}

/// Enter takes the first match, so a name that is known can be typed straight in.
#[test]
fn enter_takes_the_first_match_in_the_add_field() {
    let schema = schema();
    let doc = Document::new(SRC);
    let call = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "diagram")
        .expect("a diagram");
    let f = schema
        .function_for_callee(&call.callee)
        .expect("its schema");

    let ctx = egui::Context::default();
    let frame = |input: egui::RawInput| {
        let mut insp = Inspector::new(Some(f));
        let _ = ctx.run_ui(input, |ui| insp.ui(ui, call));
        insp.events
    };
    ctx.memory_mut(|m| m.request_focus(lilook_ui::add_argument_filter_id(call.id).with("field")));
    ctx.data_mut(|d| {
        d.insert_temp(
            lilook_ui::add_argument_filter_id(call.id),
            "xsc".to_string(),
        )
    });
    assert!(frame(egui::RawInput::default()).is_empty(), "just typing");

    let mut input = egui::RawInput::default();
    input.events.push(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    });
    assert_eq!(
        frame(input),
        vec![UiEvent::Insert {
            node: call.id,
            param: "xscale".into(),
            // `auto`, the documented default: what the parameter is set to when
            // the user has named it but not yet said what they want.
            value: "auto".into(),
        }],
    );
}

/// The promise, over every parameter in the schema: **you never type the syntax
/// for a type lilook already knows.**
///
/// Adding `title` used to land in a raw source box showing `none`, so a title had
/// to be typed as `"Flux"` or `[Flux]` by hand -- while `xlabel` needed nothing,
/// purely because its *current value* happened to be `[day]`. Every typed control
/// fell back to that box when its parser did not recognise the value, and no
/// parser recognised `none`: 140 of lilaq's 409 parameters accept `none`/`auto`
/// and most of them default to one.
///
/// So this walks the whole schema and asserts, for each parameter at each of its
/// sentinels, that the control is *not* the raw source editor -- either a typed
/// control with an empty state, or the "set" affordance. And that merely rendering
/// any of them emits nothing.
#[test]
fn no_parameter_makes_the_user_type_syntax_lilook_knows() {
    let schema = schema();
    let mut checked = 0;
    let mut with_sentinels = 0;

    for (fname, f) in schema.functions.iter() {
        for p in &f.params {
            if p.kind == "positional" {
                continue;
            }
            checked += 1;
            if p.sentinels.is_empty() {
                continue;
            }
            with_sentinels += 1;
            for s in &p.sentinels {
                let control = lilook_ui::inspector::control_of(Some(p), Editability::Literal, s);
                assert_ne!(
                    control,
                    Control::Source,
                    "{fname}.{} at `{s}` would make the user type raw source",
                    p.name
                );
                // Either a typed control that can show "empty", or the explicit
                // "not set" row -- never a box to type syntax into.
                assert!(
                    matches!(
                        control,
                        Control::Unset
                            | Control::Text
                            | Control::Enum
                            | Control::Mark
                            | Control::Scale
                    ),
                    "{fname}.{} at `{s}` gave {control:?}, which cannot show an \
                     unset value honestly",
                    p.name
                );
                // Whatever `set` would write has to reparse. Reparsing is not
                // enough on its own -- `()` parses and lilaq rejects it -- so
                // `lilook-compile`'s `seeded_arguments_compile` compiles every one
                // of these for real. This is the cheap half of that check.
                if let Some(seeded) = lilook_ui::inspector::seed_for_test(Some(p), control) {
                    assert!(
                        lilook_core::check_expr(&seeded).is_ok(),
                        "{fname}.{} seeds `{seeded}`, which does not reparse",
                        p.name
                    );
                }
            }
        }
    }
    assert!(checked > 300, "only checked {checked} parameters");
    assert!(
        with_sentinels > 100,
        "only {with_sentinels} parameters had sentinels; has the schema changed?"
    );
    eprintln!("{checked} parameters, {with_sentinels} of them with sentinels");
}

/// A value written as a string stays a string, and one written as content stays
/// content. Reopening `title: "Flux"` as a text field is the fix for having to
/// keep the quotes by hand -- but writing it back as `[Flux]` would silently
/// change the shape of the user's source.
#[test]
fn words_are_written_back_in_the_shape_they_came_in() {
    use lilook_ui::value::{parse_text, text_source, TextShape};

    assert_eq!(
        parse_text("[Flux]"),
        Some((TextShape::Content, "Flux".into()))
    );
    assert_eq!(
        parse_text("\"Flux\""),
        Some((TextShape::Str, "Flux".into()))
    );
    assert_eq!(parse_text("[]"), Some((TextShape::Content, String::new())));
    assert_eq!(parse_text("\"\""), Some((TextShape::Str, String::new())));
    // Not words: leave these to the source editor.
    assert_eq!(parse_text("none"), None);
    assert_eq!(parse_text("lq.title[x]"), None);
    assert_eq!(parse_text("\"a\" + \"b\""), None);

    // Round trip, both shapes, including what needs escaping.
    for (shape, text) in [
        (TextShape::Content, "Flux"),
        (TextShape::Content, "*bold* and $x^2$"),
        (TextShape::Str, "Flux"),
        (TextShape::Str, "he said \"no\""),
        (TextShape::Str, "a\\b"),
    ] {
        let src = text_source(shape, text);
        assert!(
            lilook_core::check_expr(&src).is_ok(),
            "{src:?} does not reparse"
        );
        assert_eq!(
            parse_text(&src),
            Some((shape, text.to_string())),
            "{src:?} did not round trip"
        );
    }
}

/// A colormap stays pickable even though its value is an expression.
///
/// `color.map.viridis` is a field access, so `Editability` calls it opaque, and
/// the general rule is that an opaque value becomes a read-only label. That rule
/// is right for a number built by arithmetic and wrong here: the picker replaces
/// the whole expression rather than editing part of one. Caught by looking at the
/// deployed site, where the most consequential control on a heatmap was a label.
#[test]
fn a_colormap_is_pickable_even_though_its_value_is_an_expression() {
    use lilook_core::{policy::control_for, Control, Editability};
    let schema = schema();
    let map = schema.functions["colormesh"]
        .params
        .iter()
        .find(|p| p.name == "map")
        .expect("colormesh takes a map");
    assert_eq!(map.widget, "colormap");
    assert_eq!(control_for(Some(map)), Control::Colormap);
    for text in ["color.map.viridis", "color.map.magma", "my-ramp"] {
        assert_eq!(
            lilook_ui::inspector::control_of(Some(map), Editability::Opaque, text),
            Control::Colormap,
            "{text} should still be pickable"
        );
    }
    // And the same for a diagram's palette, whose named form is a bare string.
    let cycle = schema.functions["diagram"]
        .params
        .iter()
        .find(|p| p.name == "cycle")
        .expect("diagram takes a cycle");
    assert_eq!(
        lilook_ui::inspector::control_of(Some(cycle), Editability::Opaque, "petroff10"),
        Control::Cycle
    );
}
