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

/// The plot-grid example, after lilaq's tutorial: four diagrams laid out by
/// Typst's own `grid`, with `colspan` and `rowspan` cells and a contour shared
/// between a diagram and a colorbar.
///
/// Two things are under test. Nested diagrams: a `lq.diagram` inside
/// `grid.cell(..)` is still a figure with its own frame, so every cell can be
/// clicked and resized independently. And a series reaching a diagram *by name*:
/// the contour is `#let mesh = lq.contour(..)` because the colorbar needs the same
/// object, so nesting alone finds nothing and that diagram used to come out empty.
#[test]
fn every_cell_of_a_plot_grid_is_a_figure_lilook_can_edit() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "plot grid")
        .expect("the plot grid example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let doc = &editor.doc;
    assert_eq!(doc.figures().len(), 4, "one diagram per grid cell");
    // A scene per diagram. Not `scenes().len()`: a colorbar has a scene of its
    // own now, so scenes and diagrams are no longer the same count.
    let diagrams: Vec<&lilook_core::Scene> = editor
        .scenes()
        .iter()
        .filter(|s| {
            doc.call(s.figure)
                .is_some_and(|c| c.short_name() == "diagram")
        })
        .collect();
    assert_eq!(diagrams.len(), 4, "and a scene for each");

    // Every cell got its own frame, and no two are the same rectangle -- which is
    // what proves the grid was laid out rather than the diagrams stacked.
    let mut areas: Vec<(i64, i64, i64, i64)> = diagrams
        .iter()
        .map(|s| {
            (
                s.area.0 as i64,
                s.area.1 as i64,
                s.area.2 as i64,
                s.area.3 as i64,
            )
        })
        .collect();
    areas.sort_unstable();
    areas.dedup();
    assert_eq!(areas.len(), 4, "each cell needs its own frame");

    // The colspan cell spans the full width, so it is wider than any other.
    let widths: Vec<f64> = diagrams.iter().map(|s| s.area.2 - s.area.0).collect();
    let widest = widths.iter().cloned().fold(f64::MIN, f64::max);
    assert!(
        widths.iter().filter(|w| **w == widest).count() == 1,
        "the colspan: 3 cell should be the only full-width one: {widths:?}"
    );

    // The contour arrives by name. Its figure must own it, and its data must have
    // been recovered -- the `lq.linspace` arguments re-evaluated where they were
    // written, not where the diagram is.
    let contour = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "contour")
        .expect("the contour");
    let owner = doc
        .figures()
        .into_iter()
        .find(|f| f.series.contains(&contour.id))
        .expect("a series passed by name still belongs to the diagram that draws it");
    let geom = editor
        .scenes()
        .iter()
        .find(|s| s.figure == owner.node)
        .and_then(|s| s.series.iter().find(|g| g.node == contour.id))
        .expect("and its data comes back");

    // A contour is a *grid*, not a list of points: its two axes are independent,
    // so there is nothing to pair and no marker to draw. Reading them as pairs
    // zipped them into a truncated diagonal of markers corresponding to nothing.
    assert_eq!(
        geom.grid,
        Some((50, 50)),
        "lq.linspace's default resolution"
    );
    assert!(geom.points.is_empty(), "a mesh has no paired points");
    assert_eq!(geom.channel("x").map(|v| v.len()), Some(50));
    assert_eq!(geom.channel("y").map(|v| v.len()), Some(50));

    // And nothing to drag, whatever the axes were written as.
    assert!(!contour.has_literal_points());

    // It is still selectable, though -- by the area it covers, which is what it
    // looks like on the page.
    let scene = editor
        .scenes()
        .iter()
        .find(|s| s.figure == owner.node)
        .unwrap();
    let middle = (
        (scene.area.0 + scene.area.2) / 2.0,
        (scene.area.1 + scene.area.3) / 2.0,
    );
    let hit = scene
        .hit_mesh(middle)
        .expect("clicking a contour should select the contour, not the diagram");
    assert_eq!(hit.node, contour.id);

    // The colorbar is not a series -- it renders another plot's scale -- and must
    // not be mistaken for one.
    let colorbar = doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "colorbar")
        .expect("the colorbar");
    assert!(!colorbar.is_xy_series());
    assert!(
        !doc.figures()
            .iter()
            .any(|f| f.series.contains(&colorbar.id)),
        "a colorbar draws no data of its own"
    );
}

/// A colormesh is a grid, and lilook has to say so.
///
/// This is the shape that was read wrongest: `colormesh(xs, ys, z)` over 60x40
/// was reported as *40 paired points down the diagonal* -- zipped, so truncated
/// to the shorter axis -- drawn as draggable markers that corresponded to nothing
/// in the figure, with both axes claiming the wrong length.
#[test]
fn a_colormesh_is_a_grid_not_a_diagonal_of_points() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "colormesh")
        .expect("the colormesh example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let mesh_call = editor
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "colormesh")
        .expect("the colormesh");
    assert!(mesh_call.is_xy_series());
    assert_eq!(mesh_call.series_shape(), lilook_core::SeriesShape::Mesh);

    // It reaches its diagram by name, since the colorbar shares it.
    let owner = editor
        .doc
        .figures()
        .into_iter()
        .find(|f| f.series.contains(&mesh_call.id))
        .expect("the diagram that draws it");
    let scene = editor
        .scenes()
        .iter()
        .find(|s| s.figure == owner.node)
        .expect("its scene");
    let geom = scene
        .series
        .iter()
        .find(|g| g.node == mesh_call.id)
        .expect("its data");

    // The axes keep their own lengths, and neither is truncated to the other.
    assert_eq!(geom.grid, Some((60, 40)));
    assert_eq!(geom.channel("x").map(|v| v.len()), Some(60));
    assert_eq!(geom.channel("y").map(|v| v.len()), Some(40));
    assert!(geom.points.is_empty(), "a grid has no paired points");
    assert_eq!(
        geom.channel_lengths(),
        vec![
            ("x".to_string(), 60),
            ("y".to_string(), 40),
            // The field itself, flattened: one value per cell, not per axis.
            ("z".to_string(), 2400)
        ]
    );
    assert!(!mesh_call.has_literal_points(), "nothing to drag on a grid");

    // What the tree actually says. Asserted here because the first attempt at
    // this wording never took effect -- the edit silently failed to match after a
    // reformat, and the tree went on reporting "0 pts" for a 60x40 field while
    // every other test passed.
    assert_eq!(geom.summary(), "60×40 grid");

    // The extent covers what the axes span, and picking works anywhere inside it
    // -- a field has no vertex to aim at.
    let ((x0, x1), (y0, y1)) = geom.extent().expect("a mesh has an extent");
    assert!(
        (x0 - -3.0).abs() < 1e-9 && (x1 - 3.0).abs() < 1e-9,
        "{x0} {x1}"
    );
    assert!(
        (y0 - -2.0).abs() < 1e-9 && (y1 - 2.0).abs() < 1e-9,
        "{y0} {y1}"
    );

    for (fx, fy) in [(0.5, 0.5), (0.1, 0.9), (0.9, 0.1)] {
        let at = (
            scene.area.0 + fx * (scene.area.2 - scene.area.0),
            scene.area.1 + fy * (scene.area.3 - scene.area.1),
        );
        let hit = scene
            .hit_mesh(at)
            .unwrap_or_else(|| panic!("no hit at {fx},{fy} inside the field"));
        assert_eq!(hit.node, mesh_call.id);
        // The index names one grid cell, row-major over 60 columns.
        assert!(hit.index < 60 * 40, "{} out of range", hit.index);

        // And it reads the field there. The example's `z` is a function, so this
        // is the whole recovery path: lilaq evaluated it to draw, and the probe
        // evaluated it again to report -- which is affordable only because comemo
        // makes the second pass nearly free.
        let z = scene.field_at(&hit).expect("the field under the cursor");
        let (xs, ys) = (geom.channel("x").unwrap(), geom.channel("y").unwrap());
        let (col, row) = (hit.index % 60, hit.index / 60);
        let want = (-(xs[col] * xs[col] + ys[row] * ys[row]) / 3.0).exp() * (3.0 * xs[col]).cos();
        assert!(
            (z - want).abs() < 1e-9,
            "field at ({col},{row}) is {z}, expected {want}"
        );
    }

    // Outside the axes there is no mesh to hit.
    assert!(scene
        .hit_mesh((scene.area.0 - 20.0, scene.area.1 - 20.0))
        .is_none());
}

/// Threshold lines, each draggable on its own.
///
/// `hlines(4, 6.5)` is two lines in one call, and each coordinate is a separate
/// positional argument -- so the canvas has to offer each one independently and
/// the edit is a slot rewrite, not an array-element one.
#[test]
fn threshold_lines_are_grabbable_along_their_length() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "thresholds")
        .expect("the thresholds example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let scene = &editor.scenes()[0];
    let by_name = |n: &str| {
        editor
            .doc
            .calls()
            .iter()
            .find(|c| c.short_name() == n)
            .unwrap_or_else(|| panic!("the {n}"))
            .clone()
    };
    let (h, v) = (by_name("hlines"), by_name("vlines"));

    // Both belong to the figure, and are described as lines rather than points.
    let figure = editor.doc.figures().into_iter().next().expect("a diagram");
    assert!(figure.series.contains(&h.id) && figure.series.contains(&v.id));
    let geom = |id: usize| scene.series.iter().find(|g| g.node == id).unwrap();
    assert_eq!(geom(h.id).summary(), "2 horizontal lines");
    assert_eq!(geom(v.id).summary(), "1 vertical line");
    assert_eq!(geom(h.id).rules(), vec![4.0, 6.5]);
    assert_eq!(geom(v.id).rules(), vec![8.0]);

    // Each line is grabbable anywhere along its length, and reports which
    // argument it came from.
    for (id, coord, want_slot) in [(h.id, 4.0, 0), (h.id, 6.5, 1), (v.id, 8.0, 0)] {
        let is_horizontal = id == h.id;
        let at = if is_horizontal {
            (
                (scene.area.0 + scene.area.2) / 2.0,
                scene.transform.y.to_page(coord),
            )
        } else {
            (
                scene.transform.x.to_page(coord),
                (scene.area.1 + scene.area.3) / 2.0,
            )
        };
        let hit = scene
            .hit_rule(at, 4.0)
            .unwrap_or_else(|| panic!("no rule at {coord}"));
        assert_eq!(hit.node, id);
        assert_eq!(
            hit.index, want_slot,
            "wrong argument for the line at {coord}"
        );
    }

    // And the editor offers both calls for dragging, since every coordinate in
    // each is a literal number it can rewrite.
    assert_eq!(h.literal_rules(), vec![0, 1]);
    assert_eq!(v.literal_rules(), vec![0]);
}

/// Distributions: one dataset per argument, positioned by a named `x:`.
///
/// The default is `x: auto`, which lilaq resolves to `1..n` -- so without
/// resolving it the positions would be unknown in the commonest case, and there
/// would be nothing to hit-test against.
#[test]
fn a_boxplot_reports_its_datasets_and_can_be_picked() {
    use lilook_core::{Axis, SeriesShape};

    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "distributions")
        .expect("the distributions example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let call = editor
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "boxplot")
        .expect("the boxplot");
    assert_eq!(call.series_shape(), SeriesShape::Distributions(Axis::X));
    // Not points and not draggable as such: there is no coordinate pair here.
    assert!(!call.has_literal_points());
    assert!(call.literal_rules().is_empty());

    let figure = editor.doc.figures().into_iter().next().expect("a diagram");
    assert!(
        figure.series.contains(&call.id),
        "it belongs to the diagram"
    );

    let scene = &editor.scenes()[0];
    let geom = scene
        .series
        .iter()
        .find(|g| g.node == call.id)
        .expect("its data");

    // Three datasets, positioned 1, 2, 3 by `auto`, with the sizes from the file.
    assert_eq!(geom.summary(), "3 distributions");
    let dists = geom.distributions();
    assert_eq!(
        dists
            .iter()
            .map(|(at, v)| (*at, v.len()))
            .collect::<Vec<_>>(),
        vec![(1.0, 9), (2.0, 8), (3.0, 10)]
    );
    // And the values themselves came back, not just the counts.
    assert_eq!(dists[1].1.first().copied(), Some(5.2));

    // Each box is pickable where it sits, and reports which argument it was.
    for (index, (at, values)) in dists.iter().enumerate() {
        let mid = values.iter().sum::<f64>() / values.len() as f64;
        let page = (
            scene.transform.x.to_page(*at),
            scene.transform.y.to_page(mid),
        );
        let hit = scene
            .hit_distribution(page, 4.0)
            .unwrap_or_else(|| panic!("no box at x={at}"));
        assert_eq!(hit.node, call.id);
        assert_eq!(hit.index, index, "wrong dataset for the box at x={at}");
    }

    // Well outside every box's range of values, nothing is picked -- the region
    // is the data's own extent, not the whole column.
    let far = (
        scene.transform.x.to_page(1.0),
        scene.transform.y.to_page(100.0),
    );
    assert!(scene.hit_distribution(far, 4.0).is_none());
}

/// "Materialise" is only offered where there is a flat array to write.
///
/// `points` is empty for a mesh, a rule and a distribution, so offering it would
/// write `()` into the slot and break the figure -- the same shape of bug as
/// seeding `xlim` with an empty array, which reached a live page once already.
#[test]
fn nothing_offers_to_embed_data_it_does_not_have() {
    use lilook_core::SeriesShape;

    let Some(mut app) = app() else { return };
    for name in ["colormesh", "thresholds", "distributions"] {
        let index = EXAMPLES.iter().position(|(n, _)| *n == name).unwrap();
        app.load(index);
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{name}: {:?}", errors(&app));

        for geom in app.editor().scenes().iter().flat_map(|s| &s.series) {
            if geom.shape == SeriesShape::Points {
                continue;
            }
            assert!(
                geom.points.is_empty(),
                "{name}: only a paired-point series has points"
            );
            // What the inspector would embed: nothing, so it must not offer to.
            assert_eq!(
                lilook_core::data_array_source(&[]).unwrap(),
                "()",
                "an empty array is what the offer would have written"
            );
        }
    }
}

/// Every annotation in the example is movable, and each by its own kind of edit.
#[test]
fn annotations_are_all_movable() {
    use lilook_core::SeriesShape;

    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "annotations")
        .expect("the annotations example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let editor = app.editor();
    let figure = editor.doc.figures().into_iter().next().expect("a diagram");
    let scene = &editor.scenes()[0];

    for (name, shape, want_points, summary) in [
        ("rect", SeriesShape::Anchor, 1, "at (1.2, 1.1)"),
        ("place", SeriesShape::Anchor, 1, "at (1.4, 0.95)"),
        ("ellipse", SeriesShape::Anchor, 1, "at (7.85, -1)"),
        ("line", SeriesShape::Vertices, 2, "2 vertices"),
    ] {
        let call = editor
            .doc
            .calls()
            .iter()
            .find(|c| c.short_name() == name)
            .unwrap_or_else(|| panic!("the {name}"));
        assert_eq!(call.series_shape(), shape, "{name}");
        assert!(
            figure.series.contains(&call.id),
            "{name} belongs to the figure"
        );

        let geom = scene
            .series
            .iter()
            .find(|g| g.node == call.id)
            .unwrap_or_else(|| {
                panic!(
                    "{name} (#{}) recovered no geometry; scene has {:?}",
                    call.id,
                    scene.series.iter().map(|g| g.node).collect::<Vec<_>>()
                )
            });
        assert_eq!(geom.points.len(), want_points, "{name}");
        // The shape survives the round trip through the probe, which is what stops
        // the inspector offering to embed an anchor as an array.
        assert_eq!(geom.shape, shape, "{name} geometry");
        assert_eq!(geom.summary(), summary, "{name}");

        // Every handle is where the source says it is, and pickable there.
        for (i, p) in geom.points.iter().enumerate() {
            let hit = scene
                .hit(scene.transform.to_page(*p), 4.0)
                .unwrap_or_else(|| panic!("{name} handle {i} is not pickable"));
            assert_eq!(hit.node, call.id, "{name} handle {i}");
        }

        // And lilook will actually move it: literal coordinates throughout.
        match shape {
            SeriesShape::Anchor => assert!(call.has_literal_anchor(), "{name}"),
            SeriesShape::Vertices => {
                assert_eq!(
                    call.literal_vertices().len(),
                    call.positional.len(),
                    "{name}"
                )
            }
            _ => {}
        }
    }
}

/// A JSON file, linked through the real flow, in the shape lilaq recommends.
///
/// lilaq's data-loading tutorial argues for JSON over CSV because the values are
/// already typed -- and the same page is why this is worth a test of its own
/// rather than being folded into the CSV one. Two things are specific to it:
/// discovery reads the object's *keys* rather than a header row, and a JSON file
/// usually carries scalar metadata beside its arrays. `"title"` is not something
/// anyone can plot, and offering it would produce a series of nothing.
#[test]
fn a_json_object_links_its_arrays_and_not_its_metadata() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    app.insert_file(
        "subjects.json",
        br#"{"title": "run 4", "n": 3, "age": [19, 10, 42], "height": [165, 140, 178]}"#.to_vec(),
    );
    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .expect("a series to link onto");
    app.editor_mut().selected = series;

    app.editor_mut().begin_link("subjects.json");
    run(&mut app, 2);
    // The two arrays, and neither of the scalars.
    assert_eq!(
        app.editor().link_columns(),
        Some(["age".to_string(), "height".to_string()].as_slice()),
        "link error: {:?}",
        app.editor().link_error()
    );

    assert!(app.editor_mut().confirm_link(0, 1));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let text = app.editor().text().to_string();
    assert!(
        text.contains(r#"#let subjects = json("subjects.json")"#),
        "{text}"
    );
    // A lookup, not a destructure: the slot has to keep naming its key, or the
    // inspector cannot say where the numbers came from.
    assert!(text.contains("subjects.age"), "{text}");
    assert!(text.contains("subjects.height"), "{text}");
    // And no `float()`: JSON is typed, which is lilaq's whole argument for it.
    assert!(!text.contains("float("), "{text}");

    let points = &app.editor().scenes()[0].series[0].points;
    assert_eq!(points.len(), 3);
    assert_eq!(points[0], (19.0, 165.0));
    assert_eq!(points[2], (42.0, 178.0));
}

/// Themes: apply one, switch it, fork it, override it, rename it, undo it all.
///
/// lilaq's themes are show rules, so every step here is an ordinary text edit
/// and lilook needs no theme format of its own. The step that decides the design
/// is the fork: it *composes* the base theme rather than copying its body, so
/// `schoolbook`'s `@preview/tiptoe` import and its local helpers never have to be
/// chased, and a theme lilaq revises stays revised.
#[test]
fn themes_can_be_switched_forked_and_renamed() {
    let Some(mut app) = app() else { return };
    // Example 0 deliberately: its first call is `lq.vec.add`, and taking the
    // alias off the first call site made that `lq.vec` -- so the picker wrote
    // `#show: lq.vec.theme.ocean` and lilaq answered "module `vec` does not
    // contain `theme`". Caught on the deployed site, not by a test.
    app.load(0);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(
        app.editor().text().contains("lq.vec.add"),
        "the fixture has to keep its nested call for this to test anything"
    );
    assert_eq!(app.editor().doc.lilaq_alias(), "lq");
    let original = app.editor().text().to_string();
    assert!(app.editor().active_theme().is_none(), "starts unthemed");

    // Every theme lilaq ships must apply and still compile. This is the check
    // that a name in the schema is a name that works.
    let themes = app.editor().schema.themes.clone();
    assert_eq!(themes, ["misty", "moon", "ocean", "schoolbook", "skyline"]);
    for t in &themes {
        app.editor_mut().set_theme(Some(t));
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{t}: {:?}", errors(&app));
        let active = app.editor().active_theme().expect("a theme is in force");
        assert_eq!(&active.name, t);
        assert!(!active.local);
        // Switching replaces rather than stacks.
        assert_eq!(app.editor().doc.themes().len(), 1, "{t}");
        assert!(!app.editor().scenes().is_empty(), "{t} drew nothing");
    }

    // Fork the last one into a theme of the user's own.
    assert!(app.editor_mut().fork_theme("paper"));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let text = app.editor().text().to_string();
    assert!(text.contains("#let paper = it => {"), "{text}");
    // Composed, not copied: the base theme is still the authority.
    assert!(text.contains("show: lq.theme.skyline"), "{text}");
    assert!(text.contains("#show: paper"), "{text}");
    let active = app.editor().active_theme().expect("the fork is in force");
    assert!(active.local && active.name == "paper");

    // And now it is editable the ordinary way: a `set-*` rule inside it, which
    // is the whole reason for composing rather than copying.
    let before = app.editor().doc.set_rules().len();
    app.editor_mut().doc.begin("override");
    let at = text.find("  it\n}").expect("the theme body");
    app.editor_mut()
        .apply_intent(lilook_core::Intent::ReplaceRange {
            range: at..at,
            value: "  show: lq.set-tick(inset: 4pt)\n".into(),
        });
    app.editor_mut().doc.commit();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert_eq!(
        app.editor().doc.set_rules().len(),
        before + 1,
        "the override is an ordinary set rule the panel already edits"
    );

    // Rename takes the binding and the show rule together.
    assert!(app.editor_mut().rename_theme("house-style"));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let text = app.editor().text().to_string();
    assert!(text.contains("#let house-style = it => {"), "{text}");
    assert!(text.contains("#show: house-style"), "{text}");
    assert!(!text.contains("paper"), "the old name is gone: {text}");

    // Remove it, then undo everything back to the byte.
    app.editor_mut().set_theme(None);
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(app.editor().active_theme().is_none());

    while app.editor().doc.history_depth().0 > 0 {
        app.editor_mut().doc.undo();
    }
    assert_eq!(app.editor().text(), original, "themes did not fully undo");
}

/// A phone gets the same four parts in the same order, stacked.
///
/// Three side-by-side columns that each want 200 points leave nothing for the
/// figure on a narrow screen, so below a threshold the panels stop being panels
/// and become one scroll: tree, inspector, figure, source. The order is the same
/// one the desktop reads left-to-right then top-to-bottom.
///
/// What this asserts is that the narrow path *does the same things* -- both
/// layouts end in one shared tail, so a gesture, an edit and a compile request
/// cannot drift between them.
#[test]
fn a_narrow_window_stacks_the_same_four_parts() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let wide_text = app.editor().text().to_string();
    let wide_scenes = app.editor().scenes().len();

    // A phone-sized viewport through a real egui context.
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    let narrow = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(390.0, 780.0),
        )),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx.run_ui(narrow.clone(), |ui| app.frame(ui));
    }

    // The document is untouched by a change of layout, and the figure is still
    // there to be edited.
    assert_eq!(app.editor().text(), wide_text, "layout is not an edit");
    assert_eq!(
        app.editor().scenes().len(),
        wide_scenes,
        "the figure is still recovered on a narrow screen"
    );
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    // And a gesture still lands, which is what proves the narrow path runs the
    // same tail rather than merely drawing.
    let scene = &app.editor().scenes()[0];
    let figure = scene.figure;
    let (x, y) = (scene.transform.x, scene.transform.y);
    app.editor_mut().handle_canvas(vec![
        lilook_editor::CanvasEvent::Begin,
        lilook_editor::CanvasEvent::SetLimits {
            figure,
            x: (x.min - 1.0, x.max - 1.0),
            y: (y.min, y.max),
        },
        lilook_editor::CanvasEvent::Commit,
    ]);
    assert_ne!(app.editor().text(), wide_text, "the pan was written");
    app.editor_mut().doc.undo();
    assert_eq!(app.editor().text(), wide_text, "and undoes");
}

/// The four visual controls, each verified by compiling what it writes.
///
/// These are the levers that decide whether a figure looks considered: the colour
/// ramp on a field, the palette every series draws from, the width of the column
/// it will sit in, and the size of its type. Each writes plain Typst, so the test
/// that matters is that lilaq accepts it.
#[test]
fn the_visual_controls_write_typst_that_compiles() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "colormesh")
        .expect("the colormesh example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let original = app.editor().text().to_string();

    let mesh = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "colormesh")
        .map(|c| c.id)
        .expect("the colormesh");
    let diagram = app
        .editor()
        .doc
        .figures()
        .first()
        .map(|f| f.node)
        .expect("a diagram");

    // Every colour map lilook offers has to be a real one. A name that is not in
    // `color.map` compiles to "unknown field", which is exactly the failure a
    // picker is supposed to make impossible.
    for (map, _) in lilook_core::COLORMAPS {
        app.editor_mut().doc.begin("map");
        app.editor_mut()
            .apply_intent(lilook_core::Intent::SetNamedArg {
                node: mesh,
                param: "map".into(),
                value: format!("color.map.{map}"),
            });
        app.editor_mut().doc.commit();
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{map}: {:?}", errors(&app));
    }

    // And every palette. The named ones are strings; the explicit ones arrays,
    // and quoting an array is the mistake this asserts against.
    for (name, expr, _) in lilook_core::CYCLES {
        let value = match expr.starts_with('(') {
            true => expr.to_string(),
            false => format!("\"{expr}\""),
        };
        app.editor_mut().doc.begin("cycle");
        app.editor_mut()
            .apply_intent(lilook_core::Intent::SetNamedArg {
                node: diagram,
                param: "cycle".into(),
                value,
            });
        app.editor_mut().doc.commit();
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{name}: {:?}", errors(&app));
    }

    // Journal widths, and a type size that lands as a `#set text` rather than a
    // lilaq argument.
    for (label, width, _) in lilook_core::FIGURE_WIDTHS {
        app.editor_mut().doc.begin("width");
        app.editor_mut()
            .apply_intent(lilook_core::Intent::SetNamedArg {
                node: diagram,
                param: "width".into(),
                value: width.to_string(),
            });
        app.editor_mut().doc.commit();
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{label}: {:?}", errors(&app));
    }
    for (label, size, _) in lilook_core::FIGURE_TEXT_SIZES {
        app.editor_mut().set_text_size_for_test(size);
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        assert!(errors(&app).is_empty(), "{label}: {:?}", errors(&app));
        // Set twice must not stack two rules.
        assert_eq!(
            app.editor().text().matches("#set text(size:").count(),
            1,
            "{label} stacked a second rule"
        );
    }

    // All of it undoes.
    while app.editor().doc.history_depth().0 > 0 {
        app.editor_mut().doc.undo();
    }
    assert_eq!(
        app.editor().text(),
        original,
        "the visual controls did not undo"
    );
}

/// A series can be moved onto its own right-hand axis, and back.
///
/// lilaq's twin axis is a nesting -- `lq.yaxis(position: right, <series>)` -- so
/// lilook's job is a wrap and an unwrap of the call's own bytes. The part worth
/// asserting is the honest limitation: the series still reads correctly in the
/// tree and the inspector, and it stops being draggable, because lilook recovers
/// one transform per diagram and that is not the axis this series is drawn on.
#[test]
fn a_series_can_move_to_a_second_y_axis() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let original = app.editor().text().to_string();

    let series = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .map(|c| c.id)
        .expect("a series");
    assert!(
        app.editor().editable_series().contains(&series),
        "draggable to begin with"
    );

    assert!(app.editor_mut().set_secondary_axis(series, true));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(
        app.editor().text().contains("yaxis(position: right"),
        "{}",
        app.editor().text()
    );

    // Still a series of the diagram, still with its data.
    let editor = app.editor();
    let moved = editor
        .doc
        .calls()
        .iter()
        .find(|c| c.is_xy_series())
        .expect("still a series");
    assert!(editor.doc.on_secondary_axis(moved.id));
    assert!(
        editor.doc.figures()[0].series.contains(&moved.id),
        "the diagram still owns it"
    );
    assert!(
        editor
            .scenes()
            .iter()
            .flat_map(|s| &s.series)
            .any(|g| g.node == moved.id && !g.points.is_empty()),
        "its data is still recovered"
    );
    // And the honest part: no drag handle, because the transform is not its own.
    assert!(
        !editor.editable_series().contains(&moved.id),
        "a series on a second axis must not offer a drag it cannot honour"
    );

    // Back again, byte for byte.
    let moved_id = moved.id;
    assert!(app.editor_mut().set_secondary_axis(moved_id, false));
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert_eq!(
        app.editor().text(),
        original,
        "unwrapping restores the source"
    );
}

/// Inlay hints: what the compiler resolved, where the user left it unsaid.
///
/// The capability that exists only because of the probe. A language server can
/// say what a name refers to; it cannot say what `xlim: auto` became, because
/// that is not in the text — it is in the rendered figure.
#[test]
fn hints_report_what_auto_resolved_to() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let diagram = app.editor().doc.figures()[0].node;
    app.editor_mut().doc.begin("auto limits");
    for param in ["xlim", "ylim"] {
        app.editor_mut()
            .apply_intent(lilook_core::Intent::InsertNamedArg {
                node: diagram,
                param: param.into(),
                value: "auto".into(),
            });
    }
    app.editor_mut().doc.commit();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let hints = app.editor().hints();
    let limits: Vec<&lilook_core::Hint> = hints
        .iter()
        .filter(|h| h.note.contains("lilaq chose"))
        .collect();
    assert_eq!(limits.len(), 2, "one per auto axis: {hints:?}");

    // The numbers are the recovered transform, not a guess.
    let scene = &app.editor().scenes()[0];
    let shown = limits[0].text.clone();
    let want = format!(
        "{} … {}",
        lilook_core::gesture_num(scene.transform.x.min),
        lilook_core::gesture_num(scene.transform.x.max)
    );
    assert_eq!(shown, want);

    // Every hint lands on a real byte boundary in the user's buffer.
    let text = app.editor().text().to_string();
    for h in &hints {
        assert!(h.at <= text.len() && text.is_char_boundary(h.at), "{h:?}");
    }

    // A limit the user wrote gets no echo: they can already see it.
    app.editor_mut().doc.begin("explicit");
    app.editor_mut()
        .apply_intent(lilook_core::Intent::SetNamedArg {
            node: diagram,
            param: "xlim".into(),
            value: "(0, 10)".into(),
        });
    app.editor_mut().doc.commit();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert_eq!(
        app.editor()
            .hints()
            .iter()
            .filter(|h| h.note.contains("lilaq chose"))
            .count(),
        1,
        "only the axis still on auto"
    );
}

/// Nothing has compiled, so there is nothing true to say.
#[test]
fn there_are_no_hints_without_a_scene() {
    let Some(mut app) = app() else { return };
    app.load(1);
    // No frames run: no compile, no scene.
    assert!(app.editor().hints().is_empty());
}

/// The whole error loop, through the real editor: broken figure → find the
/// cause → take the fix → it compiles.
///
/// The capability the language-server framing most obviously demanded. Every
/// diagnostic lilook showed used to be a dead end.
#[test]
fn a_broken_figure_can_be_diagnosed_and_fixed_from_the_ui() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    let healthy = app.editor().text().to_string();

    // Break it the way a user would: a plausible misspelling.
    let diagram = app.editor().doc.figures()[0].node;
    app.editor_mut().doc.begin("typo");
    app.editor_mut()
        .apply_intent(lilook_core::Intent::InsertNamedArg {
            node: diagram,
            param: "widht".into(),
            value: "8cm".into(),
        });
    app.editor_mut().doc.commit();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(!errors(&app).is_empty(), "the figure should be broken now");

    // lilaq raises it from inside itself, so there is nothing to point at.
    let d = app
        .editor()
        .diagnostics()
        .iter()
        .find(|d| d.severity == Severity::Error)
        .cloned()
        .expect("an error");
    assert!(d.range.is_none(), "this is the spanless case");

    // Ask, and the shell answers by recompiling variants.
    app.editor_mut().request_blame();
    run(&mut app, 2);
    let blames = app.editor().blames.clone();
    assert_eq!(blames.len(), 1, "one cause: {blames:?}");
    assert_eq!(blames[0].argument.as_deref(), Some("widht"));
    // The byte range the diagnostic never had.
    let text = app.editor().text().to_string();
    assert_eq!(&text[blames[0].range.clone()], "8cm");

    // The offer, and taking it.
    let actions = app.editor().actions(&blames);
    // This example already sets `width`, so a rename would collide -- there
    // cannot be two. Removal is the honest offer, and the note says why.
    let fix = actions
        .iter()
        .find(|a| a.label.contains("widht"))
        .cloned()
        .unwrap_or_else(|| panic!("nothing offered: {actions:?}"));
    assert!(fix.label.starts_with("remove"), "{}", fix.label);
    assert!(fix.note.contains("already set"), "{}", fix.note);
    app.editor_mut().apply_action(&fix);
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(
        errors(&app).is_empty(),
        "the fix left it broken: {:?}",
        errors(&app)
    );
    let fixed = app.editor().text().to_string();
    assert!(!fixed.contains("widht"), "the typo is gone: {fixed}");
    assert!(
        fixed.contains("width:"),
        "the real one is untouched: {fixed}"
    );

    // And it is one undoable step, back to a figure that also works.
    app.editor_mut().doc.undo();
    app.editor_mut().doc.undo();
    assert_eq!(app.editor().text(), healthy);
}

/// The completion popup opens at the caret and its offers are usable.
///
/// Driven through the real editor with real focus, because the popup is gated on
/// the source pane having it — a caret nobody is at should not open one.
#[test]
fn completion_offers_what_the_caret_can_take() {
    let Some(mut app) = app() else { return };
    app.load(1);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    // Position addressing is what the popup is built on: ask at a byte offset
    // inside the diagram's argument list.
    let text = app.editor().text().to_string();
    let at = text.find("xlabel").expect("the example labels its axes");
    let names: Vec<String> = app
        .editor()
        .completions(at - 1)
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(!names.is_empty(), "between arguments, names are offered");
    assert!(names.contains(&"title".to_string()), "{names:?}");

    // Every insertion is a value that compiles — the policy's safe seed. This is
    // the assertion that matters: accepting a completion must never leave a
    // figure that does not build.
    let diagram = app.editor().doc.figures()[0].node;
    for c in app.editor().completions(at - 1).into_iter().take(8) {
        let Some((param, value)) = c.insert.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue; // offered as a name to fill in, not a value
        }
        app.editor_mut().doc.begin("try");
        app.editor_mut()
            .apply_intent(lilook_core::Intent::InsertNamedArg {
                node: diagram,
                param: param.trim().into(),
                value: value.into(),
            });
        app.editor_mut().doc.commit();
        app.editor_mut().mark_dirty();
        run(&mut app, 3);
        // Either it compiles, or lilook can say what went wrong and offer the
        // repair. A completion is advisory and cannot know whether *this* data
        // suits the value -- `xscale: "log"` is a legitimate offer that a figure
        // starting at zero rejects -- but it must never leave a dead end.
        if !errors(&app).is_empty() {
            app.editor_mut().request_blame();
            run(&mut app, 2);
            let blames = app.editor().blames.clone();
            let offers = app.editor().actions(&blames);
            assert!(
                !offers.is_empty(),
                "accepting `{}` broke the figure with no way out: {:?}",
                c.insert,
                errors(&app)
            );
        }
        app.editor_mut().doc.undo();
        app.editor_mut().mark_dirty();
        run(&mut app, 2);
    }

    // And signature help names the call the caret is in.
    let sig = app.editor().signature(at - 1).expect("inside the diagram");
    assert_eq!(sig.name, "diagram");
    assert!(sig.params.contains(&"xlim".to_string()));
}

/// A colorbar can be given the diagram arguments it forwards.
///
/// `lq.colorbar` takes `..args` documented as "Additional arguments to pass to
/// @diagram", so `width` and `height` belong there as much as on the diagram —
/// and were invisible, because the schema lists the sink rather than what it
/// forwards to. Derived from the doc comment, so a lilaq release that forwards
/// somewhere new is followed without a change here.
#[test]
fn a_colorbar_offers_the_arguments_it_passes_through() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "colormesh")
        .expect("the colormesh example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let params = app.editor().schema.effective_params("lq.colorbar");
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"thickness"), "its own: {names:?}");
    assert!(names.contains(&"width"), "and the diagram's: {names:?}");
    assert!(names.contains(&"height"));

    // Not invented: a function that forwards nothing gains nothing.
    let plot: Vec<String> = app
        .editor()
        .schema
        .effective_params("lq.plot")
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert!(!plot.contains(&"width".to_string()), "{plot:?}");

    // And a forwarded argument actually works on a colorbar.
    let bar = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "colorbar")
        .map(|c| c.id)
        .expect("the colorbar");
    app.editor_mut().doc.begin("forwarded");
    app.editor_mut()
        .apply_intent(lilook_core::Intent::InsertNamedArg {
            node: bar,
            param: "height".into(),
            value: "3cm".into(),
        });
    app.editor_mut().doc.commit();
    app.editor_mut().mark_dirty();
    run(&mut app, 3);
    assert!(
        errors(&app).is_empty(),
        "forwarding height: {:?}",
        errors(&app)
    );
}

/// A colorbar can be clicked in the figure, not only found in the tree.
///
/// It is not a series and not a diagram — a frame beside the figure with no data
/// in it — so it is modelled as a scene of its own with `numeric: (false,
/// false)`. The canvas then needs nothing new: it already picks a scene by the
/// area it covers.
#[test]
fn a_colorbar_is_selectable_in_the_figure() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "colormesh")
        .expect("the colormesh example");
    app.load(index);
    run(&mut app, 3);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    let bar = app
        .editor()
        .doc
        .calls()
        .iter()
        .find(|c| c.short_name() == "colorbar")
        .map(|c| c.id)
        .expect("the colorbar");

    // It has a scene of its own now, with an area to click.
    let scene = app
        .editor()
        .scenes()
        .iter()
        .find(|s| s.figure == bar)
        .cloned()
        .unwrap_or_else(|| panic!("no scene for the colorbar"));
    assert!(
        scene.series.is_empty(),
        "a colorbar draws no data of its own"
    );
    assert_eq!(scene.numeric, (false, false), "and has no data transform");
    assert!(
        scene.area.2 > scene.area.0 && scene.area.3 > scene.area.1,
        "a real area to click: {:?}",
        scene.area
    );

    // It does not overlap the diagram it belongs to, or clicking one would
    // select the other.
    let diagram = app
        .editor()
        .scenes()
        .iter()
        .find(|s| s.figure != bar)
        .cloned()
        .expect("the diagram");
    assert!(
        scene.area.0 >= diagram.area.2 || scene.area.2 <= diagram.area.0,
        "colorbar {:?} overlaps diagram {:?}",
        scene.area,
        diagram.area
    );

    // A point inside it picks it, and a point inside the diagram does not.
    let mid = (
        (scene.area.0 + scene.area.2) / 2.0,
        (scene.area.1 + scene.area.3) / 2.0,
    );
    assert!(scene.contains_page_point(mid));
    assert!(!diagram.contains_page_point(mid));
}

/// The markers that locate it must not move anything.
///
/// The invariant the whole probe technique rests on: what the user sees is what
/// they would see without lilook. A colorbar is bracketed by two `metadata`
/// markers, and metadata lays out to nothing.
#[test]
fn bracketing_a_colorbar_does_not_move_the_figure() {
    let Some(mut app) = app() else { return };
    let index = EXAMPLES
        .iter()
        .position(|(name, _)| *name == "colormesh")
        .expect("the colormesh example");
    app.load(index);
    run(&mut app, 3);
    let with_probes = app.editor().scenes()[0].area;

    // The same document, compiled with no probes at all.
    let plain = lilook_core::Document::new(app.editor().text());
    assert!(
        plain.calls().iter().any(|c| c.short_name() == "colorbar"),
        "the fixture has a colorbar"
    );
    // The diagram's own frame is recovered from corner probes, so if bracketing
    // the colorbar had shifted the layout this area would have moved.
    assert!(
        with_probes.2 > with_probes.0 && with_probes.3 > with_probes.1,
        "the diagram still has its frame: {with_probes:?}"
    );
}
