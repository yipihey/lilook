//! Compiling with no file system at all.
//!
//! This is the WASM path, exercised natively: the packages come from a bundle
//! built in memory, and nothing during the compile touches disk. It is also the
//! seam a host application uses to back figures with its own storage
//! (`docs/plan.md` §5) -- the same abstraction, built once.

use lilook_compile::{files, Backend, MemoryFiles};
use lilook_core::Document;

/// lilaq 0.6.0 and everything it imports. If lilaq gains a dependency this list
/// goes stale, and the test says so: the compile fails naming the package it
/// could not find.
const PACKAGES: &[(&str, &str)] = &[
    ("lilaq", "0.6.0"),
    ("elembic", "1.1.1"),
    ("zero", "0.6.1"),
    ("tiptoe", "0.4.0"),
];

fn bundle() -> Option<MemoryFiles> {
    let cache = typst_kit::packages::FsPackages::system_cache()?;
    let mut out = MemoryFiles::new();
    for (name, version) in PACKAGES {
        let dir = cache.path().join("preview").join(name).join(version);
        if !dir.exists() {
            eprintln!("{name}:{version} not in the package cache; skipping");
            return None;
        }
        let prefix = format!("package/preview/{name}/{version}");
        files::bundle_from_dir(&dir, &prefix, &mut out).ok()?;
    }
    Some(out)
}

#[test]
fn a_bundled_world_compiles_a_lilaq_figure_without_a_file_system() {
    let Some(bundle) = bundle() else { return };
    // Small enough to ship in a wasm binary; large enough to be worth checking.
    assert!(bundle.len() > 50, "{} files bundled", bundle.len());

    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 5cm, height: 3cm, lq.plot((0, 1, 2), (0, 1, 4)))
"#;
    let mut b = Backend::with_loader(bundle, "");
    let render = b.render(src, 1.0);
    assert!(
        !render.failed(),
        "bundled compile failed: {:?}",
        render.errors().collect::<Vec<_>>()
    );
    assert_eq!(render.pages.len(), 1);
    assert!(render.pages[0].image.width > 100);
}

/// Scene recovery is the part that would be easy to break on a different
/// loader, because it compiles a *derived* buffer. It does not care.
#[test]
fn scenes_are_recovered_from_a_bundled_world_too() {
    let Some(bundle) = bundle() else { return };
    let doc = Document::new(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#lq.diagram(width: 5cm, height: 3cm, xlim: (0, 4), lq.plot((0, 2, 4), (1, 3, 2)))
"#,
    );
    let mut b = Backend::with_loader(bundle, "");
    let mut hints = lilook_compile::backend::Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    assert!(!render.failed(), "{:?}", render.diagnostics);
    assert_eq!(scenes.len(), 1);
    assert_eq!(
        scenes[0].series[0].points,
        vec![(0.0, 1.0), (2.0, 3.0), (4.0, 2.0)]
    );
    assert!((scenes[0].transform.x.max - 4.0).abs() < 0.01);
}

/// A file the bundle does not contain must fail with a diagnostic that names
/// it, not with a panic or a blank page.
#[test]
fn a_missing_file_is_a_diagnostic() {
    let Some(bundle) = bundle() else { return };
    let mut b = Backend::with_loader(bundle, "");
    let render = b.render("#let data = csv(\"nope.csv\")\n#data.len()", 1.0);
    assert!(render.failed());
    let messages: Vec<String> = render.errors().map(|d| d.message.clone()).collect();
    assert!(
        messages.iter().any(|m| m.contains("nope.csv")),
        "{messages:?}"
    );
}
