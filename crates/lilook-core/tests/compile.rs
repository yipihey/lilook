use lilook_core::{CliCompiler, Transform};

#[test]
fn recovers_known_limits_and_hit_tests() {
    let typst = std::path::Path::new("/tmp/typst-x86_64-unknown-linux-musl/typst");
    if !typst.exists() {
        eprintln!("typst CLI absent; skipping");
        return;
    }
    let tmp = std::env::temp_dir();
    let c = CliCompiler::new(typst, &tmp);

    let build = |probes: &str| {
        format!(
            r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: 14cm, height: 10cm, margin: 10pt)
{}#lq.diagram(width: 6cm, height: 4cm, xlim: (-3, 7), ylim: (100, 400),
  lq.plot((-2, 0, 5), (150, 200, 380)){}
)
"#,
            lilook_core::compile::probe_preamble("lq"),
            probes
        )
    };

    let t: Transform = lilook_core::compile::recover_transform(&c, build).expect("transform");

    assert!((t.x.min - -3.0).abs() < 0.01, "xmin {}", t.x.min);
    assert!((t.x.max - 7.0).abs() < 0.01, "xmax {}", t.x.max);
    assert!((t.y.min - 100.0).abs() < 0.05, "ymin {}", t.y.min);
    assert!((t.y.max - 400.0).abs() < 0.05, "ymax {}", t.y.max);

    // round trip through the transform
    for p in [(-2.0, 150.0), (0.0, 200.0), (5.0, 380.0)] {
        let back = t.to_data(t.to_page(p));
        assert!((back.0 - p.0).abs() < 1e-6 && (back.1 - p.1).abs() < 1e-6);
    }

    // hit-testing in data space
    let series = vec![vec![(-2.0, 150.0), (0.0, 200.0), (5.0, 380.0)]];
    let near = t.to_page((0.0, 200.0));
    let hit = lilook_core::compile::hit_test(&t, &series, (near.0 + 2.0, near.1 + 2.0), 8.0)
        .expect("should hit");
    assert_eq!((hit.series, hit.index), (0, 1));

    let far = lilook_core::compile::hit_test(&t, &series, (near.0 + 500.0, near.1), 8.0);
    assert!(far.is_none(), "tolerance must be respected");
}
