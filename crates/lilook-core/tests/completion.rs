//! What may be written at the caret. Schema and parse only — never a compile.

use lilook_core::{Schema, Session};

const SCHEMA: &str = lilook_core::schema::BUNDLED;
const SRC: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#lq.diagram(width: 8cm, yscale: "log",
  lq.colormesh(xs, ys, f, map: color.map.viridis),
)
"#;

fn session() -> Session {
    Session::new(SRC, Schema::from_json(SCHEMA).expect("schema"))
}

fn at(needle: &str) -> usize {
    SRC.find(needle).unwrap_or_else(|| panic!("no {needle:?}"))
}

#[test]
fn parameter_names_are_offered_where_a_name_goes() {
    let s = session();
    // Just before `yscale:`, between arguments.
    let names: Vec<String> = s
        .completions(at("yscale") - 1)
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(names.contains(&"title".to_string()), "{names:?}");
    assert!(names.contains(&"xlabel".to_string()));
    // Already written, so not offered again.
    assert!(
        !names.contains(&"width".to_string()),
        "width is already set"
    );
    assert!(
        !names.contains(&"map".to_string()),
        "map is not a diagram parameter"
    );

    // Accepting one leaves a value that compiles: the policy's safe seed.
    let width = s.completions(at("yscale") - 1);
    let title = width.iter().find(|c| c.label == "xlabel").expect("xlabel");
    assert!(title.insert.starts_with("xlabel:"), "{}", title.insert);
}

#[test]
fn a_parameters_own_values_are_offered_on_its_value() {
    let s = session();
    // On `yscale`'s value: the scale names.
    let vals: Vec<String> = s
        .completions(at("\"log\"") + 2)
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(vals.contains(&"log".to_string()), "{vals:?}");
    assert!(vals.contains(&"linear".to_string()));

    // On `map`: the colormaps, from the same table the picker uses.
    let maps: Vec<String> = s
        .completions(at("color.map.viridis") + 4)
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(maps.contains(&"magma".to_string()), "{maps:?}");
    assert_eq!(
        maps.len(),
        lilook_core::COLORMAPS.len(),
        "the text pane offers what the inspector does"
    );
    let magma = s
        .completions(at("color.map.viridis") + 4)
        .into_iter()
        .find(|c| c.label == "magma")
        .unwrap();
    assert_eq!(
        magma.insert, "color.map.magma",
        "an expression, not a bare name"
    );
}

#[test]
fn nothing_is_offered_inside_a_string_or_outside_a_call() {
    let s = session();
    assert!(
        s.completions(at("@preview") + 2).is_empty(),
        "inside a string"
    );
    assert!(s.completions(0).is_empty(), "outside any call");
}

#[test]
fn signature_help_names_the_call_and_the_active_parameter() {
    let s = session();
    let sig = s.signature(at("\"log\"") + 2).expect("inside the diagram");
    assert_eq!(sig.name, "diagram");
    assert_eq!(sig.active.as_deref(), Some("yscale"));
    assert!(sig.params.contains(&"xlim".to_string()));
    assert!(!sig.doc.is_empty(), "one line of what it is");

    // The innermost call wins.
    let sig = s
        .signature(at("map: color") + 1)
        .expect("inside the colormesh");
    assert_eq!(sig.name, "colormesh");
}

/// Inside a data slot the user is writing an expression, not naming a parameter.
///
/// `lq.plot(x, calc.|` wants arithmetic. Offering `stroke:` there would be
/// nonsense, and offering nothing — which is what happened before — sends people
/// to a browser tab for `calc.sqrt`.
#[test]
fn a_data_slot_offers_expressions_rather_than_parameter_names() {
    let s = session();
    let inside = at("xs, ys") + 1;
    let names: Vec<String> = s.completions(inside).into_iter().map(|c| c.label).collect();
    assert!(names.contains(&"calc.sqrt".to_string()), "{names:?}");
    assert!(names.contains(&"range".to_string()));
    assert!(
        !names.iter().any(|n| n.contains("stroke")),
        "not parameter names: {names:?}"
    );
}

/// Every helper offered is something typst accepts.
///
/// A hand-kept table earns its keep only if it is right, so each entry is parsed
/// — with the open parenthesis closed, since a completion leaves the caret
/// inside the call.
#[test]
fn every_helper_offered_is_real_typst() {
    for (label, insert, note) in lilook_core::TYPST_HELPERS {
        assert!(!note.is_empty(), "{label} has no explanation");
        let expr = match insert.ends_with('(') {
            true => format!("{insert}1)"),
            false => insert.to_string(),
        };
        assert!(
            lilook_core::check_expr(&expr).is_ok(),
            "{label} offers {expr:?}, which does not parse"
        );
    }
}
