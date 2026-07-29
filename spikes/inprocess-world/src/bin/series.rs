//! Second spike: can lilook recover *evaluated* series data for hit-testing
//! when the data is computed (linspace/map), not a literal array?
include!("../world.rs");

fn figure(n: usize, extra: &str) -> String {
    format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let x = lq.linspace(0, 10, num: {n})
#let y = x.map(t => calc.sin(t))
#lq.diagram(width: 6cm, height: 4cm, lq.plot(x, y, mark: none, stroke: red))
{extra}
"#
    )
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let mut world = SpikeWorld::new(std::env::current_dir().unwrap(), figure(n, ""));
    let t = Instant::now();
    world.compile().expect("base");
    println!("base compile ({n})            {:>8.1} ms", ms(t));

    // The series probe: re-evaluate the same expressions into metadata. comemo
    // should make the second evaluation of `x` cheap.
    world.edit(figure(n, "#metadata((id: 0, x: x, y: y))<lilook-series>"));
    let t = Instant::now();
    let doc = world.compile().expect("series probe");
    let label =
        typst::foundations::Label::new(typst::utils::PicoStr::intern("lilook-series")).unwrap();
    let hits = doc
        .introspector()
        .query(&typst::foundations::Selector::Label(label));
    let dt = ms(t);
    let mut count = 0usize;
    for h in &hits {
        if let Ok(typst::foundations::Value::Dict(d)) = h.field_by_name("value") {
            if let Ok(typst::foundations::Value::Array(a)) = d.at("x".into(), None) {
                count = a.len();
            }
        }
    }
    println!(
        "series probe + query         {dt:>8.1} ms  ({} series, {count} pts recovered)",
        hits.len()
    );

    // Marshalling cost alone, once compiled.
    let t = Instant::now();
    let mut sum = 0.0;
    for h in &hits {
        if let Ok(typst::foundations::Value::Dict(d)) = h.field_by_name("value") {
            for k in ["x", "y"] {
                if let Ok(typst::foundations::Value::Array(a)) = d.at(k.into(), None) {
                    for v in a.iter() {
                        match v {
                            typst::foundations::Value::Float(f) => sum += f,
                            typst::foundations::Value::Int(i) => sum += *i as f64,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    println!(
        "marshal to f64 vectors       {:>8.3} ms  (checksum {sum:.3})",
        ms(t)
    );
}
