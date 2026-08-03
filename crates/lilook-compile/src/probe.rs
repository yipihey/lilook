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
/// The label bracketing a pickable non-series element.
const ELEMENT_LABEL: &str = "lilook-el";

/// The label carrying where a figure's non-series parts landed.
const DECOR_LABEL: &str = "lilook-decor";

/// How far outside a frame a decoration may sit and still belong to it, in
/// points. A title is above the frame and an axis label below it.
const DECOR_REACH: f64 = 60.0;

/// How wide a bracketed element is taken to be, in points.
///
/// The markers bracket a flow, so they agree about the left edge and say nothing
/// about the right. Wide enough to click a colorbar and its label, narrow enough
/// not to reach the figure beside it.
const ELEMENT_WIDTH: f64 = 44.0;

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
    /// Every splice, as `(offset in the *user's* buffer, bytes inserted)`.
    ///
    /// Kept so a diagnostic's byte range can be carried back. typst reports
    /// spans against the buffer it compiled, which is the derived one, so an
    /// error after the first diagram is shifted by however much was injected
    /// before it -- a 200-byte file reporting an error at byte 781.
    pub splices: Vec<(usize, usize)>,
}

impl Injection {
    /// Carry an offset in the derived buffer back to the user's buffer.
    ///
    /// `None` when it lands *inside* injected text, which has no counterpart in
    /// what the user wrote. Declining is the right answer there: no range at all
    /// is honest, and a wrong one is worse than none.
    pub fn to_original(&self, derived: usize) -> Option<usize> {
        let mut splices = self.splices.clone();
        splices.sort_unstable();
        let mut shift = 0usize;
        for (at, len) in splices {
            if derived < at + shift {
                break;
            }
            if derived < at + shift + len {
                return None;
            }
            shift += len;
        }
        derived.checked_sub(shift)
    }

    /// Carry a whole range back, keeping it only if both ends survive and it
    /// still fits the buffer it claims to describe.
    pub fn range_to_original(
        &self,
        derived: &std::ops::Range<usize>,
        original_len: usize,
    ) -> Option<std::ops::Range<usize>> {
        let start = self.to_original(derived.start)?;
        let end = self.to_original(derived.end)?;
        (start <= end && end <= original_len).then_some(start..end)
    }
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
    let mut splices: Vec<(usize, usize)> = vec![];

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
            // A mesh's field is positional slot 2, and it is usually a *function*
            // -- so recovering it means evaluating it over the grid. Measured at
            // ~7% of a compile even at 60,000 cells, because lilaq has already
            // evaluated the same closure to draw the figure and comemo makes the
            // second pass nearly free.
            //
            // Rows over y, columns over x, which is the shape lilaq documents for
            // an explicit array; a test pins that by building one field both ways
            // and asserting the recovered values match.
            let mut field = String::new();
            if series.series_shape() == SeriesShape::Mesh {
                if let (Some(xa), Some(ya), Some(z)) = (
                    positional(doc, series, 0),
                    positional(doc, series, 1),
                    positional(doc, series, 2),
                ) {
                    field = format!(
                        ", zf: ((xa, ya, f) => if type(f) == function \
                         {{ ya.map(yy => xa.map(xx => f(xx, yy))) }} else {{ f }})\
                         ({xa}, {ya}, {z})"
                    );
                }
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
                 x: {x}, y: {y}, ch: {channels}{field}))<{SERIES_LABEL}>])",
                fig.node
            ));
        }

        if let Some(at) = insertion_point(diagram, &out) {
            splices.push((at, args.len()));
            out.insert_str(at, &args);
        }
    }

    // Elements that are not series and not diagrams -- a colorbar is the one
    // lilaq has -- are bracketed by two layout-neutral markers, so lilook learns
    // where they landed. Verified pixel-identical: `metadata` lays out to
    // nothing, and `here().position()` still reports where nothing was put.
    let mut pickable: Vec<usize> = doc
        .calls()
        .iter()
        .filter(|c| {
            c.parent.is_none()
                && c.short_name() != "diagram"
                && !c.is_xy_series()
                && SELECTABLE.contains(&c.short_name())
        })
        .map(|c| c.id)
        .collect();
    pickable.sort_by_key(|id| std::cmp::Reverse(doc.call(*id).map(|c| c.range.start)));
    // Offsets are in the *user's* text, and the figure probes above have already
    // been spliced in, so each one has to be carried forward past everything
    // inserted before it. Getting this wrong put a bracket in the middle of a
    // probe and produced `lq.[#context ...]#place(..`.
    let forward = |splices: &[(usize, usize)], at: usize| -> usize {
        splices
            .iter()
            .filter(|(s, _)| *s <= at)
            .map(|(_, len)| len)
            .sum::<usize>()
            + at
    };
    for id in &pickable {
        let Some(call) = doc.call(*id) else { continue };
        let (a, b) = (
            forward(&splices, call.range.start),
            forward(&splices, call.range.end),
        );
        // Wrapped in a content block, because the call may sit in *code* -- a
        // grid cell, an argument list -- where a bare `#` is a syntax error. In
        // markup both the marker and the call take their hash, and the block
        // lays out to exactly what it contains: verified pixel-identical.
        let marker = |e: usize| {
            format!(
                "#context [#metadata((el: {id}, e: {e}, pos: here().position()))<{ELEMENT_LABEL}>]"
            )
        };
        // In markup the call already carries its `#`; in code it does not, and
        // inside the wrapper it must, or it becomes literal text.
        let hashed = out[..a].ends_with('#');
        let close = format!("{}]", marker(1));
        let open = format!("[{}{}", marker(0), if hashed { "" } else { "#" });
        out.insert_str(b, &close);
        out.insert_str(a, &open);
        // Recorded against the user's own text, like every other splice, because
        // that is the coordinate system diagnostics are carried back into.
        splices.push((call.range.end, close.len()));
        splices.push((call.range.start, open.len()));
    }

    let lq = doc.lilaq_alias();
    // Only where there is a figure to ask about. A document with no diagram has
    // no decorations, and `lq.selector` is not defined in one that never
    // imported lilaq -- which made every such compile fail.
    if !figures.is_empty() {
        // Where lilaq put the parts of a figure that are not series. None of these
        // is a call site -- a legend is `legend: (..)` on the diagram -- so there is
        // nothing to bracket; but typst can locate them, which is enough.
        //
        // One query for the whole document: which diagram each belongs to is decided
        // by where it landed, not by counting, so a figure with three diagrams and
        // two legends still gets it right.
        //
        // `measure(m)` alongside the position: unlike a colorbar, a decoration
        // is not a call site to bracket, but it is real content once query
        // finds it, and typst can lay that same content out again to learn its
        // size -- a legend or a label does not stretch to fill a container, so
        // measuring it gives the size it actually rendered at, not a guess.
        let spot = |el: &str| {
            format!(
                "query({lq}.selector({lq}.{el})).map(m => {{\
                   let p = m.location().position()\n\
                   let s = measure(m)\n\
                   (page: p.page, x: p.x, y: p.y, w: s.width, h: s.height)\n\
                 }})"
            )
        };
        out.push_str(&format!(
            "\n#context [#metadata((\
           legend: {}, \
           title: {}, \
           label: {} \
         ))<{DECOR_LABEL}>]\n",
            spot("legend"),
            spot("title"),
            spot("label"),
        ));
    }

    (
        out,
        Injection {
            scale_probes,
            splices,
        },
    )
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

/// A page and a point on it: where a marker ended up.
type Spot = (usize, f64, f64);

/// A decoration's kind, where it landed (with its page), and its measured
/// (width, height) in page points when `measure` on it succeeded -- the
/// pre-fold shape, before it is assigned to the diagram it belongs to.
type RawDecoration = (lilook_core::scene::Decoration, Spot, Option<(f64, f64)>);

#[derive(Debug, Default)]
struct Raw {
    /// (figure, kind) -> (page, x pt, y pt)
    probes: HashMap<(usize, String), (usize, f64, f64)>,
    /// figure -> [(series node, points)]
    series: HashMap<usize, Vec<SeriesGeom>>,
    /// figure -> whether each axis's data is numeric. Absent means it is.
    non_numeric: HashMap<usize, (bool, bool)>,
    /// Where the figure's non-series parts landed, before they are assigned to
    /// the diagram that contains them.
    decorations: Vec<RawDecoration>,
    /// call -> where the content flow entered and left a pickable element.
    elements: HashMap<usize, [Option<Spot>; 2]>,
}

/// Calls worth being able to click, that are neither a diagram nor a series.
///
/// A colorbar is the one lilaq has: a frame of its own, drawn beside a figure,
/// which until now could be reached only through the tree.
const SELECTABLE: &[&str] = &["colorbar"];

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

    if let Some(c) = doc.introspector().query_first(&label(DECOR_LABEL)) {
        use lilook_core::scene::Decoration;
        if let Ok(Value::Dict(d)) = c.field_by_name("value") {
            for (key, kind) in [
                ("legend", Decoration::Legend),
                ("title", Decoration::Title),
                // Which label is which is decided when it is assigned to a
                // frame, from where it sits: under it, or beside it.
                ("label", Decoration::XLabel),
            ] {
                let Some(Value::Array(list)) = field(&d, key) else {
                    continue;
                };
                for item in list.iter() {
                    let Value::Dict(pos) = item else { continue };
                    let (Some(page), Some(x), Some(y)) = (
                        field(pos, "page").as_ref().and_then(as_f64),
                        field(pos, "x").as_ref().and_then(as_pt),
                        field(pos, "y").as_ref().and_then(as_pt),
                    ) else {
                        continue;
                    };
                    // Absent only if `measure` itself failed -- content this
                    // pathological would have failed to render at all, but a
                    // missing size degrades to the old assumed box rather
                    // than dropping the decoration.
                    let extent = match (
                        field(pos, "w").as_ref().and_then(as_pt),
                        field(pos, "h").as_ref().and_then(as_pt),
                    ) {
                        (Some(w), Some(h)) => Some((w, h)),
                        _ => None,
                    };
                    raw.decorations
                        .push((kind, ((page as usize).saturating_sub(1), x, y), extent));
                }
            }
        }
    }

    for c in doc.introspector().query(&label(ELEMENT_LABEL)) {
        let Ok(Value::Dict(d)) = c.field_by_name("value") else {
            continue;
        };
        let (Some(el), Some(end), Some(Value::Dict(pos))) =
            (field(&d, "el"), field(&d, "e"), field(&d, "pos"))
        else {
            continue;
        };
        let (Some(el), Some(end), Some(page), Some(x), Some(y)) = (
            as_f64(&el),
            as_f64(&end),
            field(&pos, "page").as_ref().and_then(as_f64),
            field(&pos, "x").as_ref().and_then(as_pt),
            field(&pos, "y").as_ref().and_then(as_pt),
        ) else {
            continue;
        };
        raw.elements.entry(el as usize).or_default()[end as usize] =
            Some(((page as usize).saturating_sub(1), x, y));
    }

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
                // The field, flattened row-major, so `grid` alone indexes it.
                let z: Vec<f64> = rows(field(&d, "zf").as_ref()).concat();
                if !z.is_empty() {
                    ch.push(("z".to_string(), z));
                }
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

    // The fit came from two probes; the third tests it.
    //
    // Without this the choice is a coin flip between the only two scales lilook
    // knows, and lilaq ships more than two. A **symlog** axis bends and is
    // positive, so it fitted "log" and the transform was quietly wrong -- pans
    // and drags wrote incorrect limits with no error anywhere. Declining is the
    // honest answer: `Scene::numeric` then reports the axis as unusable in data
    // space and the gestures fall back to moving the view, exactly as they do for
    // a datetime axis.
    let predicted = origin + f(vm) * scale;
    if (predicted - pm).abs() > (p1 - p0).abs() * 0.02 {
        return None;
    }

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
    // An axis whose recovered maximum is below its minimum is not a transform,
    // whatever scale it came from. A custom `lq.scale` produced exactly that --
    // `min 0.846, max -1.749` -- and it passed every check above, because the
    // three probes happened to look straight in page space while the mapping
    // between them was anything but.
    //
    // Cheap, and it does not depend on knowing which scales lilaq has: any fit
    // that inverts the axis is refused, and the figure degrades to frame-only.
    if map.min >= map.max {
        return None;
    }
    Some(map)
}

/// Turn a compiled document plus the injection record into scenes.
pub fn scenes(doc: &PagedDocument, injection: &Injection) -> Vec<Scene> {
    let mut raw = read(doc);
    let mut out = vec![];

    // A colorbar is a frame on the page with no data in it. Modelled as a scene
    // of its own so the canvas needs nothing new: it already picks a scene by the
    // area it covers, and already offers the frame's edges for resizing -- which
    // works here because a colorbar forwards `width` and `height` to the diagram
    // it is drawn from.
    //
    // `numeric: (false, false)` because there is no data transform to recover:
    // the two markers bracket where it sits, not what it means.
    for (&node, ends) in &raw.elements {
        let (Some(a), Some(b)) = (ends[0], ends[1]) else {
            continue;
        };
        if a.0 != b.0 {
            continue; // split across pages; nothing sensible to outline
        }
        let flat = AxisMap {
            origin: 0.0,
            scale: 1.0,
            min: 0.0,
            max: 0.0,
            kind: AxisScale::Linear,
        };
        out.push(Scene {
            figure: node,
            page: a.0,
            // The two markers report where the content flow entered and left,
            // which gives an honest *height* and no width at all -- both sit on
            // the same left edge. So the box is the flow between them, grown
            // rightwards by enough to be clickable.
            //
            // A pick target, not a measurement. Recovering a real outline would
            // need corner probes inside the element, and a colorbar's argument
            // list has nowhere to put them.
            area: (
                a.1.min(b.1),
                a.2.min(b.2),
                a.1.max(b.1) + ELEMENT_WIDTH,
                a.2.max(b.2),
            ),
            transform: Transform { x: flat, y: flat },
            numeric: (false, false),
            series: vec![],
            decorations: vec![],
        });
    }
    for (&figure, &data) in &injection.scale_probes {
        let get = |k: &str| raw.probes.get(&(figure, k.to_string())).copied();
        // Without the scale probes there is no transform to solve, but the corners
        // still give the frame -- so the diagram can be drawn, selected and
        // resized. `numeric: (false, false)` is what stops anything asking an
        // identity transform for a data coordinate it cannot supply.
        // A frame with no usable data transform: the diagram can still be drawn,
        // selected and resized, and `numeric: (false, false)` stops anything
        // asking an identity transform for a data coordinate it cannot supply.
        // Reached two ways -- no scale probes at all, and probes whose fit no
        // scale lilook knows reproduces (a symlog or a custom `lq.scale`).
        let frame_only = |raw: &mut Raw, r0: (usize, f64, f64), r1: (usize, f64, f64)| {
            let flat = AxisMap {
                origin: 0.0,
                scale: 1.0,
                min: 0.0,
                max: 0.0,
                kind: AxisScale::Linear,
            };
            let series = raw.series.remove(&figure).unwrap_or_default();
            Scene {
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
                decorations: vec![],
            }
        };
        if let (Some(r0), Some(r1), None) = (get("r0"), get("r1"), get("d0")) {
            out.push(frame_only(&mut raw, r0, r1));
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
            out.push(frame_only(&mut raw, r0, r1));
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
            decorations: vec![],
        });
    }
    for (kind, spot, extent) in &raw.decorations {
        let Some(scene) = out
            .iter_mut()
            .filter(|s| s.page == spot.0 && !s.series.is_empty())
            .find(|s| {
                spot.1 >= s.area.0 - DECOR_REACH
                    && spot.1 <= s.area.2 + DECOR_REACH
                    && spot.2 >= s.area.1 - DECOR_REACH
                    && spot.2 <= s.area.3 + DECOR_REACH
            })
        else {
            continue;
        };
        // An axis label's identity is its position: below the frame is the x
        // label, left of it the y. lilaq reports both under one element type, and
        // guessing from the order they come back in would be a coin flip.
        let kind = match kind {
            lilook_core::scene::Decoration::XLabel if spot.1 < scene.area.0 => {
                lilook_core::scene::Decoration::YLabel
            }
            other => *other,
        };
        scene.decorations.push((kind, (spot.1, spot.2), *extent));
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
