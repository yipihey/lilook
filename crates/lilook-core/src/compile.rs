//! Compile service and geometric hit-testing.
//!
//! Phase 0 established that lilaq's rendered primitives carry spans pointing
//! into lilaq's own source, not the user's call site, and that exported SVG
//! carries no span data at all. So series identity does not come from spans.
//!
//! Instead we inject probes at known coordinates using lilaq's own `lq.place`,
//! read back where they landed via `typst query`, and recover the data<->page
//! transform. Hit-testing then happens in data space, against data lilook
//! already owns.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisMap {
    /// Page position (pt) of data value 0.
    pub origin: f64,
    /// Points per data unit. Negative on the y axis, which grows upward.
    pub scale: f64,
    pub min: f64,
    pub max: f64,
}

impl AxisMap {
    pub fn to_page(&self, data: f64) -> f64 {
        self.origin + data * self.scale
    }
    pub fn to_data(&self, page: f64) -> f64 {
        (page - self.origin) / self.scale
    }
    pub fn span(&self) -> f64 {
        self.max - self.min
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub x: AxisMap,
    pub y: AxisMap,
}

impl Transform {
    pub fn to_page(&self, p: (f64, f64)) -> (f64, f64) {
        (self.x.to_page(p.0), self.y.to_page(p.1))
    }
    pub fn to_data(&self, p: (f64, f64)) -> (f64, f64) {
        (self.x.to_data(p.0), self.y.to_data(p.1))
    }
}

/// A hit in data space: which series, which point, how far away in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub series: usize,
    pub index: usize,
    pub data: (f64, f64),
    pub distance_pt: f64,
}

#[derive(Debug, Deserialize)]
struct RawProbe {
    id: String,
    pos: RawPos,
}

#[derive(Debug, Deserialize)]
struct RawPos {
    x: String,
    y: String,
}

fn pt(s: &str) -> Result<f64, String> {
    s.strip_suffix("pt")
        .ok_or_else(|| format!("not a pt length: {s}"))?
        .parse()
        .map_err(|e| format!("bad length {s}: {e}"))
}

/// Anything that can run `typst query` over a source string.
pub trait Compiler {
    fn query(&self, source: &str, selector: &str) -> Result<String, String>;
}

/// Shells out to the typst CLI. Adequate for the CLI and for a first GUI --
/// Phase 0 measured a ~570 ms floor per invocation, so an in-process backend
/// with comemo memoisation replaces this before drag-rate interaction.
pub struct CliCompiler {
    pub program: PathBuf,
    pub root: PathBuf,
}

impl CliCompiler {
    pub fn new(program: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        CliCompiler {
            program: program.as_ref().to_path_buf(),
            root: root.as_ref().to_path_buf(),
        }
    }
}

impl Compiler for CliCompiler {
    fn query(&self, source: &str, selector: &str) -> Result<String, String> {
        let path = self
            .root
            .join(format!("._lilook_probe_{}.typ", std::process::id()));
        std::fs::write(&path, source).map_err(|e| e.to_string())?;
        let out = Command::new(&self.program)
            .arg("query")
            .arg(&path)
            .arg(selector)
            .arg("--field")
            .arg("value")
            .output()
            .map_err(|e| format!("running typst: {e}"))?;
        let _ = std::fs::remove_file(&path);
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

pub const PROBE_LABEL: &str = "<lilook-probe>";

/// The helper injected into the probe pass. Never written to the user's buffer.
pub fn probe_preamble(lq: &str) -> String {
    format!(
        "#let __lilook_probe(id, x, y) = {lq}.place(x, y, \
         context [#metadata((id: id, pos: here().position()))<lilook-probe>])\n"
    )
}

fn probe_args(x0: f64, x1: f64, y0: f64, y1: f64) -> String {
    format!(
        ", __lilook_probe(\"d0\", {x0}, {y0}), __lilook_probe(\"d1\", {x1}, {y1}), \
         __lilook_probe(\"r0\", 0%, 0%), __lilook_probe(\"r1\", 100%, 100%)"
    )
}

fn parse_probes(json: &str) -> Result<std::collections::HashMap<String, (f64, f64)>, String> {
    let raw: Vec<RawProbe> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut out = std::collections::HashMap::new();
    for p in raw {
        out.insert(p.id, (pt(&p.pos.x)?, pt(&p.pos.y)?));
    }
    for k in ["d0", "d1", "r0", "r1"] {
        if !out.contains_key(k) {
            return Err(format!("probe `{k}` missing from query result"));
        }
    }
    Ok(out)
}

fn solve(
    probes: &std::collections::HashMap<String, (f64, f64)>,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
) -> Result<Transform, String> {
    let (d0, d1) = (probes["d0"], probes["d1"]);
    let (r0, r1) = (probes["r0"], probes["r1"]);
    let sx = (d1.0 - d0.0) / (x1 - x0);
    let sy = (d1.1 - d0.1) / (y1 - y0);
    if sx == 0.0 || sy == 0.0 {
        return Err("degenerate probe separation".into());
    }
    // lq.place relative 0% is the top-left of the data area; page y grows
    // downward, so r0 carries ymax and r1 carries ymin.
    let xmin = x0 + (r0.0 - d0.0) / sx;
    let xmax = x0 + (r1.0 - d0.0) / sx;
    let ymax = y0 + (r0.1 - d0.1) / sy;
    let ymin = y0 + (r1.1 - d0.1) / sy;
    Ok(Transform {
        x: AxisMap {
            origin: d0.0 - x0 * sx,
            scale: sx,
            min: xmin,
            max: xmax,
        },
        y: AxisMap {
            origin: d0.1 - y0 * sy,
            scale: sy,
            min: ymin,
            max: ymax,
        },
    })
}

/// Recover the data<->page transform for one diagram.
///
/// Two passes: unit-separated probes give an approximate range, then probes at
/// 10%/90% of that range give a well-conditioned second solve. Single-pass
/// error scales with how small the probe separation is relative to the data
/// range -- measured 2.16 units of error on a 300-unit axis in one pass,
/// 0.007 after the refinement.
pub fn recover_transform(
    compiler: &dyn Compiler,
    build_source: impl Fn(&str) -> String,
) -> Result<Transform, String> {
    let first = compiler.query(&build_source(&probe_args(0.0, 1.0, 0.0, 1.0)), PROBE_LABEL)?;
    let t1 = solve(&parse_probes(&first)?, 0.0, 1.0, 0.0, 1.0)?;

    let (xs, ys) = (t1.x.span(), t1.y.span());
    let (x0, x1) = (t1.x.min + 0.1 * xs, t1.x.min + 0.9 * xs);
    let (y0, y1) = (t1.y.min + 0.1 * ys, t1.y.min + 0.9 * ys);

    let second = compiler.query(&build_source(&probe_args(x0, x1, y0, y1)), PROBE_LABEL)?;
    solve(&parse_probes(&second)?, x0, x1, y0, y1)
}

/// Nearest point across a set of series, with the tolerance expressed in page
/// points so it behaves the same at any zoom.
pub fn hit_test(
    transform: &Transform,
    series: &[Vec<(f64, f64)>],
    page_point: (f64, f64),
    tolerance_pt: f64,
) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (si, points) in series.iter().enumerate() {
        for (i, &p) in points.iter().enumerate() {
            let q = transform.to_page(p);
            let d = ((q.0 - page_point.0).powi(2) + (q.1 - page_point.1).powi(2)).sqrt();
            if d <= tolerance_pt && best.as_ref().is_none_or(|b| d < b.distance_pt) {
                best = Some(Hit {
                    series: si,
                    index: i,
                    data: p,
                    distance_pt: d,
                });
            }
        }
    }
    best
}
