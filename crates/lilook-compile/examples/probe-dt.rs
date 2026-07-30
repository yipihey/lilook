fn main() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#let days = (
  datetime(year: 2026, month: 1, day: 1),
  datetime(year: 2026, month: 2, day: 1),
  datetime(year: 2026, month: 3, day: 1),
)
#lq.diagram(width: 7cm, height: 4cm,
  lq.plot(days, (3, 5, 4)),
)
"#;
    let doc = lilook_core::Document::new(src);
    let mut b = lilook_compile::Backend::new(std::env::temp_dir(), "");
    // Plain compile first: does the user's own document work?
    let plain = b.render(src, 1.0);
    println!(
        "plain compile: {}",
        if plain.failed() { "FAILED" } else { "ok" }
    );
    for e in plain.errors() {
        println!("   {}", e.message);
    }

    let mut hints = lilook_compile::backend::Hints::new();
    let (probed, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    println!(
        "with lilook's probes: {}",
        if probed.failed() { "FAILED" } else { "ok" }
    );
    for e in probed.errors() {
        println!("   {}", e.message);
    }
    for s in &scenes {
        for g in &s.series {
            println!("   series #{} points={}", g.node, g.points.len());
        }
    }
}
