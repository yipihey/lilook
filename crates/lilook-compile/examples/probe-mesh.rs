//! What does lilook recover from a colormesh? A measurement.
fn main() {
    let src = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)
#let xs = lq.linspace(0, 4, num: 5)
#let ys = lq.linspace(0, 3, num: 4)
#lq.diagram(
  width: 6cm, height: 4cm,
  lq.colormesh(xs, ys, (x, y) => x * y),
)
"#;
    let doc = lilook_core::Document::new(src);
    let mut b = lilook_compile::Backend::new(std::env::temp_dir(), "");
    let mut hints = lilook_compile::backend::Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    println!("compiles: {}", !render.failed());
    for d in render.errors() {
        println!("  error: {}", d.message);
    }
    for c in doc.calls() {
        println!(
            "  #{} {} series={} slots={}",
            c.id,
            c.callee,
            c.is_xy_series(),
            c.positional.len()
        );
    }
    for s in &scenes {
        println!("scene figure #{}", s.figure);
        for g in &s.series {
            println!(
                "  series #{}: {} points, channels {:?}",
                g.node,
                g.points.len(),
                g.channel_lengths()
            );
            println!("  points: {:?}", g.points);
        }
    }
}
