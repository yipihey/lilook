//! What does it cost to read data from a file at compile time?
//!
//! `csv()` yields *strings*, so a linked dataset spends one interpreted `float()`
//! call per cell -- and the series probe re-evaluates the same slot expression to
//! recover the values, so it is two per cell per compile. `cbor()` by contrast
//! returns native floats with no per-cell interpretation at all.
//!
//! **The answer is that it makes no measurable difference**, at 1k, 10k or 100k
//! rows: what costs is lilaq drawing N points, which it does identically whatever
//! produced them. So a CSV can be linked live at any size, and a transcoded
//! sidecar is justified by *capability* -- typst cannot read HDF5, npz or FITS at
//! all -- rather than by speed. The table is in `docs/findings.md`; this is here
//! so the claim can be re-checked rather than believed. Run with:
//!
//! ```sh
//! cargo run --release --example measure-data -p lilook-compile
//! ```
//!
//! The numbers land in `docs/findings.md`. Needs `@preview/lilaq:0.6.0` in the
//! typst package cache.

use std::time::{Duration, Instant};

use lilook_compile::{backend::Hints, Backend};
use lilook_core::Document;

/// Rows to try. 100k is past anything a figure should plot, and is here to show
/// where the wall is rather than to suggest anyone go there.
const SIZES: [usize; 3] = [1_000, 10_000, 100_000];

fn main() {
    let dir = std::env::temp_dir().join("lilook-measure-data");
    std::fs::create_dir_all(&dir).expect("scratch dir");

    println!(
        "{:>7}  {:<22} {:>9} {:>9} {:>9}",
        "rows", "shape", "cold", "warm", "bytes"
    );
    for n in SIZES {
        write_fixtures(&dir, n);
        let cases: [(&str, String, bool); 6] = [
            ("csv + float, probed", csv_doc(n), true),
            ("csv + float, no probe", csv_doc(n), false),
            ("csv inlined, probed", csv_inline_doc(n), true),
            ("csv inlined, no probe", csv_inline_doc(n), false),
            ("cbor, probed", cbor_doc(n), true),
            ("literal array, probed", literal_doc(n), true),
        ];
        for (name, source, probed) in cases {
            let bytes = source.len();
            match time(&dir, &source, probed) {
                Some((cold, warm)) => println!(
                    "{n:>7}  {name:<22} {:>7.1?} {:>7.1?} {:>8}",
                    cold, warm, bytes
                ),
                None => {
                    println!("{n:>7}  {name:<22} {:>9} {:>9} {:>8}", "failed", "-", bytes);
                }
            }
        }
    }
    println!("\nfixtures in {}", dir.display());
}

/// Cold compile, then the fastest of five warm recompiles.
///
/// A warm recompile changes only the stroke colour, which is what a style edit
/// does: the point of the number is what lilook pays *per interaction*, and if
/// the data survives in comemo's cache that cost is not the data's at all.
fn time(dir: &std::path::Path, source: &str, probed: bool) -> Option<(Duration, Duration)> {
    let mut b = Backend::new(dir, "");
    let mut hints = Hints::new();

    // comemo's cache is process-global, so without this each case inherits the
    // work the previous ones did -- which made the *fourth* case look like the
    // fastest way to read a file. Drop everything, so "cold" means cold.
    comemo::evict(0);

    let cold = Instant::now();
    let ok = compile(&mut b, source, probed, &mut hints);
    let cold = cold.elapsed();
    if !ok {
        return None;
    }

    let mut warm = Duration::MAX;
    for c in ["blue", "green", "orange", "purple", "teal"] {
        let edited = source.replace("stroke: red", &format!("stroke: {c}"));
        let t = Instant::now();
        let ok = compile(&mut b, &edited, probed, &mut hints);
        warm = warm.min(t.elapsed());
        if !ok {
            return None;
        }
    }
    Some((cold, warm))
}

/// One compile, with or without the probes that recover series data.
fn compile<L: typst_kit::files::FileLoader + Send + Sync>(
    b: &mut Backend<L>,
    source: &str,
    probed: bool,
    hints: &mut Hints,
) -> bool {
    if !probed {
        let r = b.render(source, 2.0);
        return report(&r);
    }
    let doc = Document::new(source);
    let (r, scenes) = b.render_scenes(&doc, 2.0, hints);
    if !report(&r) {
        return false;
    }
    // A probed compile that recovered nothing measured the wrong thing.
    if scenes
        .iter()
        .all(|s| s.series.iter().all(|x| x.points.is_empty()))
    {
        eprintln!("  probes recovered no points");
        return false;
    }
    true
}

fn report(r: &lilook_compile::Render) -> bool {
    if r.failed() {
        for d in r.errors().take(2) {
            eprintln!("  {}", d.message);
        }
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// The documents. Same figure, same data, six ways of getting at it.

fn preamble() -> &'static str {
    r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
"#
}

fn plot() -> &'static str {
    "#lq.diagram(width: 6cm, height: 4cm,\n  lq.plot(t, y, mark: none, stroke: red)\n)\n"
}

/// What a live CSV link looks like: `float()` per cell, twice per compile.
fn csv_doc(_n: usize) -> String {
    format!(
        "{}#let rows = csv(\"run.csv\", row-type: dictionary)\n\
         #let t = rows.map(r => float(r.t))\n\
         #let y = rows.map(r => float(r.y))\n{}",
        preamble(),
        plot()
    )
}

/// The same data, but with the conversion inlined into the series slot instead
/// of bound to a name. This is the shape lilook must *not* write: the probe
/// recovers series data by re-evaluating the slot's source text, so an inlined
/// `map` is converted twice per compile where a bound name is free.
fn csv_inline_doc(_n: usize) -> String {
    format!(
        "{}#let rows = csv(\"run.csv\", row-type: dictionary)\n\
         #lq.diagram(width: 6cm, height: 4cm,\n  \
         lq.plot(rows.map(r => float(r.t)), rows.map(r => float(r.y)), \
         mark: none, stroke: red)\n)\n",
        preamble()
    )
}

/// What a sidecar link looks like: native floats, no interpretation.
fn cbor_doc(_n: usize) -> String {
    format!(
        "{}#let d = cbor(\"run.cbor\")\n#let t = d.t\n#let y = d.y\n{}",
        preamble(),
        plot()
    )
}

/// What "unlocked" looks like: the values in the document.
fn literal_doc(n: usize) -> String {
    let t: Vec<String> = (0..n).map(|i| fmt(x_of(i, n))).collect();
    let y: Vec<String> = (0..n).map(|i| fmt(y_of(i, n))).collect();
    format!(
        "{}#let t = ({})\n#let y = ({})\n{}",
        preamble(),
        t.join(", "),
        y.join(", "),
        plot()
    )
}

fn x_of(i: usize, n: usize) -> f64 {
    10.0 * i as f64 / n as f64
}

fn y_of(i: usize, n: usize) -> f64 {
    x_of(i, n).sin()
}

/// The same emitter lilook writes data with, so the fixture and the document
/// under test cannot disagree about what a value is.
fn fmt(v: f64) -> String {
    lilook_core::data_num(v)
}

// ---------------------------------------------------------------------------

fn write_fixtures(dir: &std::path::Path, n: usize) {
    let mut csv = String::from("t,y\n");
    for i in 0..n {
        csv.push_str(&format!("{},{}\n", fmt(x_of(i, n)), fmt(y_of(i, n))));
    }
    std::fs::write(dir.join("run.csv"), &csv).expect("write csv");

    let t: Vec<f64> = (0..n).map(|i| x_of(i, n)).collect();
    let y: Vec<f64> = (0..n).map(|i| y_of(i, n)).collect();
    std::fs::write(dir.join("run.cbor"), cbor_columns(&[("t", &t), ("y", &y)]))
        .expect("write cbor");
}

/// A CBOR map of named `f64` arrays -- the sidecar format, hand-rolled.
///
/// Deliberately not a dependency: this is the whole encoder lilook needs, it is
/// the one shape typst reads back as native floats, and writing it here keeps
/// the measurement honest about what a sidecar costs. `lilook-data`'s
/// `cbor_out.rs` grows from this.
fn cbor_columns(columns: &[(&str, &[f64])]) -> Vec<u8> {
    let mut out = Vec::new();
    head(&mut out, 5, columns.len() as u64); // map
    for (name, values) in columns {
        head(&mut out, 3, name.len() as u64); // text string
        out.extend_from_slice(name.as_bytes());
        head(&mut out, 4, values.len() as u64); // array
        for v in *values {
            out.push(0xfb); // float64
            out.extend_from_slice(&v.to_bits().to_be_bytes());
        }
    }
    out
}

/// A CBOR item head: three bits of major type, then the argument in the
/// smallest of the five widths the format allows.
fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    match arg {
        0..=23 => out.push(m | arg as u8),
        24..=0xff => out.extend_from_slice(&[m | 24, arg as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}
