//! Headless inspector tests. `egui::__run_test_ui` runs a real egui pass with
//! no window, which is exactly the property that made egui the right choice for
//! the first frontend: an agent can drive and assert on it without a display.

use lilook_core::{Document, Schema};
use lilook_ui::{control_for, refine, widget_control, Control, Inspector, UiEvent};
use std::cell::RefCell;

const SCHEMA_JSON: &str = include_str!("../../../assets/lilaq-0.6.0.schema.json");

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
    assert_eq!(pick("lq.diagram", 0, "xlabel"), Control::Content);
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
