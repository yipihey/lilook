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
    /// Page position (pt) where the axis's own space reads zero -- data 0 on a
    /// linear axis, data 1 on a log one.
    pub origin: f64,
    /// Points per unit *of the axis's own space*: per data unit when linear, per
    /// decade when logarithmic. Negative on the y axis, which grows upward.
    pub scale: f64,
    pub min: f64,
    pub max: f64,
    /// Which space the axis is linear in. Recovered from the compile like
    /// everything else here -- see `probe.rs` -- rather than read out of the
    /// source, because the scale can come from the call, a set rule, or an
    /// `lq.axis` handed to `xaxis:`.
    pub kind: AxisScale,
}

/// How data maps to distance along an axis.
///
/// Getting this wrong is not a rounding error. Fitting a straight line through a
/// log axis gives the chord between the two probe points: every value between
/// them lands in the wrong place, hit-testing included, and extrapolating below
/// them produced a *negative* minimum on an axis where lilaq requires strictly
/// positive values -- which is what a pan then wrote into `ylim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisScale {
    #[default]
    Linear,
    /// Base 10. The base does not change the shape of the mapping, only the unit
    /// `scale` is measured in, so it need not be recovered.
    Log,
}

impl AxisMap {
    /// Data into the space the axis is linear in.
    pub fn fwd(&self, data: f64) -> f64 {
        match self.kind {
            AxisScale::Linear => data,
            AxisScale::Log => data.log10(),
        }
    }

    /// Back out of it. On a log axis this is `10^m`, which is *always* strictly
    /// positive -- so no value this produces can be one lilaq rejects.
    pub fn inv(&self, mapped: f64) -> f64 {
        match self.kind {
            AxisScale::Linear => mapped,
            AxisScale::Log => 10f64.powf(mapped),
        }
    }

    pub fn to_page(&self, data: f64) -> f64 {
        self.origin + self.fwd(data) * self.scale
    }

    pub fn to_data(&self, page: f64) -> f64 {
        self.inv((page - self.origin) / self.scale)
    }

    /// The limits after sliding the axis `page_delta` points.
    ///
    /// The shift happens in the axis's own space, which is what makes panning a
    /// log axis multiplicative rather than additive: limits scale instead of
    /// stepping, so they approach zero and never reach it. That is the guard --
    /// not a clamp bolted on afterwards, but the arithmetic being right.
    pub fn shifted(&self, page_delta: f64) -> (f64, f64) {
        let d = page_delta / self.scale;
        (
            self.inv(self.fwd(self.min) - d),
            self.inv(self.fwd(self.max) - d),
        )
    }

    /// One value moved by `page_delta` points along the axis, in its own space.
    pub fn nudged(&self, data: f64, page_delta: f64) -> f64 {
        self.inv(self.fwd(data) + page_delta / self.scale)
    }

    /// Extent in data units. Linear axes only: on a log axis the useful measure
    /// is the ratio, and `shifted`/`nudged` are what callers actually want.
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
            kind: AxisScale::Linear,
            scale: sx,
            min: xmin,
            max: xmax,
        },
        y: AxisMap {
            origin: d0.1 - y0 * sy,
            kind: AxisScale::Linear,
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

#[cfg(test)]
mod axis_tests {
    use super::*;

    fn log_axis() -> AxisMap {
        // A decade per 100 pt, y-style (negative scale, page grows downward),
        // spanning 1..1000.
        let mut m = AxisMap {
            origin: 0.0,
            scale: -100.0,
            min: 0.0,
            max: 0.0,
            kind: AxisScale::Log,
        };
        m.origin = 300.0; // page 300 is data 1
        m.min = m.to_data(300.0);
        m.max = m.to_data(0.0);
        m
    }

    #[test]
    fn a_log_axis_maps_through_its_own_space() {
        let m = log_axis();
        assert!((m.min - 1.0).abs() < 1e-9, "{}", m.min);
        assert!((m.max - 1000.0).abs() < 1e-6, "{}", m.max);
        // A decade is 100 pt, so the geometric middle sits halfway up.
        assert!((m.to_data(150.0) - 31.6227766).abs() < 1e-4);
        for d in [1.0, 3.0, 10.0, 316.0, 1000.0] {
            assert!((m.to_data(m.to_page(d)) - d).abs() < 1e-6, "{d}");
        }
    }

    /// The bug this exists for: panning a log axis used to subtract a *linear*
    /// delta from both limits, so a big enough drag produced a zero or negative
    /// limit and lilaq refused the figure -- "value must be strictly positive".
    ///
    /// Shifting in the axis's own space makes the limits scale instead of step.
    /// A ratio can approach zero without ever reaching it, so the guard is the
    /// arithmetic rather than a clamp bolted on afterwards.
    #[test]
    fn panning_a_log_axis_cannot_reach_zero() {
        let m = log_axis();
        // Twenty decades in either direction, far more than any real drag.
        for px in [
            -2000.0, -700.0, -101.0, -1.0, 0.0, 1.0, 101.0, 700.0, 2000.0,
        ] {
            let (lo, hi) = m.shifted(px);
            assert!(
                lo > 0.0 && hi > 0.0,
                "panning {px} pt gave ({lo}, {hi}); a log axis cannot hold that"
            );
            assert!(lo < hi, "panning {px} pt inverted the axis: ({lo}, {hi})");
            // The span is preserved as a *ratio*, which is what panning a log
            // axis means: the picture slides, it does not stretch.
            assert!(
                ((hi / lo) / (m.max / m.min) - 1.0).abs() < 1e-9,
                "panning {px} pt changed the ratio"
            );
        }
        // And it is a shift: one decade of pixels is one decade of data. Positive
        // here because page y grows downward while data grows upward, which is the
        // same direction the linear pan has always had.
        let (up, _) = m.shifted(100.0);
        assert!((up / m.min - 10.0).abs() < 1e-6, "{up}");
        let (down, _) = m.shifted(-100.0);
        assert!((down / m.min - 0.1).abs() < 1e-6, "{down}");
    }

    #[test]
    fn panning_a_linear_axis_is_unchanged() {
        let m = AxisMap {
            origin: 0.0,
            scale: 2.0,
            min: 0.0,
            max: 50.0,
            kind: AxisScale::Linear,
        };
        let (lo, hi) = m.shifted(10.0);
        assert!(
            (lo - -5.0).abs() < 1e-9 && (hi - 45.0).abs() < 1e-9,
            "{lo} {hi}"
        );
        // A linear axis may legitimately go negative; nothing here prevents that.
        assert!(m.shifted(1000.0).0 < 0.0);
    }

    #[test]
    fn nudging_a_point_follows_the_axis() {
        let m = log_axis();
        // Moving a point one decade up the page multiplies it by ten.
        assert!((m.nudged(10.0, -100.0) / 100.0 - 1.0).abs() < 1e-9);
        assert!(m.nudged(1e-6, -3000.0) > 0.0, "still positive");
        let lin = AxisMap {
            origin: 0.0,
            scale: 2.0,
            min: 0.0,
            max: 50.0,
            kind: AxisScale::Linear,
        };
        assert!((lin.nudged(10.0, 4.0) - 12.0).abs() < 1e-9);
    }
}
