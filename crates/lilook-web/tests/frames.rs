//! The browser app, driven natively.
//!
//! A browser pane cannot be part of `cargo test`, but almost nothing here needs
//! one: the app is an `egui::Ui` away from a real frame, so a real `egui`
//! context can run it, compile each example against the bundled packages, and
//! let the assertions be about figures rather than about pixels on someone
//! else's screen.

use lilook_core::render::Severity;
use lilook_web::{WebApp, EXAMPLES};

/// Run `n` frames through a real egui context, as a browser would.
fn run(app: &mut WebApp, n: usize) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    for _ in 0..n {
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.frame(ui));
    }
}

fn errors(app: &WebApp) -> Vec<String> {
    app.editor()
        .diagnostics()
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn every_example_compiles_and_yields_a_scene() {
    let mut app = WebApp::new();
    for (i, (name, source)) in EXAMPLES.iter().enumerate() {
        app.load(i);
        assert_eq!(app.editor().text(), *source, "{name} did not load");
        // A few frames: one to ask for a compile, one to take the result.
        run(&mut app, 3);

        assert!(errors(&app).is_empty(), "{name}: {:?}", errors(&app));
        assert!(
            !app.editor().scenes().is_empty(),
            "{name} produced no scene to click on"
        );
    }
}

/// The example the browser build exists to show: lilaq's stacked area chart,
/// editable rather than a picture of a figure.
#[test]
fn the_stacked_area_example_is_a_figure_lilook_understands() {
    let mut app = WebApp::new();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let doc = &editor.doc;
    // One diagram; the areas inside it come out of a fold and a map, so they
    // are visible and selectable but not structurally editable -- which is
    // what the source says, and why the text pane matters for this example.
    assert_eq!(doc.figures().len(), 1);
    assert!(
        doc.calls().iter().any(|c| c.generated),
        "the areas are generated"
    );

    let scene = &editor.scenes()[0];
    let diagram = doc.call(scene.figure).unwrap();
    assert!(!diagram.generated, "the diagram itself is the user's call");
    assert!(diagram.named.iter().any(|a| a.name == "width"));

    // The transform is recovered, so the canvas can hit-test in data space.
    assert!(scene.transform.x.max > scene.transform.x.min);
    assert!(scene.transform.y.scale < 0.0, "page y grows downward");
}

/// Editing works as it does on the desktop: an intent, a recompile, an undo.
#[test]
fn an_edit_recompiles_and_undoes() {
    let mut app = WebApp::new();
    run(&mut app, 3);
    let before = app.editor().scenes()[0].transform;
    let figure = app.editor().scenes()[0].figure;

    app.editor_mut()
        .doc
        .apply(lilook_core::Intent::SetNamedArg {
            node: figure,
            param: "width".into(),
            value: "6cm".into(),
        })
        .unwrap();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);

    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let after = app.editor().scenes()[0].transform;
    assert_ne!(
        before.x.scale, after.x.scale,
        "a narrower diagram should rescale the axis"
    );

    app.editor_mut().doc.undo();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(app.editor().text(), EXAMPLES[0].1);
}

/// The line-plot example is the one with literal data, so its points are
/// draggable in the browser exactly as they are on the desktop.
#[test]
fn the_line_plot_examples_points_are_editable() {
    let mut app = WebApp::new();
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let series: Vec<_> = editor
        .doc
        .calls()
        .iter()
        .filter(|c| c.is_xy_series())
        .collect();
    assert_eq!(series.len(), 2);
    assert!(
        series.iter().all(|c| c.has_literal_points()),
        "both series are literal arrays"
    );
    assert_eq!(editor.scenes()[0].series.len(), 2);
    assert_eq!(editor.scenes()[0].series[0].points.len(), 6);
}
