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

/// Dropping a library on the window adds it to yours.
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
    assert_eq!(e.prefs.saved.len(), 2, "both arrived: {said}");
    assert!(e.prefs_dirty, "and will be written out");
    assert!(said.contains("2 added"), "said what happened: {said}");
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
