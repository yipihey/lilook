//! Probe injection and scene recovery.
//!
//! lilaq builds its drawing primitives inside its own functions, so a span on a
//! rendered curve resolves into lilaq's source rather than the user's call site
//! (ADR-0008). Instead we inject markers into a *derived* copy of the buffer --
//! never the user's text -- and read back where they landed.
//!
//! Two kinds of marker, and both are injected as extra arguments to the diagram
//! itself. That placement is what makes the series probes work at all: the
//! argument expressions are then evaluated in exactly the scope they were
//! written in, so a series whose data comes from a local `#let` or a captured
//! variable still resolves.
//!
//! - **Corner probes** at `0%`/`100%` give the data area's page rectangle.
//! - **Scale probes** at two known data coordinates give points per data unit.
//! - **Series probes** re-evaluate each series' positional arguments into
//!   `metadata`, which is how lilook gets the points of a series whose data was
//!   computed. Measured cost on an already-compiled figure: 3.3 ms at 1k
//!   points, 6.3 ms at 5k -- comemo makes the second evaluation nearly free.

use std::collections::HashMap;

use lilook_core::compile::{AxisMap, Transform};
use lilook_core::scene::{Bounds, Scene, SeriesGeom};
use lilook_core::{CallSite, Document};
use typst::foundations::{Label, Selector, Value};
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

pub const PROBE_LABEL: &str = "lilook-probe";
pub const SERIES_LABEL: &str = "lilook-series";

/// Where the scale probes sit inside the data range. Not 0 and 1: a probe on
/// the very edge of the axis limits is the case most likely to fall outside
/// them once lilaq applies its own padding, and a probe outside the limits
/// displaces the layout origin by thousands of points.
const SCALE_PROBE_T: (f64, f64) = (0.1, 0.9);

/// The bounds used for a figure whose data lilook has not seen yet. Any value
/// works for recovering the *scale*; this one keeps the first pass's probes
/// near the origin, where lilaq's default limits are.
const FALLBACK_BOUNDS: Bounds = Bounds {
    x: (0.0, 1.0),
    y: (0.0, 1.0),
};

/// What was injected, so the recovered values can be interpreted.
#[derive(Debug, Clone)]
pub struct Injection {
    /// diagram call site -> the data coordinates its scale probes were given.
    pub scale_probes: HashMap<usize, (f64, f64, f64, f64)>,
}

/// Build the derived source. `hints` carries the data bounds recovered for each
/// figure last time round, which is what lets the steady state be one compile
/// rather than two.
pub fn inject(doc: &Document, hints: &HashMap<usize, Bounds>) -> (String, Injection) {
    let mut out = doc.text().to_string();
    let mut scale_probes = HashMap::new();

    // Back to front, so byte offsets stay valid as we splice.
    let mut figures = doc.figures();
    figures.sort_by_key(|f| std::cmp::Reverse(doc.call(f.node).map(|c| c.range.start)));

    for fig in &figures {
        let Some(diagram) = doc.call(fig.node) else {
            continue;
        };
        if diagram.generated {
            // Not the user's call: editing it is refused, and injecting into a
            // loop body would duplicate the probes anyway.
            continue;
        }
        let lq = diagram.module().unwrap_or("lq");
        let b = hints
            .get(&fig.node)
            .copied()
            .unwrap_or(FALLBACK_BOUNDS)
            .padded();
        let (x0, y0) = b.lerp((SCALE_PROBE_T.0, SCALE_PROBE_T.0));
        let (x1, y1) = b.lerp((SCALE_PROBE_T.1, SCALE_PROBE_T.1));
        scale_probes.insert(fig.node, (x0, x1, y0, y1));

        let mut args = String::new();
        for (kind, x, y) in [
            ("r0", "0%".to_string(), "0%".to_string()),
            ("r1", "100%".to_string(), "100%".to_string()),
            ("d0", fmt(x0), fmt(y0)),
            ("d1", fmt(x1), fmt(y1)),
        ] {
            args.push_str(&format!(
                ", {lq}.place({x}, {y}, context [#metadata((fig: {}, k: \"{kind}\", \
                 pos: here().position()))<{PROBE_LABEL}>])",
                fig.node
            ));
        }
        for &node in &fig.series {
            let Some(series) = doc.call(node) else {
                continue;
            };
            if series.generated {
                continue;
            }
            let (Some(x), Some(y)) = (positional(doc, series, 0), positional(doc, series, 1))
            else {
                continue;
            };
            // Relative coordinates, so this marker can never widen the data
            // range and change the layout it is trying to measure.
            args.push_str(&format!(
                ", {lq}.place(0%, 0%, [#metadata((fig: {}, node: {node}, x: {x}, y: {y}))\
                 <{SERIES_LABEL}>])",
                fig.node
            ));
        }

        if let Some(at) = insertion_point(diagram, &out) {
            out.insert_str(at, &args);
        }
    }

    (out, Injection { scale_probes })
}

fn fmt(v: f64) -> String {
    // Typst has no `1e-3` literal for floats in argument position in all
    // contexts, and `NaN`/`inf` are not literals at all.
    if !v.is_finite() {
        return "0".into();
    }
    format!("{v:.6}")
}

fn positional(_doc: &Document, call: &CallSite, i: usize) -> Option<String> {
    Some(call.positional.get(i)?.text.clone())
}

/// Insert after the last existing argument, never before the closing paren: a
/// call with a trailing comma and its paren on its own line otherwise yields
/// `,\n, ...)`, which does not parse.
fn insertion_point(call: &CallSite, _text: &str) -> Option<usize> {
    call.named
        .iter()
        .map(|a| a.value.end)
        .chain(call.positional.iter().map(|p| p.range.end))
        .max()
}

// ------------------------------------------------------------------ recovery

#[derive(Debug, Default)]
struct Raw {
    /// (figure, kind) -> (page, x pt, y pt)
    probes: HashMap<(usize, String), (usize, f64, f64)>,
    /// figure -> [(series node, points)]
    series: HashMap<usize, Vec<SeriesGeom>>,
}

fn label(name: &str) -> Selector {
    Selector::Label(Label::new(PicoStr::intern(name)).expect("non-empty label"))
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn as_pt(v: &Value) -> Option<f64> {
    match v {
        Value::Length(l) => Some(l.abs.to_pt()),
        _ => None,
    }
}

fn points(x: &Value, y: &Value) -> Vec<(f64, f64)> {
    let (Value::Array(x), Value::Array(y)) = (x, y) else {
        return vec![];
    };
    x.iter()
        .zip(y.iter())
        .filter_map(|(a, b)| Some((as_f64(a)?, as_f64(b)?)))
        .collect()
}

fn read(doc: &PagedDocument) -> Raw {
    use typst::introspection::Introspector as _;
    let mut raw = Raw::default();

    // `Dict::at` hands back an owned `Value`, so everything here is by value.
    let field = |d: &typst::foundations::Dict, k: &str| d.at(k.into(), None).ok();

    for c in doc.introspector().query(&label(PROBE_LABEL)) {
        let Ok(Value::Dict(d)) = c.field_by_name("value") else {
            continue;
        };
        let (Some(fig), Some(Value::Str(kind)), Some(Value::Dict(pos))) =
            (field(&d, "fig"), field(&d, "k"), field(&d, "pos"))
        else {
            continue;
        };
        let (Some(fig), Some(page), Some(x), Some(y)) = (
            as_f64(&fig),
            field(&pos, "page").as_ref().and_then(as_f64),
            field(&pos, "x").as_ref().and_then(as_pt),
            field(&pos, "y").as_ref().and_then(as_pt),
        ) else {
            continue;
        };
        raw.probes.insert(
            (fig as usize, kind.to_string()),
            // `here().position()` numbers pages from 1.
            ((page as usize).saturating_sub(1), x, y),
        );
    }

    for c in doc.introspector().query(&label(SERIES_LABEL)) {
        let Ok(Value::Dict(d)) = c.field_by_name("value") else {
            continue;
        };
        let (Some(fig), Some(node), Some(x), Some(y)) = (
            field(&d, "fig"),
            field(&d, "node"),
            field(&d, "x"),
            field(&d, "y"),
        ) else {
            continue;
        };
        let (Some(fig), Some(node)) = (as_f64(&fig), as_f64(&node)) else {
            continue;
        };
        raw.series
            .entry(fig as usize)
            .or_default()
            .push(SeriesGeom {
                node: node as usize,
                points: points(&x, &y),
            });
    }

    raw
}

/// Solve one figure's transform from its four probes.
///
/// `lq.place` relative `0%` is the top-left of the data area, and page y grows
/// downward, so `r0` carries ymax and `r1` carries ymin. Getting that backwards
/// produces a plausible-looking but inverted transform.
fn solve(
    corners: ((f64, f64), (f64, f64)),
    scale: ((f64, f64), (f64, f64)),
    data: (f64, f64, f64, f64),
) -> Option<Transform> {
    let (r0, r1) = corners;
    let (d0, d1) = scale;
    let (x0, x1, y0, y1) = data;
    let sx = (d1.0 - d0.0) / (x1 - x0);
    let sy = (d1.1 - d0.1) / (y1 - y0);
    if !sx.is_finite() || !sy.is_finite() || sx == 0.0 || sy == 0.0 {
        return None;
    }
    Some(Transform {
        x: AxisMap {
            origin: d0.0 - x0 * sx,
            scale: sx,
            min: x0 + (r0.0 - d0.0) / sx,
            max: x0 + (r1.0 - d0.0) / sx,
        },
        y: AxisMap {
            origin: d0.1 - y0 * sy,
            scale: sy,
            min: y0 + (r1.1 - d0.1) / sy,
            max: y0 + (r0.1 - d0.1) / sy,
        },
    })
}

/// Turn a compiled document plus the injection record into scenes.
pub fn scenes(doc: &PagedDocument, injection: &Injection) -> Vec<Scene> {
    let raw = read(doc);
    let mut out = vec![];
    for (&figure, &data) in &injection.scale_probes {
        let get = |k: &str| raw.probes.get(&(figure, k.to_string())).copied();
        let (Some(r0), Some(r1), Some(d0), Some(d1)) = (get("r0"), get("r1"), get("d0"), get("d1"))
        else {
            continue;
        };
        let Some(transform) = solve(
            ((r0.1, r0.2), (r1.1, r1.2)),
            ((d0.1, d0.2), (d1.1, d1.2)),
            data,
        ) else {
            continue;
        };
        let mut series = raw.series.get(&figure).cloned().unwrap_or_default();
        series.sort_by_key(|s| s.node);
        out.push(Scene {
            figure,
            page: r0.0,
            area: (
                r0.1.min(r1.1),
                r0.2.min(r1.2),
                r0.1.max(r1.1),
                r0.2.max(r1.2),
            ),
            transform,
            series,
        });
    }
    out.sort_by_key(|s| s.figure);
    out
}

/// Whether the probes used for this scene sat inside the limits they recovered.
///
/// When they did not, the *scale* is still right but the origin is displaced,
/// so the scene has to be recompiled with probes derived from what was learnt.
/// This is the check that decides between one compile and two.
pub fn probes_were_in_range(scene: &Scene, injection: &Injection) -> bool {
    let Some(&(x0, x1, y0, y1)) = injection.scale_probes.get(&scene.figure) else {
        return false;
    };
    let inside = |v: f64, a: f64, b: f64| {
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        v >= lo && v <= hi
    };
    inside(x0, scene.transform.x.min, scene.transform.x.max)
        && inside(x1, scene.transform.x.min, scene.transform.x.max)
        && inside(y0, scene.transform.y.min, scene.transform.y.max)
        && inside(y1, scene.transform.y.min, scene.transform.y.max)
}
