//! The in-process backend, against the real lilaq package.
//!
//! Needs `@preview/lilaq:0.6.0` in the typst package cache (or a network to
//! fetch it once); it skips itself rather than failing when neither is
//! available, the same way the CLI-backed test does.

use lilook_compile::{Backend, Severity};
use std::time::Instant;

fn figure(n: usize, stroke: &str) -> String {
    format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let x = lq.linspace(0, 10, num: {n})
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot(x, x.map(t => calc.sin(t)), mark: none, stroke: {stroke})
)
"#
    )
}

/// Package unavailable (offline, empty cache) reads as "skip", not "fail".
fn unavailable(r: &lilook_compile::Render) -> bool {
    r.errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"))
}

#[test]
fn compiles_and_rasterises_a_lilaq_figure_in_process() {
    let mut b = Backend::new(std::env::temp_dir(), "");

    let cold = Instant::now();
    let r = b.render(&figure(1000, "red"), 2.0);
    let cold = cold.elapsed();
    if unavailable(&r) {
        eprintln!("lilaq package unavailable; skipping");
        return;
    }
    assert!(
        !r.failed(),
        "compile failed: {:?}",
        r.errors().collect::<Vec<_>>()
    );

    let page = &r.pages[0];
    assert_eq!(page.index, 0);
    // 6cm x 4cm of diagram plus margins and labels, at 2 px/pt.
    assert!(page.image.width > 300 && page.image.width < 800, "{page:?}");
    assert_eq!(
        page.image.rgba.len(),
        (page.image.width * page.image.height * 4) as usize
    );
    assert!(
        page.size_pt.0 > 100.0 && page.size_pt.1 > 50.0,
        "{:?}",
        page.size_pt
    );

    // Warm path: a style edit must not re-do the work the figure already did.
    // Measured 20 ms on an M-series laptop; the threshold is loose enough for a
    // shared CI runner but tight enough to catch a lost comemo cache, which
    // would put this back at cold-compile cost.
    let mut warm = std::time::Duration::MAX;
    for c in ["blue", "green", "orange", "purple", "teal"] {
        let t = Instant::now();
        let r = b.render(&figure(1000, c), 2.0);
        warm = warm.min(t.elapsed());
        assert!(!r.failed());
    }
    assert!(
        warm < cold / 2,
        "warm recompile {warm:?} is not meaningfully faster than cold {cold:?}"
    );
    assert!(warm.as_millis() < 250, "warm recompile too slow: {warm:?}");
    eprintln!("cold {cold:?}, warm {warm:?}");
}

#[test]
fn reports_errors_with_a_byte_range_and_keeps_the_last_good_document() {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let good = b.render(&figure(50, "red"), 1.0);
    if unavailable(&good) {
        eprintln!("lilaq package unavailable; skipping");
        return;
    }
    assert!(!good.failed());

    let broken = figure(50, "red").replace("lq.plot", "lq.plott");
    let r = b.render(&broken, 1.0);
    assert!(r.failed(), "a bad callee must not compile");
    let e = r.errors().next().expect("an error diagnostic");
    assert_eq!(e.severity, Severity::Error);
    // The span lands on the unknown field rather than the whole call, which is
    // what an inline error marker wants anyway.
    let range = e.range.clone().expect("a range in the main buffer");
    assert_eq!(&broken[range], "plott");

    // The canvas keeps drawing while the buffer is transiently broken.
    assert!(b.document().is_some());
}

#[test]
fn a_never_saved_buffer_still_compiles() {
    // No path, no file on disk: the whole point of serving main from memory.
    let mut b = Backend::new(std::env::temp_dir(), "");
    let r = b.render("#set page(width: 4cm, height: 2cm)\nhello", 1.0);
    assert!(!r.failed(), "{:?}", r.diagnostics);
    assert_eq!(r.pages.len(), 1);
}
