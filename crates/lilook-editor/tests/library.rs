//! The library panel's two shell-facing gestures: taking another library in,
//! and asking for yours as a file.

use lilook_core::{Kind, Prefs, Schema};
use lilook_editor::Editor;

fn editor() -> Editor {
    Editor::new(
        "#import \"@preview/lilaq:0.6.0\" as lq\n".to_string(),
        Schema::from_json(lilook_core::schema::BUNDLED).expect("bundled schema"),
    )
}

/// Answer whatever the editor is asking about, as a compiler would.
///
/// `ok` decides whether the value is accepted, which is what lets these tests
/// drive the whole import loop without a compiler in the room.
fn answer(e: &mut Editor, ok: bool) -> bool {
    e.pump_checks();
    let Some(expr) = e.queued_query.take() else {
        return false;
    };
    let diagnostics = match ok {
        true => vec![],
        false => vec![lilook_core::Diagnostic {
            severity: lilook_core::Severity::Error,
            message: "not a list of colours".into(),
            range: None,
            hints: vec![],
        }],
    };
    e.accept_answer(&expr, None, &diagnostics);
    true
}

/// Dropping a library on the window adds it to yours -- once the compiler has
/// agreed that what is in it can be drawn with.
///
/// The whole import gesture, because neither shell has a file dialog and the
/// browser would otherwise need a second way of getting bytes in.
#[test]
fn a_dropped_library_is_taken_in() {
    let mut theirs = Prefs::default();
    theirs
        .save(Kind::Cycle, "warm", "(red, orange)")
        .expect("saved");
    theirs
        .save(Kind::Colormap, "dusk", "(black, white)")
        .expect("saved");

    let mut e = editor();
    let said = e.import_library(&theirs.to_toml());
    assert!(said.contains("checking"), "not yet -- asking first: {said}");
    assert!(
        e.prefs.saved.is_empty(),
        "nothing offered before it is checked"
    );

    while answer(&mut e, true) {}
    assert_eq!(e.prefs.saved.len(), 2, "both arrived: {}", e.status);
    assert!(e.prefs_dirty, "and will be written out");
    assert!(
        e.status.contains("2 added"),
        "said what happened: {}",
        e.status
    );
}

/// A value the compiler refuses never reaches the library.
///
/// This is the case `check_expr` cannot see: `(1, 2, 3)` reparses perfectly and
/// draws nothing. Before, it went into the menu and failed at whichever figure
/// the user first chose it for.
#[test]
fn a_value_the_compiler_refuses_is_kept_out() {
    let mut theirs = Prefs::default();
    theirs
        .save(Kind::Cycle, "wrong", "(1, 2, 3)")
        .expect("reparses");

    let mut e = editor();
    e.import_library(&theirs.to_toml());
    while answer(&mut e, false) {}

    assert!(e.prefs.saved.is_empty(), "not in the library");
    assert!(!e.prefs_dirty, "and nothing to write out");
    assert!(
        e.status.contains("refused") && e.status.contains("wrong"),
        "named, not dropped quietly: {}",
        e.status
    );
}

/// A theme is admitted without asking, and that is deliberate.
///
/// Typst resolves what is inside a closure when it is *called*, so evaluating a
/// theme's definition proves nothing about it. The document that uses it answers
/// for it, at the compile that follows.
#[test]
fn a_theme_is_not_put_to_the_compiler() {
    let mut theirs = Prefs::default();
    theirs.save(Kind::Theme, "mine", "it => it").expect("saved");

    let mut e = editor();
    let said = e.import_library(&theirs.to_toml());
    assert!(
        e.prefs.get(Kind::Theme, "mine").is_some(),
        "straight in: {said}"
    );
    e.pump_checks();
    assert!(e.queued_query.is_none(), "and nothing was asked");
}

/// Yours wins its name; theirs is kept beside it rather than dropped.
#[test]
fn an_import_keeps_both_sides() {
    let mut e = editor();
    e.prefs
        .save(Kind::Cycle, "warm", "(red, orange)")
        .expect("mine");
    let mut theirs = Prefs::default();
    theirs
        .save(Kind::Cycle, "warm", "(pink, brown)")
        .expect("theirs");

    let said = e.import_library(&theirs.to_toml());
    while answer(&mut e, true) {}
    let said = format!("{said} {}", e.status);
    assert_eq!(e.prefs.of(Kind::Cycle).count(), 2, "both: {said}");
    assert_eq!(
        e.prefs.get(Kind::Cycle, "warm").map(|s| s.value.as_str()),
        Some("(red, orange)"),
        "mine under my name"
    );
    assert!(said.contains("renamed"), "and said so: {said}");
}

/// A `.toml` that is not a library is refused with a reason, not merged into
/// nothing and called success.
#[test]
fn a_toml_that_is_not_a_library_is_refused() {
    let mut e = editor();
    let said = e.import_library("[package]\nname = \"something-else\"\n");
    assert!(said.contains("not a lilook library"), "{said}");
    assert!(!e.prefs_dirty, "and nothing to save");
}

/// Asking for the library as a file is a request, like every other file the
/// editor cannot write itself: the desktop puts it beside the document, the
/// page downloads it. What the shell writes is the library as it stands.
#[test]
fn exporting_is_a_request_the_shell_answers() {
    let mut e = editor();
    e.prefs.save(Kind::Cycle, "warm", "(red,)").expect("saved");
    assert!(!e.requests.library_export, "not until asked");
    e.requests.library_export = true;
    let out = Prefs::from_toml(&e.prefs.to_toml()).expect("what the shell would write");
    assert_eq!(out.saved.len(), 1, "the library as it stands");
}
