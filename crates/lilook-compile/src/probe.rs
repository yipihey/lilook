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

use lilook_core::compile::{AxisMap, AxisScale, Transform};
use lilook_core::scene::{Bounds, Scene, SeriesGeom};
use lilook_core::{CallSite, Document, SeriesShape};
use typst::foundations::{Label, Selector, Value};
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

pub const PROBE_LABEL: &str = "lilook-probe";
pub const SERIES_LABEL: &str = "lilook-series";

/// Named arguments that carry data rather than style, and so are worth
/// recovering as channels alongside x and y.
///
/// `y2` is `fill-between`'s second surface: `fill-between(x, y1, y2: ..)` fits the
/// paired-point contract for slots 0 and 1, and its upper edge arrives this way.
/// `width` and `base` are `bar`'s -- both take one value per bar, so a linked file
/// can drive them and their lengths are worth checking against the data's.
///
/// A list rather than a schema lookup: the schema says a parameter's *type*, and
/// several style parameters also take arrays. What matters here is whether the
/// values are measurements, and that is a fact about lilaq's vocabulary.
pub const DATA_ARGS: [&str; 7] = ["yerr", "xerr", "y2", "z", "size", "width", "base"];

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
    inject_with(doc, hints, true)
}

/// As [`inject`], but able to leave out the *scale* probes.
///
/// Those are the only markers placed at data coordinates, and on an axis that is
/// not numeric -- `datetime` -- lilaq maps a number like `0.1` onto a calendar and
/// the `auto`-sized page grows without bound. Without them the figure lays out
/// normally and still draws; what is lost is the data<->page transform, which was
/// never recoverable for that axis anyway.
pub fn inject_with(
    doc: &Document,
    hints: &HashMap<usize, Bounds>,
    with_scale: bool,
) -> (String, Injection) {
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
        let mut markers: Vec<(&str, String, String)> = vec![
            ("r0", "0%".into(), "0%".into()),
            ("r1", "100%".into(), "100%".into()),
        ];
        if with_scale {
            markers.extend([
                ("d0", fmt(x0), fmt(y0)),
                ("d1", fmt(x1), fmt(y1)),
                // The midpoint in *data* terms. On an axis that is linear in
                // data, this lands exactly halfway between `d0` and `d1` on the
                // page; on a log axis it does not, which is how the scale is
                // recovered without reading the source.
                ("dm", fmt((x0 + x1) / 2.0), fmt((y0 + y1) / 2.0)),
            ]);
        }
        for (kind, x, y) in markers {
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
            // A rules series has no pair to read: every positional argument is one
            // line's coordinate, so they are gathered into one array and sent on
            // the axis they belong to. The other axis stays empty -- the line
            // spans the frame, so there is nothing to recover there.
            // A distributions series carries one dataset per positional argument
            // and its position on the named axis -- `auto` by default, which lilaq
            // resolves to `1..n`. Resolving it here, in typst, is what makes the
            // commonest case (no `x:` at all) recoverable rather than unknown.
            let mut datasets = String::from("()");
            if let SeriesShape::Distributions(axis) = series.series_shape() {
                let n = series.positional.len();
                let slots = (0..n)
                    .filter_map(|i| positional(doc, series, i))
                    .collect::<Vec<_>>()
                    .join(", ");
                datasets = format!("({slots},)");
                let name = match axis {
                    lilook_core::Axis::X => "x",
                    lilook_core::Axis::Y => "y",
                };
                let given = series
                    .named
                    .iter()
                    .find(|a| a.name == name)
                    .map(|a| doc.text()[a.value.clone()].to_string())
                    .unwrap_or_else(|| "auto".into());
                let positions = format!(
                    "((p) => if type(p) == array {{ p }} \
                     else if p == auto {{ range(1, {}) }} else {{ (p,) }})({given})",
                    n + 1
                );
                let (x, y) = match axis {
                    lilook_core::Axis::X => (positions, "()".to_string()),
                    lilook_core::Axis::Y => ("()".to_string(), positions),
                };
                args.push_str(&format!(
                    ", {lq}.place(0%, 0%, [#metadata((fig: {}, node: {node}, sh: \"dist\", \
                     x: {x}, y: {y}, ds: {datasets}, ch: (:)))<{SERIES_LABEL}>])",
                    fig.node
                ));
                continue;
            }
            let (x, y) = match series.series_shape() {
                // One point, from two scalar arguments.
                SeriesShape::Anchor => {
                    match (positional(doc, series, 0), positional(doc, series, 1)) {
                        (Some(x), Some(y)) => (format!("({x},)"), format!("({y},)")),
                        _ => continue,
                    }
                }
                // One point per slot, each slot an `(x, y)` array. Split into two
                // parallel arrays so they arrive as ordinary paired points.
                SeriesShape::Vertices => {
                    let slots: Vec<String> = (0..series.positional.len())
                        .filter_map(|i| positional(doc, series, i))
                        .collect();
                    if slots.is_empty() {
                        continue;
                    }
                    let axis = |k: usize| {
                        let parts: Vec<String> = slots
                            .iter()
                            .map(|v| format!("({v}).at({k}, default: float.nan)"))
                            .collect();
                        format!("({},)", parts.join(", "))
                    };
                    (axis(0), axis(1))
                }
                SeriesShape::Rules(axis) => {
                    let coords = (0..series.positional.len())
                        .filter_map(|i| positional(doc, series, i))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let list = format!("({coords},)");
                    match axis {
                        lilook_core::Axis::X => (list, "()".to_string()),
                        lilook_core::Axis::Y => ("()".to_string(), list),
                    }
                }
                _ => match (positional(doc, series, 0), positional(doc, series, 1)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => continue,
                },
            };
            let _ = &datasets;
            // Data-bearing named arguments come back too, as a dictionary keyed
            // by argument name. `yerr` is why: a linked dataset can feed it, and
            // without recovering it an error column would be linkable but
            // invisible. The expressions are the user's own source text,
            // re-evaluated in the scope they were written in, exactly as x and y
            // are.
            let extra: Vec<String> = series
                .named
                .iter()
                .filter(|a| DATA_ARGS.contains(&a.name.as_str()))
                .map(|a| format!("{}: {}", a.name, &doc.text()[a.value.clone()]))
                .collect();
            let channels = if extra.is_empty() {
                "(:)".to_string()
            } else {
                format!("({})", extra.join(", "))
            };
            // The shape travels with the data, because how to read `x` and `y`
            // depends on it: paired coordinates for a plot, independent grid axes
            // for a colormesh. Reading a mesh as pairs zipped two axes of
            // different length into a truncated diagonal of points that
            // corresponded to nothing in the figure.
            let shape = match series.series_shape() {
                SeriesShape::Mesh => "mesh",
                SeriesShape::Points => "points",
                SeriesShape::Rules(_) => "rules",
                SeriesShape::Distributions(_) => "dist",
                // Both fill `points`, so `Scene::hit` finds them with no new case.
                // But the shape still travels: the *edit* differs, and so does what
                // may be embedded. Calling them "points" let the inspector offer to
                // materialise an anchor, which would have written `(7.85,)` where a
                // scalar belongs -- the third outing for that same bug.
                SeriesShape::Anchor => "anchor",
                SeriesShape::Vertices => "vertices",
            };
            // Relative coordinates, so this marker can never widen the data
            // range and change the layout it is trying to measure.
            args.push_str(&format!(
                ", {lq}.place(0%, 0%, [#metadata((fig: {}, node: {node}, sh: \"{shape}\", \
                 x: {x}, y: {y}, ch: {channels}))<{SERIES_LABEL}>])",
                fig.node
            ));
        }

        if let Some(at) = insertion_point(diagram, &out) {
            out.insert_str(at, &args);
        }
    }

    (out, Injection { scale_probes })
}

/// A probe's data coordinate, exactly.
///
/// This used to be `{v:.6}`, and the comment justifying it -- that typst has no
/// exponent literal in argument position -- was simply untrue; `lq.place(9e-17,
/// ..)` compiles. Six decimal places did two kinds of damage:
///
/// - it wrote any coordinate below a microunit as `0`, so a probe on a log axis
///   became `lq.place(0, ..)` and lilaq refused the whole figure: "value must be
///   strictly positive". That is the error a panned log-log plot reported.
/// - more quietly, the number written into the document then disagreed with the
///   `f64` that `solve` fits against, so on any figure whose data lives near zero
///   the recovered transform was wrong by the rounding.
///
/// `data_num` is exact and round-trips, which is what a coordinate the transform
/// is solved from has to be.
fn fmt(v: f64) -> String {
    // `NaN` and the infinities are not typst literals, and a probe cannot be
    // placed at one anyway.
    if !v.is_finite() {
        return "0".into();
    }
    lilook_core::data_num(v)
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
    /// figure -> whether each axis's data is numeric. Absent means it is.
    non_numeric: HashMap<usize, (bool, bool)>,
}

pub(crate) fn label(name: &str) -> Selector {
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

/// A numeric array, or nothing. Used for a mesh's axes, which are read whole
/// rather than zipped against each other.
fn numbers(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().filter_map(as_f64).collect(),
        _ => vec![],
    }
}

/// An array of numeric arrays: one row per dataset, or per grid row.
fn rows(v: Option<&Value>) -> Vec<Vec<f64>> {
    match v {
        Some(Value::Array(a)) => a.iter().map(numbers).collect(),
        _ => vec![],
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
        // Channels are optional: an older injection, or a series with no data
        // arguments, simply has none.
        let channels = match field(&d, "ch") {
            Some(Value::Dict(ch)) => ch
                .iter()
                .filter_map(|(k, v)| {
                    let Value::Array(a) = v else { return None };
                    let values: Vec<f64> = a.iter().filter_map(as_f64).collect();
                    // A partly-numeric array is not a data channel; refusing it
                    // is better than reporting a length that is not the file's.
                    (values.len() == a.len() && !values.is_empty())
                        .then(|| (k.as_str().to_string(), values))
                })
                .collect(),
            _ => vec![],
        };
        // A slot that held values but yielded no numbers is not empty -- it is not
        // numeric. `datetime` is that case in practice.
        let len_of = |v: &Value| match v {
            Value::Array(a) => a.len(),
            _ => 0,
        };
        let (x_len, y_len) = (len_of(&x), len_of(&y));
        if x_len > 0 && numbers(&x).is_empty() {
            raw.non_numeric
                .entry(fig as usize)
                .or_insert((true, true))
                .0 = false;
        }
        if y_len > 0 && numbers(&y).is_empty() {
            raw.non_numeric
                .entry(fig as usize)
                .or_insert((true, true))
                .1 = false;
        }
        let sh = match field(&d, "sh") {
            Some(Value::Str(s)) => s.to_string(),
            _ => "points".to_string(),
        };
        // A mesh keeps its axes apart: they have independent lengths, so there
        // are no pairs and nothing to zip. A rules series has coordinates on one
        // axis only.
        let (shape, points, grid, channels) = match sh.as_str() {
            "mesh" => {
                let (xs, ys) = (numbers(&x), numbers(&y));
                let grid = Some((xs.len(), ys.len()));
                let mut ch = vec![("x".to_string(), xs), ("y".to_string(), ys)];
                ch.extend(channels);
                (SeriesShape::Mesh, vec![], grid, ch)
            }
            "rules" => {
                let (xs, ys) = (numbers(&x), numbers(&y));
                let axis = if xs.is_empty() {
                    lilook_core::Axis::Y
                } else {
                    lilook_core::Axis::X
                };
                let name = match axis {
                    lilook_core::Axis::X => "x",
                    lilook_core::Axis::Y => "y",
                };
                let coords = if xs.is_empty() { ys } else { xs };
                let mut ch = vec![(name.to_string(), coords)];
                ch.extend(channels);
                (SeriesShape::Rules(axis), vec![], None, ch)
            }
            "dist" => {
                let (xs, ys) = (numbers(&x), numbers(&y));
                let axis = if xs.is_empty() {
                    lilook_core::Axis::Y
                } else {
                    lilook_core::Axis::X
                };
                let name = match axis {
                    lilook_core::Axis::X => "x",
                    lilook_core::Axis::Y => "y",
                };
                let positions = if xs.is_empty() { ys } else { xs };
                let mut ch = vec![(name.to_string(), positions)];
                // One channel per dataset, so the inspector can report each
                // one's size and a linked file can be checked against it.
                for (i, values) in rows(field(&d, "ds").as_ref()).into_iter().enumerate() {
                    ch.push((format!("d{i}"), values));
                }
                ch.extend(channels);
                (SeriesShape::Distributions(axis), vec![], None, ch)
            }
            // Handles on the page, edited as arguments rather than as arrays.
            "anchor" => (SeriesShape::Anchor, points(&x, &y), None, channels),
            "vertices" => (SeriesShape::Vertices, points(&x, &y), None, channels),
            _ => (SeriesShape::Points, points(&x, &y), None, channels),
        };
        raw.series
            .entry(fig as usize)
            .or_default()
            .push(SeriesGeom {
                node: node as usize,
                shape,
                channels,
                grid,
                points,
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
    mid: (f64, f64),
    data: (f64, f64, f64, f64),
) -> Option<Transform> {
    let (r0, r1) = corners;
    let (d0, d1) = scale;
    let (x0, x1, y0, y1) = data;
    Some(Transform {
        x: axis(x0, x1, (x0 + x1) / 2.0, d0.0, d1.0, mid.0, r0.0, r1.0)?,
        // `lq.place` relative `0%` is the top-left, and page y grows downward, so
        // the *first* corner carries ymax.
        y: axis(y0, y1, (y0 + y1) / 2.0, d0.1, d1.1, mid.1, r1.1, r0.1)?,
    })
}

/// One axis, fitted in whichever space it turns out to be linear in.
///
/// `dm` is the marker placed at the data midpoint of `v0`..`v1`. If the axis is
/// linear in data, its page position is the average of `p0` and `p1`. Any other
/// position means the axis bends, and the only bend lilaq offers is logarithmic --
/// so the fit is redone against `log10`, which is what stops a log axis coming out
/// as the chord between two probes.
#[allow(clippy::too_many_arguments)]
fn axis(
    v0: f64,
    v1: f64,
    vm: f64,
    p0: f64,
    p1: f64,
    pm: f64,
    page_lo: f64,
    page_hi: f64,
) -> Option<AxisMap> {
    let linear_prediction = (p0 + p1) / 2.0;
    let bend = (pm - linear_prediction).abs();
    // Relative to the span the probes cover, so the test does not depend on the
    // figure's size. Below this the two fits agree to within a fraction of a
    // point anyway, so either is right.
    let straight = bend <= (p1 - p0).abs() * 0.01;
    let positive = v0 > 0.0 && v1 > 0.0 && vm > 0.0;

    let kind = if straight || !positive {
        AxisScale::Linear
    } else {
        AxisScale::Log
    };
    let f = |v: f64| match kind {
        AxisScale::Linear => v,
        AxisScale::Log => v.log10(),
    };
    let scale = (p1 - p0) / (f(v1) - f(v0));
    if !scale.is_finite() || scale == 0.0 {
        return None;
    }
    let origin = p0 - f(v0) * scale;
    let mut map = AxisMap {
        origin,
        scale,
        min: 0.0,
        max: 0.0,
        kind,
    };
    map.min = map.to_data(page_lo);
    map.max = map.to_data(page_hi);
    if !map.min.is_finite() || !map.max.is_finite() {
        return None;
    }
    Some(map)
}

/// Turn a compiled document plus the injection record into scenes.
pub fn scenes(doc: &PagedDocument, injection: &Injection) -> Vec<Scene> {
    let mut raw = read(doc);
    let mut out = vec![];
    for (&figure, &data) in &injection.scale_probes {
        let get = |k: &str| raw.probes.get(&(figure, k.to_string())).copied();
        // Without the scale probes there is no transform to solve, but the corners
        // still give the frame -- so the diagram can be drawn, selected and
        // resized. `numeric: (false, false)` is what stops anything asking an
        // identity transform for a data coordinate it cannot supply.
        if let (Some(r0), Some(r1), None) = (get("r0"), get("r1"), get("d0")) {
            let flat = AxisMap {
                origin: 0.0,
                scale: 1.0,
                min: 0.0,
                max: 0.0,
                kind: AxisScale::Linear,
            };
            let series = raw.series.remove(&figure).unwrap_or_default();
            out.push(Scene {
                figure,
                page: r0.0,
                area: (
                    r0.1.min(r1.1),
                    r0.2.min(r1.2),
                    r0.1.max(r1.1),
                    r0.2.max(r1.2),
                ),
                transform: Transform { x: flat, y: flat },
                numeric: (false, false),
                series,
            });
            continue;
        }
        let (Some(r0), Some(r1), Some(d0), Some(d1), Some(dm)) =
            (get("r0"), get("r1"), get("d0"), get("d1"), get("dm"))
        else {
            continue;
        };
        let Some(transform) = solve(
            ((r0.1, r0.2), (r1.1, r1.2)),
            ((d0.1, d0.2), (d1.1, d1.2)),
            (dm.1, dm.2),
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
            numeric: raw
                .non_numeric
                .get(&figure)
                .copied()
                .unwrap_or((true, true)),
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
