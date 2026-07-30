//! What files did that compile read?
//!
//! This is the whole mechanism behind linked datasets: typst's file store
//! already tracks it, so lilook does not need to parse the document looking for
//! `csv(..)` calls -- which would miss a path built by an expression, and would
//! be wrong about anything conditional.

use lilook_compile::Backend;
use lilook_core::{Document, FileRoot};

fn skip(r: &lilook_compile::Render) -> bool {
    let missing = r
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"));
    if missing {
        eprintln!("lilaq package unavailable; skipping");
    }
    missing
}

/// A scratch project directory, since these tests are about files on disk.
fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lilook-deps-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    for (path, text) in files {
        std::fs::write(dir.join(path), text).expect("fixture");
    }
    dir
}

const ROWS: &str = "t,y\n0,0\n1,1\n2,4\n3,9\n";

fn figure() -> String {
    r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let rows = csv("run.csv", row-type: dictionary)
#let t = rows.map(r => float(r.t))
#let y = rows.map(r => float(r.y))
#lq.diagram(width: 6cm, height: 4cm, lq.plot(t, y, stroke: red))
"#
    .to_string()
}

/// The filter that makes the list usable: lilaq and its dependencies contribute
/// dozens of `.typ` files to every compile, and the user's one CSV has to be
/// findable among them.
#[test]
fn the_users_data_files_are_separable_from_a_packages_sources() {
    let dir = project("found", &[("run.csv", ROWS)]);
    let mut b = Backend::new(&dir, "");
    let r = b.render(&figure(), 1.0);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    let deps = b.dependencies();
    let data: Vec<&str> = deps
        .iter()
        .filter(|d| d.is_data())
        .map(|d| d.path.as_str())
        .collect();
    assert_eq!(data, ["run.csv"], "among {} dependencies", deps.len());

    // The unfiltered list is dominated by the package, which is the reason the
    // filter exists rather than a detail of it.
    assert!(
        deps.iter().filter(|d| !d.is_data()).count() > 20,
        "expected a package's worth of sources: {deps:?}"
    );
    assert!(deps
        .iter()
        .any(|d| matches!(&d.root, FileRoot::Package(p) if p == "preview/lilaq/0.6.0")));

    // Main is excluded: it is the buffer, not a file the figure read.
    assert!(!deps.iter().any(|d| d.path.contains("<lilook>")));

    let csv = deps.iter().find(|d| d.is_data()).unwrap();
    assert!(csv.loaded);
    assert_eq!(csv.extension().as_deref(), Some("csv"));
}

/// A file the figure asked for and did not get is the case the panel exists to
/// explain, so it has to appear -- and typst's store records failed loads, which
/// is what makes that possible at all.
#[test]
fn a_missing_data_file_is_still_reported() {
    let dir = project("missing", &[]);
    let mut b = Backend::new(&dir, "");
    let r = b.render(&figure(), 1.0);
    if skip(&r) {
        return;
    }
    assert!(r.failed(), "a missing csv should fail the compile");

    let deps = b.dependencies();
    let csv = deps
        .iter()
        .find(|d| d.path == "run.csv")
        .expect("the file that was not there is still a dependency");
    assert!(!csv.loaded, "it did not load, and must not claim to have");
}

/// Dropping a file in makes the next compile see it. This is refresh: the store
/// is reset before every compile, so correctness here is free and only the
/// *trigger* is lilook's problem.
#[test]
fn a_file_that_appears_is_picked_up_by_the_next_compile() {
    let dir = project("appears", &[]);
    let mut b = Backend::new(&dir, "");
    let r = b.render(&figure(), 1.0);
    if skip(&r) {
        return;
    }
    assert!(r.failed());

    std::fs::write(dir.join("run.csv"), ROWS).unwrap();
    let mut hints = lilook_compile::backend::Hints::new();
    let doc = Document::new(figure());
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());
    assert!(b
        .dependencies()
        .iter()
        .any(|d| d.path == "run.csv" && d.loaded));

    // And the data came back through the probe with no new machinery: a linked
    // dataset is a series like any other.
    assert_eq!(scenes[0].series[0].points.len(), 4);
    assert_eq!(scenes[0].series[0].points[3], (3.0, 9.0));
}

/// Changed contents, same path: the figure follows the file.
#[test]
fn changed_contents_change_the_figure() {
    let dir = project("changed", &[("run.csv", ROWS)]);
    let mut b = Backend::new(&dir, "");
    let mut hints = lilook_compile::backend::Hints::new();
    let doc = Document::new(figure());
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert_eq!(scenes[0].series[0].points.len(), 4);

    std::fs::write(dir.join("run.csv"), "t,y\n0,0\n1,2\n").unwrap();
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());
    assert_eq!(scenes[0].series[0].points.len(), 2);
    assert_eq!(scenes[0].series[0].points[1], (1.0, 2.0));

    // The document never changed. That is the property the whole design rests
    // on: refreshing a linked dataset is a recompile, not an edit, so there is
    // nothing for undo to have to know about.
    assert_eq!(doc.text(), figure());
    assert_eq!(doc.history_depth(), (0, 0));
}

/// Asking the compiler about a file, before the document mentions it.
///
/// This is how the link flow learns what columns a file has. It cannot go
/// through `probe.rs`: linking a file to a document with no diagram yet is the
/// first-run case, and there would be nothing to inject into.
#[test]
fn a_query_reads_a_file_the_document_never_mentions() {
    let dir = project("query", &[("run.csv", ROWS)]);
    let mut b = Backend::new(&dir, "");

    // Headers. Note the document under test is empty -- the query stands alone.
    let (answer, diags) = b.query(&lilook_compile::query::header_expr("run.csv"));
    if diags.iter().any(|d| d.message.contains("package")) {
        return;
    }
    assert_eq!(
        answer,
        Some(lilook_compile::Answer::Strings(vec![
            "t".into(),
            "y".into()
        ])),
        "{diags:?}"
    );

    let (answer, _) = b.query(&lilook_compile::query::row_count_expr("run.csv"));
    // Four data rows plus the header.
    assert_eq!(answer, Some(lilook_compile::Answer::Int(5)));

    // A column of numbers comes back as numbers, which is what makes the same
    // mechanism able to read a CBOR sidecar later.
    let (answer, _) = b.query(r#"csv("run.csv", row-type: dictionary).map(r => float(r.y))"#);
    assert_eq!(
        answer,
        Some(lilook_compile::Answer::Numbers(vec![0.0, 1.0, 4.0, 9.0]))
    );

    // A file that is not there fails as a query rather than as a panic.
    let (answer, diags) = b.query(&lilook_compile::query::header_expr("nope.csv"));
    assert_eq!(answer, None);
    assert!(!diags.is_empty(), "a failed query has to say why");
}

/// The reason a query gets its own file id: sharing the document's would rewrite
/// its cached source, and the next edit would recompile from cold.
#[test]
fn a_query_does_not_cost_the_next_compile_its_warm_path() {
    use std::time::Instant;
    let dir = project("query-warm", &[("run.csv", ROWS)]);
    let mut b = Backend::new(&dir, "");
    let big = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let x = lq.linspace(0, 10, num: 1000)
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot(x, x.map(t => calc.sin(t)), mark: none, stroke: {})
)
"#,
        "red"
    );
    let cold = Instant::now();
    let r = b.render(&big, 1.0);
    let cold = cold.elapsed();
    if skip(&r) {
        return;
    }
    assert!(!r.failed());

    // A warm recompile with no query in between, for the baseline.
    let t = Instant::now();
    assert!(!b.render(&big.replace("red", "blue"), 1.0).failed());
    let warm = t.elapsed();

    // And now the same edit, with a query in between.
    let (answer, _) = b.query(&lilook_compile::query::header_expr("run.csv"));
    assert!(answer.is_some());
    let t = Instant::now();
    assert!(!b.render(&big.replace("red", "green"), 1.0).failed());
    let after_query = t.elapsed();

    eprintln!("cold {cold:?}, warm {warm:?}, warm after a query {after_query:?}");
    // The assertion is about which band it lands in, not a ratio to `warm`: on a
    // shared runner the absolute numbers move, but cold and warm are far apart.
    assert!(
        after_query < cold / 2,
        "a query cost the next compile its warm path: {after_query:?} vs cold {cold:?}"
    );
}

/// Error-bar columns come back as channels.
///
/// This is what Veusz's ASCII descriptor needs: `+-` names a symmetric error
/// column, which lands on lilaq's `yerr:`. `SeriesGeom.points` has no room for
/// it, so without channels such a column would be linkable but invisible -- no
/// length to check against the file, no staleness, no unlock.
#[test]
fn an_error_bar_column_is_recovered_as_its_own_channel() {
    let dir = project(
        "channels",
        &[("run.csv", "t,y,dy\n0,1,0.1\n1,2,0.2\n2,4,0.4\n")],
    );
    let mut b = Backend::new(&dir, "");
    let mut hints = lilook_compile::backend::Hints::new();
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let run = csv("run.csv", row-type: dictionary)
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot(
    run.map(r => float(r.t)),
    run.map(r => float(r.y)),
    yerr: run.map(r => float(r.dy)),
  )
)
"#;
    let doc = Document::new(src);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    let series = &scenes[0].series[0];
    assert_eq!(series.points.len(), 3);
    assert_eq!(
        series.channel("yerr"),
        Some(vec![0.1, 0.2, 0.4]),
        "channels: {:?}",
        series.channel_lengths()
    );
    // x and y are channels too, so a UI can list every set uniformly.
    assert_eq!(series.channel("x"), Some(vec![0.0, 1.0, 2.0]));
    assert_eq!(series.channel("y"), Some(vec![1.0, 2.0, 4.0]));
    assert_eq!(series.channel("nope"), None);
    assert_eq!(
        series.channel_lengths(),
        vec![
            ("x".to_string(), 3),
            ("y".to_string(), 3),
            ("yerr".to_string(), 3)
        ]
    );

    // A style argument that happens to be an array is not a data channel.
    let styled = src.replace("yerr:", "dash: ");
    let doc = Document::new(styled);
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if !r.failed() {
        assert!(scenes[0].series[0].channels.is_empty());
    }
}

/// The other half of the transcode exit criterion: typst reads the sidecar, and
/// the figure gets the values the original file held.
///
/// `lilook-data` makes the sidecar and this compiles it, which is the seam that
/// matters -- a CBOR encoder that is subtly wrong would pass its own tests and
/// fail here.
#[test]
fn a_transcoded_sidecar_is_a_figure_typst_can_draw() {
    use lilook_data::{Column, Dataset};

    let t: Vec<f64> = (0..16).map(|i| i as f64 * 0.5).collect();
    let y: Vec<f64> = t.iter().map(|t| t.sin()).collect();
    let dy: Vec<f64> = y.iter().map(|v| v.abs() * 0.1).collect();
    let data = Dataset {
        columns: vec![
            Column::new("t", t.clone()),
            Column::new("flux (mJy)", y.clone()),
            Column::new("flux_err", dy.clone()),
        ],
    };

    let dir = project("sidecar", &[]);
    std::fs::create_dir_all(dir.join(".lilook")).unwrap();
    std::fs::write(dir.join(".lilook/run.cbor"), data.to_cbor()).unwrap();

    // Exactly what the editor writes for a keyed link: a lookup, and no
    // per-cell `float()`, because CBOR already holds numbers.
    let kind = lilook_core::SourceKind::of(".lilook/run.cbor");
    let cols = lilook_core::Columns {
        names: data.names(),
        has_header: true,
        grids: vec![],
    };
    let src = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let run = {}
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot({}, {}, yerr: {})
)
"#,
        lilook_core::binding_source(".lilook/run.cbor", kind, true),
        lilook_core::column_source("run", kind, &cols, 0).unwrap(),
        lilook_core::column_source("run", kind, &cols, 1).unwrap(),
        lilook_core::column_source("run", kind, &cols, 2).unwrap(),
    );
    // A name no field access can reach, so this also proves the `at` form.
    assert!(src.contains(r#"run.at("flux (mJy)")"#), "{src}");

    let mut b = Backend::new(&dir, "");
    let mut hints = lilook_compile::backend::Hints::new();
    let doc = Document::new(src.clone());
    let (r, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&r) {
        return;
    }
    assert!(!r.failed(), "{:?}", r.errors().collect::<Vec<_>>());

    // The values that come back are the ones the dataset held, not a rounding of
    // them -- CBOR carries doubles, so this is exact.
    let series = &scenes[0].series[0];
    assert_eq!(series.channel("x").unwrap(), t);
    assert_eq!(series.channel("y").unwrap(), y);
    assert_eq!(series.channel("yerr").unwrap(), dy);

    // And the sidecar is what the compile read, so it is watchable.
    assert!(b
        .dependencies()
        .iter()
        .any(|d| d.path == ".lilook/run.cbor" && d.loaded && d.is_data()));

    // Finally: the document is plain typst. If the real binary is on PATH, it
    // compiles there too, which is the promise the whole design rests on.
    if let Ok(out) = std::process::Command::new(std::env::var("TYPST").unwrap_or("typst".into()))
        .arg("compile")
        .arg("--root")
        .arg(&dir)
        .arg("-")
        .arg(dir.join("out.svg"))
        .arg("--format")
        .arg("svg")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write as _;
            c.stdin.as_mut().unwrap().write_all(src.as_bytes())?;
            c.wait_with_output()
        })
    {
        assert!(
            out.status.success(),
            "the typst binary could not compile it: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A FITS image, transcoded to a sidecar, read back as a colormesh's field.
///
/// This is the join between the two halves of 2-D linking: `lilook-data` writes
/// a nested CBOR array, and lilaq takes `z` as rows of values. Both halves have
/// their own tests; only compiling the result proves they agree, and "it decodes"
/// has been mistaken for "it compiles" here before.
#[test]
fn a_two_dimensional_sidecar_links_to_a_colormesh() {
    const COLS: usize = 5;
    const ROWS_N: usize = 3;
    let value = |col: usize, row: usize| (col + 10 * row) as f64;

    // The sidecar, written exactly as the transcode path writes one.
    let field: Vec<f64> = (0..ROWS_N)
        .flat_map(|r| (0..COLS).map(move |c| value(c, r)))
        .collect();
    let sidecar = lilook_data::cbor::map_of_arrays(&[lilook_data::Column::field(
        "image", field, COLS, ROWS_N,
    )]);

    let dir = project("field", &[]);
    std::fs::create_dir_all(dir.join(".lilook")).expect("sidecar dir");
    std::fs::write(dir.join(".lilook/run-image.cbor"), &sidecar).expect("sidecar");
    // Axes from the grid's own indices, which is what `commit_link` writes when
    // the file holds the field and nothing else.
    let src = format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let run = cbor(".lilook/run-image.cbor")
#lq.diagram(width: 6cm, height: 4cm,
  lq.colormesh(range({COLS}), range({ROWS_N}), run.image),
)
"#
    );

    let mut b = Backend::new(&dir, "");
    let doc = Document::new(&src);
    let mut hints = lilook_compile::backend::Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if skip(&render) {
        return;
    }
    assert!(!render.failed(), "{:?}", render.diagnostics);

    let g = &scenes[0].series[0];
    assert_eq!(g.grid, Some((COLS, ROWS_N)));
    let z = g.channel("z").expect("the field, back out of the sidecar");
    for row in 0..ROWS_N {
        for col in 0..COLS {
            assert_eq!(z[row * COLS + col], value(col, row), "({col},{row})");
        }
    }

    // And the sidecar is what the compile read, so changing the file refreshes
    // the figure -- the point of linking rather than embedding.
    let read: Vec<String> = b
        .dependencies()
        .into_iter()
        .filter(|f| f.root == FileRoot::Project)
        .map(|f| f.path)
        .collect();
    assert!(
        read.iter().any(|p| p.ends_with("run-image.cbor")),
        "{read:?}"
    );
}
