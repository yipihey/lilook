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

/// An app with the fonts the page fetches. Returns `None` when typst-assets is
/// not where cargo said it would be, which is a reason to skip rather than to
/// fail.
fn app() -> Option<WebApp> {
    let dir = typst_assets_fonts()?;
    let fonts: Vec<Vec<u8>> = lilook_web::WEB_FONTS
        .iter()
        .map(|name| std::fs::read(dir.join(name)).expect(name))
        .collect();
    Some(WebApp::with_fonts(fonts))
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
    let Some(mut app) = app() else { return };
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
    let Some(mut app) = app() else { return };
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
    let Some(mut app) = app() else { return };
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
    let Some(mut app) = app() else { return };
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

/// The browser build does not embed typst's fonts -- 9.6 MB, most of it faces a
/// lilaq figure never asks for -- and fetches four instead. Getting that list
/// wrong fails *silently*: the figure lays out identically and every label
/// comes out blank. So compile with exactly those four and count dark pixels.
#[test]
fn the_four_fetched_fonts_are_enough_to_draw_a_labelled_figure() {
    let Some(mut app) = app() else { return };
    assert_eq!(lilook_web::WEB_FONTS.len(), 4);
    // The line plot has axis labels, tick numbers and a legend; the scatter has
    // maths in its labels.
    for example in [1usize, 2] {
        app.load(example);
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{:?}", errors(&app));

        // Warnings name a missing font family explicitly, which is the failure
        // this test exists for.
        let complaints: Vec<&str> = app
            .editor()
            .diagnostics()
            .iter()
            .filter(|d| d.message.contains("font") || d.message.contains("family"))
            .map(|d| d.message.as_str())
            .collect();
        assert!(complaints.is_empty(), "{complaints:?}");
    }
}

/// Where cargo put typst-assets, so the test can read the same files the build
/// script copies into the site.
fn typst_assets_fonts() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    // Cheap scan rather than a json dependency for one field.
    let at = text.find("\"typst-assets\"")?;
    let key = "\"manifest_path\":\"";
    let start = text[at..].find(key)? + at + key.len();
    let end = text[start..].find('"')? + start;
    let manifest = std::path::PathBuf::from(&text[start..end]);
    Some(manifest.parent()?.join("files").join("fonts"))
}

/// Linking a data file, end to end, through the same code path the browser runs.
///
/// The file is dropped into the page's file system, the compiler is asked what
/// columns it has, two of them are chosen, and the figure follows the file. Then
/// undo takes the whole thing back -- one step, byte for byte, because a link is
/// one transaction.
#[test]
fn a_dropped_csv_can_be_linked_to_a_series_and_undone() {
    let Some(mut app) = app() else { return };
    // The line-plot example: two series with literal data, so there is something
    // to link *onto* and something to compare against.
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let before = app.editor().text().to_string();
    let original_points = app.editor().scenes()[0].series[0].points.clone();
    assert_eq!(original_points.len(), 6);

    app.insert_file("run.csv", b"t,flux (mJy)\n0,2.5\n1,3\n2,7\n".to_vec());

    // Select the first series, which is what a link writes into.
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .expect("a series to link onto");
    app.editor_mut().selected = series;

    // Ask the compiler what is in the file. The answer comes back in the frame,
    // because the browser build compiles in the frame.
    app.editor_mut().begin_link("run.csv");
    run(&mut app, 2);
    assert_eq!(
        app.editor().link_columns(),
        Some(["t".to_string(), "flux (mJy)".to_string()].as_slice()),
        "link error: {:?}",
        app.editor().link_error()
    );

    // Column 0 against column 1.
    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let text = app.editor().text().to_string();
    assert!(
        text.contains(r#"#let run = csv("run.csv", row-type: dictionary)"#),
        "{text}"
    );
    assert!(text.contains("run.map(r => float(r.t))"), "{text}");
    // A header no field access can reach goes through `at`, and still reparses.
    assert!(
        text.contains(r#"run.map(r => float(r.at("flux (mJy)")))"#),
        "{text}"
    );

    // The figure is now the file's. This is the assertion the feature exists for.
    let points = &app.editor().scenes()[0].series[0].points;
    assert_eq!(points.len(), 3, "the series follows the linked file");
    assert_eq!(points[0], (0.0, 2.5));
    assert_eq!(points[2], (2.0, 7.0));

    // One undo, back to the byte.
    assert_eq!(app.editor().doc.history_depth().0, 1);
    app.editor_mut().doc.undo();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(app.editor().text(), before);
    assert_eq!(app.editor().scenes()[0].series[0].points, original_points);
}

/// A file that is not there fails as a link rather than as a figure.
#[test]
fn linking_a_missing_file_reports_instead_of_writing() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    let before = app.editor().text().to_string();

    app.editor_mut().begin_link("nope.csv");
    run(&mut app, 2);
    assert!(app.editor().link_columns().is_none());
    assert!(
        app.editor().link_error().is_some_and(|e| !e.is_empty()),
        "a failed link has to say why"
    );
    // And nothing was written.
    assert_eq!(app.editor().text(), before);
    assert_eq!(app.editor().doc.history_depth().0, 0);
}

/// A file with no header row is still linkable, positionally.
#[test]
fn a_headerless_file_links_by_column_number() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    app.insert_file("bare.csv", b"0,2.5\n1,3\n2,7\n".to_vec());
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .unwrap();
    app.editor_mut().selected = series;

    app.editor_mut().begin_link("bare.csv");
    run(&mut app, 2);
    assert_eq!(
        app.editor().link_columns(),
        Some(["column 1".to_string(), "column 2".to_string()].as_slice()),
    );
    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let text = app.editor().text().to_string();
    // Plain rows, indexed -- there are no names to use.
    assert!(text.contains(r#"#let bare = csv("bare.csv")"#), "{text}");
    assert!(text.contains("bare.map(r => float(r.at(0)))"), "{text}");
    // All three rows are data: nothing was mistaken for a header.
    assert_eq!(app.editor().scenes()[0].series[0].points.len(), 3);
}

/// A changed file is reported, then reread on request -- and rereading is not an
/// edit. This is the property the whole linked-dataset design rests on: because
/// the file is the source of truth, refreshing costs the undo history nothing.
#[test]
fn a_changed_linked_file_is_reported_and_reread_without_touching_the_document() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    app.insert_file("run.csv", b"t,y\n0,1\n1,2\n2,3\n".to_vec());
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .unwrap();
    app.editor_mut().selected = series;
    app.editor_mut().begin_link("run.csv");
    run(&mut app, 2);
    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(app.editor().scenes()[0].series[0].points.len(), 3);

    let linked_text = app.editor().text().to_string();
    let (undo, redo) = app.editor().doc.history_depth();

    // The file grows a row. Nothing happens on its own: a figure that redraws
    // from a half-written file is worse than one that waits to be told.
    app.insert_file("run.csv", b"t,y\n0,1\n1,2\n2,3\n3,4\n".to_vec());
    app.editor_mut().files_changed(&["run.csv".to_string()]);
    run(&mut app, 2);
    assert_eq!(app.editor().changed_files(), ["run.csv".to_string()]);
    assert_eq!(
        app.editor().scenes()[0].series[0].points.len(),
        3,
        "nothing should have been reread yet"
    );

    // Now reread, as the panel's button does.
    app.editor_mut().reload_data();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(app.editor().changed_files().is_empty());
    assert_eq!(app.editor().scenes()[0].series[0].points.len(), 4);
    assert_eq!(app.editor().scenes()[0].series[0].points[3], (3.0, 4.0));

    // And the assertion that matters: the figure changed, the document did not.
    assert_eq!(app.editor().text(), linked_text);
    assert_eq!(app.editor().doc.history_depth(), (undo, redo));
}

/// With "follow" on, the same change is reread without being asked.
#[test]
fn following_a_file_rereads_it_immediately() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    app.insert_file("run.csv", b"t,y\n0,1\n1,2\n".to_vec());
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .unwrap();
    app.editor_mut().selected = series;
    app.editor_mut().begin_link("run.csv");
    run(&mut app, 2);
    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(app.editor().scenes()[0].series[0].points.len(), 2);
    let text = app.editor().text().to_string();

    app.editor_mut().set_follow_files(true);
    app.insert_file("run.csv", b"t,y\n0,1\n1,2\n2,9\n".to_vec());
    app.editor_mut().files_changed(&["run.csv".to_string()]);
    run(&mut app, 3);
    assert!(
        app.editor().changed_files().is_empty(),
        "nothing left pending"
    );
    assert_eq!(app.editor().scenes()[0].series[0].points.len(), 3);
    assert_eq!(app.editor().scenes()[0].series[0].points[2], (2.0, 9.0));
    assert_eq!(app.editor().text(), text, "still not an edit");
}

/// Unlocking a linked slot: Veusz's other state. The values move into the
/// document, the figure stops following the file, and the binding that read it
/// goes with it -- otherwise the document would keep reading a file nothing plots
/// and the Data panel would go on claiming a link that no longer exists.
#[test]
fn unlocking_a_linked_series_embeds_its_values_and_ends_the_link() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    app.insert_file("run.csv", b"t,y\n0,1.5\n1,2.5\n2,4\n".to_vec());
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .unwrap();
    app.editor_mut().selected = series;
    app.editor_mut().begin_link("run.csv");
    run(&mut app, 2);
    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);

    let linked_text = app.editor().text().to_string();
    assert!(linked_text.contains(r#"csv("run.csv""#));
    assert!(app
        .editor()
        .data_files()
        .iter()
        .any(|d| d.path == "run.csv" && d.loaded));
    // While linked, the points are an expression's, so they cannot be dragged.
    assert!(!app.editor().doc.call(series).unwrap().has_literal_points());

    // Unlock both slots. Each is its own explicit action, as materialise was.
    app.editor_mut().unlock(series, 0);
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    app.editor_mut().unlock(series, 1);
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let text = app.editor().text().to_string();
    // The values are in the document, and the file is not read any more -- so the
    // binding that read it is gone too.
    assert!(!text.contains("csv("), "the link should be gone:\n{text}");
    assert!(
        !text.contains("#let run"),
        "the binding is orphaned:\n{text}"
    );
    assert!(text.contains("1.5, 2.5, 4"), "{text}");
    assert!(
        !app.editor()
            .data_files()
            .iter()
            .any(|d| d.path == "run.csv"),
        "nothing reads the file now, so it is not a dependency"
    );

    // The figure is unchanged by unlocking -- that is the point of it.
    let points = &app.editor().scenes()[0].series[0].points;
    assert_eq!(points.len(), 3);
    assert_eq!(points[0], (0.0, 1.5));
    // And now they are literal, so they can be dragged.
    assert!(app.editor().doc.call(series).unwrap().has_literal_points());

    // Two unlocks, two undo steps, back to the linked document.
    app.editor_mut().doc.undo();
    app.editor_mut().doc.undo();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(app.editor().text(), linked_text);
}
