//! The document: Typst source, its CST, and the lilaq call sites within it.
//!
//! Phase 0 established that `typst_syntax` round-trips losslessly, so the
//! source text is the single source of truth and every change is a surgical
//! byte-range replacement. Nothing is ever regenerated from a model.

use crate::edit::{Anchor, AppliedEdit, History};
use crate::intent::Intent;
use std::collections::HashMap;
use std::ops::Range;
use typst_syntax::{parse, LinkedNode, SyntaxKind, SyntaxNode};

/// How editable an argument value is from a GUI control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editability {
    /// A literal we can bind straight to a widget.
    Literal,
    /// A known builtin constant (`red`, `auto`, `center`) -- editable, and the
    /// widget can show its value.
    Builtin,
    /// A user `#let` binding. Read-only here; offer jump-to-definition.
    Binding,
    /// Computed, callable or otherwise outside the recognised profile.
    Opaque,
}

/// Typst builtins that parse as bare identifiers. Phase 0 found that `red` and
/// a user's `accent` are both `SyntaxKind::Ident`, so this table is what tells
/// a colour swatch from a jump-to-definition.
const BUILTIN_IDENTS: &[&str] = &[
    "black", "gray", "silver", "white", "navy", "blue", "aqua", "teal", "eastern", "purple",
    "fuchsia", "maroon", "red", "orange", "yellow", "olive", "green", "lime", "luma", "oklab",
    "oklch", "rgb", "cmyk", "left", "center", "right", "top", "horizon", "bottom", "start", "end",
    "ltr", "rtl", "ttb", "btt",
];

#[derive(Debug, Clone)]
pub struct NamedArg {
    pub name: String,
    /// Byte range of the *value*, which is what an edit replaces.
    pub value: Range<usize>,
    pub editability: Editability,
    pub text: String,
}

/// A positional argument -- a data slot. Unlike a named argument its identity
/// is its index, and what the GUI can do with it depends on whether the user
/// wrote a literal there or an expression.
#[derive(Debug, Clone)]
pub struct PositionalArg {
    pub range: Range<usize>,
    pub editability: Editability,
    pub text: String,
    /// Byte ranges of the elements, when this is a literal array. Empty
    /// otherwise -- which is exactly the test for "can this point be dragged".
    pub elements: Vec<Range<usize>>,
}

#[derive(Debug, Clone)]
pub struct CallSite {
    pub id: usize,
    /// e.g. `lq.plot`
    pub callee: String,
    pub range: Range<usize>,
    pub named: Vec<NamedArg>,
    /// Positional arguments, in order.
    pub positional: Vec<PositionalArg>,
    /// True when the call is produced by a loop, closure or spread and so is
    /// visible but not structurally editable.
    pub generated: bool,
    /// The nearest enclosing lilaq call, which is what makes a series belong to
    /// a diagram. Nesting, not position: `lq.plot` inside `lq.diagram(..)` is a
    /// child even when the source puts them on different lines.
    pub parent: Option<usize>,
}

impl CallSite {
    /// The name without its module prefix: `lq.plot` -> `plot`.
    pub fn short_name(&self) -> &str {
        self.callee.rsplit('.').next().unwrap_or(&self.callee)
    }

    /// The alias lilaq was imported under at this call site, e.g. `lq`.
    pub fn module(&self) -> Option<&str> {
        self.callee.rsplit_once('.').map(|(m, _)| m)
    }

    /// True when positional arguments 0 and 1 carry the geometry, so the compile
    /// backend can recover what was drawn from them.
    pub fn is_xy_series(&self) -> bool {
        XY_SERIES.contains(&self.short_name())
    }

    /// How to read the positional arguments: paired coordinates, grid axes, or
    /// one line each.
    pub fn series_shape(&self) -> SeriesShape {
        series_shape_of(self.short_name())
    }

    /// True when both data slots are literal arrays of the same length, which
    /// is what makes an individual point draggable. Computed data is shown and
    /// hit-tested but not moved: the edit would have to rewrite an expression.
    ///
    /// Never true for a mesh: its slots are axes of independent length, so there
    /// is no point to move even when both are literal.
    pub fn has_literal_points(&self) -> bool {
        if self.series_shape() != SeriesShape::Points {
            return false;
        }
        match (self.positional.first(), self.positional.get(1)) {
            (Some(x), Some(y)) => !x.elements.is_empty() && x.elements.len() == y.elements.len(),
            _ => false,
        }
    }
}

/// Every positional argument of a rules series is one coordinate, and each is
/// draggable when it is a literal number: `hlines(1, 2, 3)` has three.
///
/// Separate from `has_literal_points` because the edit is different -- a slot
/// rather than an array element -- and so is the geometry.
impl CallSite {
    /// True when an anchored annotation's `(x, y)` are both literal numbers, so
    /// the annotation can be dragged.
    pub fn has_literal_anchor(&self) -> bool {
        self.series_shape() == SeriesShape::Anchor
            && (0..2).all(|i| {
                self.positional
                    .get(i)
                    .is_some_and(|p| p.editability == Editability::Literal && p.elements.is_empty())
            })
    }

    /// The slots holding a literal `(x, y)` vertex, so those vertices can be
    /// dragged. Each is an array of exactly two literal numbers.
    pub fn literal_vertices(&self) -> Vec<usize> {
        if self.series_shape() != SeriesShape::Vertices {
            return vec![];
        }
        self.positional
            .iter()
            .enumerate()
            .filter(|(_, p)| p.elements.len() == 2)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn literal_rules(&self) -> Vec<usize> {
        if !matches!(self.series_shape(), SeriesShape::Rules(_)) {
            return vec![];
        }
        self.positional
            .iter()
            .enumerate()
            .filter(|(_, p)| p.editability == Editability::Literal && p.elements.is_empty())
            .map(|(i, _)| i)
            .collect()
    }
}

/// lilaq constructors whose first two positional arguments carry the geometry.
/// Checked against the generated schema by a test, so a lilaq release that
/// reorders a signature fails loudly rather than silently mis-plotting a hit test.
pub const XY_SERIES: &[&str] = &[
    "plot",
    "scatter",
    "bar",
    "hbar",
    "stem",
    "hstem",
    "quiver",
    "colormesh",
    "contour",
    "fill-between",
    "hlines",
    "vlines",
    "boxplot",
    "violin",
    "hboxplot",
    "hviolin",
    "place",
    "rect",
    "ellipse",
    "line",
    "path",
];

/// What slots 0 and 1 *mean*, which is not the same for every series.
///
/// The distinction is load-bearing and its absence was a real defect: a
/// `colormesh(xs, ys, z)` on a 5x4 grid was read as four paired points down the
/// diagonal -- zipped, so truncated to the shorter axis -- and drawn as four
/// draggable markers that corresponded to nothing in the figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesShape {
    /// Parallel arrays: one point per index. `plot`, `scatter`, `bar`, ...
    Points,
    /// Grid axes, with a field over them in slot 2. `colormesh` and `contour`:
    /// x has one value per column and y one per row, so there is no pairing
    /// between them and no single point to pick up.
    Mesh,
    /// Every positional argument is *one line's* coordinate on a single axis:
    /// `hlines(1, 2, 3)` draws three horizontal lines. There is no second
    /// coordinate at all -- the line spans the frame -- so moving one rewrites
    /// that argument rather than an element of an array.
    Rules(Axis),
    /// Every positional argument is *one dataset*, and its position along the
    /// carried axis comes from a named `x:`/`y:` -- or from `1..n` when that is
    /// `auto`, which is the default. `boxplot`, `violin` and their h variants.
    ///
    /// The distribution extends along the *other* axis, and lilook does not
    /// compute the quartiles: it knows where each box sits and what values went
    /// into it, which is what selection and an honest readout need.
    Distributions(Axis),
    /// Slots 0 and 1 are the *scalar* coordinates of one point: `place(x, y, ..)`,
    /// `rect`, `ellipse`. Moving it rewrites those two arguments.
    Anchor,
    /// Each positional slot is one vertex, written as an `(x, y)` array:
    /// `line(start, end)`, `path(..vertices)`. Moving a vertex rewrites two
    /// elements *inside* that slot.
    Vertices,
}

/// Which axis a value sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// How to read a series' positional slots, by name alone.
///
/// Free-standing as well as a method, because a frontend with only a name in
/// hand -- an MCP server categorising what lilaq can draw -- needs the same
/// answer without a parsed call site to ask.
pub fn series_shape_of(name: &str) -> SeriesShape {
    if let Some((_, axis)) = DISTRIBUTION_SERIES.iter().find(|(n, _)| *n == name) {
        return SeriesShape::Distributions(*axis);
    }
    match name {
        n if MESH_SERIES.contains(&n) => SeriesShape::Mesh,
        "hlines" => SeriesShape::Rules(Axis::Y),
        "vlines" => SeriesShape::Rules(Axis::X),
        "place" | "rect" | "ellipse" => SeriesShape::Anchor,
        "line" | "path" => SeriesShape::Vertices,
        _ => SeriesShape::Points,
    }
}

/// The mesh-shaped constructors. Their slots 0 and 1 are axes, not coordinates.
///
/// Deliberately *not* `lq.mesh`, despite the name. That is a data helper in
/// lilaq's `math.typ` -- it evaluates a function over a grid and returns the
/// field, drawing nothing -- and listing it here made it a phantom series: a
/// document doing the idiomatic `#let z = lq.mesh(xs, ys, f)` showed two entries
/// under one diagram, the second reporting a plausible "6x4 grid" for a call
/// that puts no ink on the page.
const MESH_SERIES: &[&str] = &["colormesh", "contour"];

/// The distribution constructors, and which axis carries their *position*.
const DISTRIBUTION_SERIES: &[(&str, Axis)] = &[
    ("boxplot", Axis::X),
    ("violin", Axis::X),
    ("hboxplot", Axis::Y),
    ("hviolin", Axis::Y),
];

/// A `#show: lq.set-tick(..)` and the region of the document it governs.
///
/// This is a show rule, not a property of a figure: it applies from where it
/// appears to the end of its enclosing scope, so the same rule may restyle
/// several figures or none. lilook edits set rules from a document-level panel
/// rather than from a figure's inspector, because a panel that said "this
/// diagram's ticks" would be claiming something Typst does not mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetRule {
    /// The `lq.set-*` call site, which is an ordinary indexed call and so is
    /// edited by the ordinary intents.
    pub node: usize,
    /// The element it configures: `tick`, `legend`, ...
    pub element: String,
    /// Bytes it governs: from the rule to the end of its enclosing block.
    pub scope: Range<usize>,
    /// True when the enclosing scope is the file itself.
    pub document_level: bool,
}

/// What a range of source *is*, for a frontend to colour.
///
/// Deliberately about meaning rather than syntax: `Series(i)` is not a token
/// kind any parser reports, but it is the one that lets a source pane show a
/// curve's own colour beside the line that drew it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Comment,
    Str,
    Number,
    /// `#import`, `#let`, `#show`, `#set` and the rest.
    Keyword,
    /// A call that draws a series, and which one it is in its diagram.
    Series(usize),
    /// Any other function call.
    Call,
    /// A name bound by `#let`.
    Binding,
}

/// What sits at a byte offset in the source.
///
/// Deliberately small and plain: a frontend asks where the caret is and gets
/// back enough to know what to offer, without a parse of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The innermost call containing the offset.
    pub call: Option<usize>,
    /// The named argument it is inside, if any.
    pub argument: Option<String>,
    /// True when the caret is on the *name* rather than the value -- so a
    /// completion offers parameters, not values.
    pub on_name: bool,
    /// The positional slot it is inside, if any.
    pub slot: Option<usize>,
    /// Inside a string literal, where nothing should be offered.
    pub in_string: bool,
}

/// A theme applied to the document: `#show: lq.theme.ocean`.
///
/// lilaq's themes are *show rules*, not objects, which is why lilook needs no
/// theme format of its own -- applying one is a text edit and nothing else.
/// Deriving from one is a `#let` that composes it with `set-*` overrides the
/// inspector already edits:
///
/// ```typst
/// #let mine = it => { show: lq.theme.ocean; show: lq.set-tick(inset: 4pt); it }
/// #show: mine
/// ```
///
/// Composition rather than copying a theme's body, deliberately: `schoolbook`
/// imports `@preview/tiptoe` and defines local helpers, and every theme's
/// `set-*` calls are unqualified inside their own module. A copy would have to
/// chase all of that and would go stale if lilaq revised a theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The whole `#show: ..` rule, so it can be replaced or removed.
    pub range: Range<usize>,
    /// Just the transform, for repointing one theme at another.
    pub transform: Range<usize>,
    /// `ocean` for `lq.theme.ocean`; the binding's name for a local one.
    pub name: String,
    /// True when `name` is a `#let` in this document rather than one of lilaq's.
    pub local: bool,
    /// True when the rule is at the top level of the file rather than inside a
    /// block.
    ///
    /// The same distinction `SetRule` draws, and for the same reason. A derived
    /// theme contains `show: lq.theme.ocean` *inside* its own `#let` body -- that
    /// is its definition, not the document's theme -- and treating it as the
    /// latter meant removing a theme appeared to leave one in force.
    pub document_level: bool,
}

/// One diagram and the series drawn inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figure {
    pub node: usize,
    pub series: Vec<usize>,
}

pub struct Document {
    text: String,
    root: SyntaxNode,
    calls: Vec<CallSite>,
    anchors: HashMap<u64, Anchor>,
    next_anchor: u64,
    history: History,
}

impl Document {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let root = parse(&text);
        let mut doc = Document {
            text,
            root,
            calls: vec![],
            anchors: HashMap::new(),
            next_anchor: 0,
            history: History::default(),
        };
        doc.reindex();
        doc
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn calls(&self) -> &[CallSite] {
        &self.calls
    }

    pub fn call(&self, id: usize) -> Option<&CallSite> {
        self.calls.iter().find(|c| c.id == id)
    }

    /// Every `lq.diagram` in the document, with the series nested inside it.
    ///
    /// A series is a descendant rather than a direct child: `lq.diagram(..(
    /// if flag { lq.plot(..) }))` still draws that plot, and the user still
    /// expects to click it.
    pub fn figures(&self) -> Vec<Figure> {
        self.calls
            .iter()
            .filter(|c| c.short_name() == "diagram")
            .map(|d| {
                let mut series: Vec<usize> = self
                    .calls
                    .iter()
                    .filter(|c| c.is_xy_series() && self.descends_from(c.id, d.id))
                    .map(|c| c.id)
                    .collect();
                for id in self.series_named_by(d) {
                    if !series.contains(&id) {
                        series.push(id);
                    }
                }
                // Document order, so the tree lists them the way they are written.
                series.sort_unstable();
                Figure { node: d.id, series }
            })
            .collect()
    }

    /// Series a diagram draws by *name* rather than by containing them.
    ///
    /// `#let mesh = lq.contour(..)` and then `lq.diagram(mesh)` is idiomatic
    /// lilaq, and unavoidable when two things share one plot -- a contour and the
    /// `lq.colorbar` beside it, which is exactly what the plot-grid tutorial
    /// does. Nesting alone finds no series there, so the diagram came out empty:
    /// nothing to select, nothing to inspect, no points recovered.
    ///
    /// One hop only, deliberately. `#let a = b` chains and series built inside
    /// functions are not followed: a wrong answer about which figure draws what
    /// is worse than no answer, and one hop is the shape lilaq documents.
    fn series_named_by(&self, diagram: &CallSite) -> Vec<usize> {
        let mut out = vec![];
        for slot in &diagram.positional {
            for name in self.free_identifiers(slot.range.clone()) {
                let Some(binding) = self.binding_of(&name) else {
                    continue;
                };
                // The series call inside that `#let`. A binding holding anything
                // else -- `lq.linspace`, a number -- simply contributes nothing.
                out.extend(
                    self.calls
                        .iter()
                        .filter(|c| {
                            c.is_xy_series()
                                && c.range.start >= binding.start
                                && c.range.end <= binding.end
                        })
                        .map(|c| c.id),
                );
            }
        }
        out
    }

    /// Identifiers a fragment of the document depends on from outside itself.
    ///
    /// This is what makes copy/paste more than string handling: a copied series
    /// routinely reads `#let x = lq.linspace(..)` defined elsewhere, and pasting
    /// it somewhere that binding does not exist produces a document that will
    /// not compile. The caller decides what to do about it -- carry the
    /// bindings, or paste and report what is unresolved -- but it has to know.
    ///
    /// Bindings made *inside* the fragment (closure parameters, a local `#let`)
    /// are not free, and neither are field names, argument names or builtins.
    pub fn free_identifiers(&self, range: Range<usize>) -> Vec<String> {
        let root = LinkedNode::new(&self.root);
        let Some(node) = find_node(&root, &range) else {
            return vec![];
        };
        let mut out = vec![];
        let mut bound = vec![];
        free_idents(&node, &self.text, &mut bound, &mut out);
        out.sort();
        out.dedup();
        out
    }

    /// The whole `#let name = ..` (or `#let name(..) = ..`) that binds `name` at
    /// the top level, if any.
    pub fn binding_of(&self, name: &str) -> Option<Range<usize>> {
        let root = LinkedNode::new(&self.root);
        let mut found = None;
        find_binding(&root, &self.text, name, &mut found);
        // The `#` is a markup token outside the `LetBinding` node, and a caller
        // carrying this binding into another document needs the whole thing.
        found.map(|r| match self.text[..r.start].ends_with('#') {
            true => r.start - 1..r.end,
            false => r,
        })
    }

    /// Every name the document binds with `#let`.
    ///
    /// Derived from the same spans the source pane colours, so there is one
    /// answer to "what is a binding here" rather than two that can disagree.
    pub fn binding_names(&self) -> Vec<String> {
        self.spans()
            .into_iter()
            .filter(|(_, t)| *t == Token::Binding)
            .map(|(r, _)| self.text[r].to_string())
            .collect()
    }

    /// Every `#show: lq.set-*(..)` in the document, with the region it governs.
    pub fn set_rules(&self) -> Vec<SetRule> {
        let mut out = vec![];
        let root = LinkedNode::new(&self.root);
        collect_set_rules(&root, self.text.len(), &mut out, &self.calls);
        out.sort_by_key(|r| r.node);
        out
    }

    /// What this document calls lilaq: the alias on its `#import`.
    ///
    /// In the core rather than in a frontend because it is a fact about the
    /// document, and every frontend that writes `lq.something` needs the same
    /// answer. The obvious shortcut -- take the module off the first call site --
    /// is wrong on any document whose first call is nested, and it shipped:
    /// `lq.vec.add` made the alias `lq.vec`, and the theme picker wrote
    /// `#show: lq.vec.theme.ocean`.
    pub fn lilaq_alias(&self) -> String {
        for line in self.text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("#import ") else {
                continue;
            };
            if !rest.contains("lilaq") {
                continue;
            }
            // `#import "@preview/lilaq:0.6.0" as lq`, and the bare form where the
            // module takes the package's own name.
            if let Some((_, alias)) = rest.rsplit_once(" as ") {
                let alias = alias.trim().trim_end_matches(':').trim();
                if !alias.is_empty() && !alias.contains(' ') {
                    return alias.to_string();
                }
            }
            return "lilaq".to_string();
        }
        // No import found: a fragment, or an unusual spelling. Fall back to the
        // module of a call this document already makes, preferring the shallow
        // ones -- `lq.plot` over `lq.vec.add`.
        self.calls
            .iter()
            .filter(|c| c.is_xy_series() || c.short_name() == "diagram")
            .find_map(|c| c.module())
            .unwrap_or("lq")
            .to_string()
    }

    /// Every theme show rule in the document, in document order.
    ///
    /// A bare identifier counts only when the document also binds it to a
    /// function of one argument -- the shape a theme has. Without that check
    /// `#show: emph` and every other ordinary show rule would be offered as a
    /// theme to switch away from.
    pub fn themes(&self) -> Vec<Theme> {
        let mut out = vec![];
        let root = LinkedNode::new(&self.root);
        collect_themes(&root, &mut out, self);
        out.sort_by_key(|t| t.range.start);
        out
    }

    /// Is this name bound to something theme-shaped -- `#let n = it => ..`?
    fn binds_a_theme(&self, name: &str) -> bool {
        let Some(range) = self.binding_of(name) else {
            return false;
        };
        let text = &self.text[range];
        // A closure of exactly one parameter. Enough to tell a theme from a
        // value, and it is what every lilaq theme is.
        text.split_once('=')
            .map(|(_, rhs)| rhs.trim_start())
            .is_some_and(|rhs| {
                let head = rhs.split("=>").next().unwrap_or("");
                rhs.contains("=>") && !head.contains(',') && head.len() < 40
            })
    }

    /// The document, as coloured spans.
    ///
    /// Data, not colours: the core says *what* each range is and a frontend
    /// decides how it looks, which is what lets egui, SwiftUI and a terminal
    /// agree about a document without agreeing about a palette.
    ///
    /// Spans are non-overlapping and in order, so a renderer can walk them
    /// straight into a layout without sorting or nesting.
    pub fn spans(&self) -> Vec<(Range<usize>, Token)> {
        let mut out: Vec<(Range<usize>, Token)> = vec![];
        collect_tokens(&LinkedNode::new(&self.root), &mut out);

        // A series call is tinted by the colour it draws, so its *ordinal within
        // its diagram* has to travel with it -- that is what indexes the cycle.
        for fig in self.figures() {
            for (i, id) in fig.series.iter().enumerate() {
                let Some(call) = self.call(*id) else { continue };
                // The callee, not the whole call: colouring the arguments too
                // would drown out the literals inside them.
                let head =
                    call.range.start..call.range.start + call.callee.len().min(call.range.len());
                out.retain(|(r, _)| r.start < head.start || r.start >= head.end);
                out.push((head, Token::Series(i)));
            }
        }
        out.sort_by_key(|(r, _)| r.start);
        out.dedup_by_key(|(r, _)| r.start);
        out
    }

    /// What is at this byte offset.
    ///
    /// The second addressing axis. Everything lilook does today is *node*
    /// addressed -- an operation takes a call-site id -- but every question a
    /// source pane asks is *position* addressed: what may I write here, what is
    /// this, what did it resolve to. Both have to exist.
    ///
    /// The innermost call wins, so a cursor inside `lq.diagram(lq.plot(..))`
    /// names the plot, which is what a human pointing at it would say.
    pub fn at(&self, offset: usize) -> Cursor {
        let call = self
            .calls
            .iter()
            .filter(|c| c.range.contains(&offset))
            // Innermost: the shortest containing range.
            .min_by_key(|c| c.range.end - c.range.start);
        let Some(call) = call else {
            return Cursor::default();
        };
        let mut cursor = Cursor {
            call: Some(call.id),
            ..Cursor::default()
        };
        // Inside a string literal nothing may be offered: the user is writing
        // words, and a parameter list dropped into the middle of them is noise.
        cursor.in_string = self.in_string_literal(offset);
        for arg in &call.named {
            if arg.value.contains(&offset) {
                cursor.argument = Some(arg.name.clone());
                return cursor;
            }
        }
        for (i, slot) in call.positional.iter().enumerate() {
            if slot.range.contains(&offset) {
                cursor.slot = Some(i);
                return cursor;
            }
        }
        // Inside the call but inside none of its arguments: between them, or in
        // the whitespace after a comma. That is where a *name* goes, so this is
        // where a completion offers parameters.
        cursor.on_name = true;
        cursor
    }

    /// Is this offset inside a string literal?
    fn in_string_literal(&self, offset: usize) -> bool {
        let mut node = LinkedNode::new(&self.root).leaf_at(offset, typst_syntax::Side::Before);
        while let Some(n) = node {
            if n.kind() == SyntaxKind::Str {
                return true;
            }
            node = n.parent().cloned();
        }
        false
    }

    /// Is this call drawn against an axis of its own rather than the diagram's?
    ///
    /// `lq.diagram(.., lq.yaxis(position: right, lq.plot(..)))` is how lilaq does
    /// a twin axis: the plot is nested inside an axis, and that axis has its own
    /// scale. lilook recovers one transform per *diagram*, so such a series is
    /// read correctly but cannot be positioned by it.
    pub fn on_secondary_axis(&self, id: usize) -> bool {
        let mut at = self.call(id).and_then(|c| c.parent);
        while let Some(p) = at {
            let Some(call) = self.call(p) else {
                return false;
            };
            if matches!(call.short_name(), "axis" | "xaxis" | "yaxis") {
                return true;
            }
            at = call.parent;
        }
        false
    }

    /// The diagram a call site is drawn in, if any.
    pub fn figure_of(&self, id: usize) -> Option<usize> {
        let mut at = self.call(id)?.parent;
        while let Some(p) = at {
            let call = self.call(p)?;
            if call.short_name() == "diagram" {
                return Some(p);
            }
            at = call.parent;
        }
        None
    }

    fn descends_from(&self, mut id: usize, ancestor: usize) -> bool {
        while let Some(p) = self.call(id).and_then(|c| c.parent) {
            if p == ancestor {
                return true;
            }
            id = p;
        }
        false
    }

    pub fn history_depth(&self) -> (usize, usize) {
        self.history.depth()
    }

    /// Re-parse and rebuild the call index. Cheap enough at figure scale; the
    /// incremental reparser is the optimisation, not a correctness requirement.
    fn reindex(&mut self) {
        self.root = parse(&self.text);
        let mut calls = vec![];
        let linked = LinkedNode::new(&self.root);
        let mut id = 0usize;
        collect(&linked, &self.text, &mut calls, &mut id, false, None);
        self.calls = calls;
    }

    // ---------------------------------------------------------- transactions

    /// Open a coalescing transaction (mousedown on a slider). Every intent
    /// applied until `commit` becomes one undo step, and intents that rewrite
    /// the same target collapse into a single edit -- which target is decided
    /// per intent, not per transaction, because one gesture can drive several
    /// parameters at once.
    pub fn begin(&mut self, label: &str) {
        self.history.begin(label);
    }

    /// Close it (mouseup). Everything in between is one undo step.
    pub fn commit(&mut self) {
        self.history.commit();
    }

    pub fn apply(&mut self, intent: Intent) -> Result<(), String> {
        let edit = self.resolve(&intent)?;
        let key = intent.coalesce_key();
        self.splice(&edit);
        self.history.record(edit, key);
        Ok(())
    }

    fn splice(&mut self, edit: &AppliedEdit) {
        self.text.replace_range(edit.range.clone(), &edit.after);
        for a in self.anchors.values_mut() {
            a.transform(edit);
        }
        self.reindex();
    }

    pub fn undo(&mut self) -> bool {
        let Some(tx) = self.history.take_undo() else {
            return false;
        };
        for e in tx.edits.iter().rev() {
            let inv = e.inverse();
            self.text.replace_range(inv.range.clone(), &inv.after);
            for a in self.anchors.values_mut() {
                a.transform(&inv);
            }
        }
        self.reindex();
        self.history.push_undone(tx);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(tx) = self.history.take_redo() else {
            return false;
        };
        for e in tx.edits.iter() {
            self.text.replace_range(e.range.clone(), &e.after);
            for a in self.anchors.values_mut() {
                a.transform(e);
            }
        }
        self.reindex();
        self.history.push_done(tx);
        true
    }

    // ---------------------------------------------------------- anchors

    pub fn anchor(&mut self, offset: usize) -> u64 {
        let id = self.next_anchor;
        self.next_anchor += 1;
        self.anchors.insert(id, Anchor::new(offset));
        id
    }

    pub fn anchor_offset(&self, id: u64) -> Option<usize> {
        self.anchors.get(&id).map(|a| a.offset)
    }

    // ---------------------------------------------------------- intents

    /// What to put between the previous argument and a newly inserted one.
    ///
    /// A call whose argument list already spans lines gets a newline and the
    /// indentation of the line the insertion lands on; a single-line call stays
    /// on its line. Insertion used to always use a space, which was valid and
    /// ugly, and the GUI now adds `xlim`/`ylim` on the first frame of every pan.
    fn separator(&self, call_start: usize, at: usize) -> String {
        if !self.text[call_start..at].contains('\n') {
            return " ".to_string();
        }
        let line_start = self.text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let indent: String = self.text[line_start..at]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        format!("\n{indent}")
    }

    /// Widen a range to swallow the comma that separated it from its
    /// neighbours, so removing an argument does not leave `a,, c`.
    fn with_separator(&self, range: Range<usize>) -> Range<usize> {
        let after: usize = self.text[range.end..]
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .filter(|(_, c)| *c == ',')
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if after > 0 {
            return range.start..range.end + after;
        }
        // Last argument: take the comma *before* it instead, plus the
        // whitespace that came with it.
        let head = &self.text[..range.start];
        let trimmed = head.trim_end();
        if trimmed.ends_with(',') {
            return trimmed.len() - 1..range.end;
        }
        range
    }

    fn resolve(&self, intent: &Intent) -> Result<AppliedEdit, String> {
        if let Some(value) = intent.value() {
            check_expr(value)?;
        }
        match intent {
            Intent::SetNamedArg { node, param, value } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                let arg = call
                    .named
                    .iter()
                    .find(|a| &a.name == param)
                    .ok_or_else(|| format!("`{}` has no argument `{param}`", call.callee))?;
                Ok(AppliedEdit {
                    range: arg.value.clone(),
                    before: self.text[arg.value.clone()].to_string(),
                    after: value.clone(),
                })
            }
            Intent::InsertNamedArg { node, param, value } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                if call.named.iter().any(|a| &a.name == param) {
                    return Err(format!("`{param}` already present"));
                }
                // Insert directly after the last existing argument, not before
                // the closing paren -- a call written with a trailing comma and
                // the paren on its own line otherwise yields `,\n, param: v)`.
                let last_end = call
                    .named
                    .iter()
                    .map(|a| a.value.end)
                    .chain(call.positional.iter().map(|p| p.range.end))
                    .max();
                match last_end {
                    Some(at) => Ok(AppliedEdit {
                        range: at..at,
                        before: String::new(),
                        after: format!(",{}{param}: {value}", self.separator(call.range.start, at)),
                    }),
                    None => {
                        // Empty argument list: insert just inside the parens.
                        let open = self.text[call.range.clone()]
                            .find('(')
                            .map(|i| call.range.start + i + 1)
                            .ok_or("call has no argument list")?;
                        Ok(AppliedEdit {
                            range: open..open,
                            before: String::new(),
                            after: format!("{param}: {value}"),
                        })
                    }
                }
            }
            Intent::RemoveNamedArg { node, param } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                let arg = call
                    .named
                    .iter()
                    .find(|a| &a.name == param)
                    .ok_or_else(|| format!("`{}` has no argument `{param}`", call.callee))?;
                // The name starts before the value; take the whole `name: value`
                // and the separator that goes with it.
                let start = self.text[call.range.start..arg.value.start]
                    .rfind(param)
                    .map(|i| call.range.start + i)
                    .ok_or("argument name not found")?;
                let range = self.with_separator(start..arg.value.end);
                Ok(AppliedEdit {
                    range: range.clone(),
                    before: self.text[range].to_string(),
                    after: String::new(),
                })
            }
            Intent::SetPositionalArg { node, index, value } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                let arg = call.positional.get(*index).ok_or_else(|| {
                    format!("`{}` has no positional argument {index}", call.callee)
                })?;
                Ok(AppliedEdit {
                    range: arg.range.clone(),
                    before: arg.text.clone(),
                    after: value.clone(),
                })
            }
            Intent::SetArrayElement {
                node,
                arg,
                element,
                value,
            } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                let slot = call
                    .positional
                    .get(*arg)
                    .ok_or_else(|| format!("`{}` has no positional argument {arg}", call.callee))?;
                let at = slot.elements.get(*element).ok_or_else(|| {
                    if slot.elements.is_empty() {
                        format!("argument {arg} is not a literal array")
                    } else {
                        format!("argument {arg} has no element {element}")
                    }
                })?;
                Ok(AppliedEdit {
                    range: at.clone(),
                    before: self.text[at.clone()].to_string(),
                    after: value.clone(),
                })
            }
            Intent::InsertPositionalArg { node, value } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                // Same rule as a named insertion: after the last argument, not
                // before the closing paren.
                let last_end = call
                    .named
                    .iter()
                    .map(|a| a.value.end)
                    .chain(call.positional.iter().map(|p| p.range.end))
                    .max();
                match last_end {
                    Some(at) => Ok(AppliedEdit {
                        range: at..at,
                        before: String::new(),
                        after: format!(",{}{value}", self.separator(call.range.start, at)),
                    }),
                    None => {
                        let open = self.text[call.range.clone()]
                            .find('(')
                            .map(|i| call.range.start + i + 1)
                            .ok_or("call has no argument list")?;
                        Ok(AppliedEdit {
                            range: open..open,
                            before: String::new(),
                            after: value.clone(),
                        })
                    }
                }
            }
            Intent::RemoveNode { node } => {
                let call = self
                    .call(*node)
                    .ok_or_else(|| format!("no call site {node}"))?;
                // Take the separating comma too: leaving `a,, c` behind was a
                // known gap, and it does not reparse as the same argument list.
                let range = self.with_separator(call.range.clone());
                Ok(AppliedEdit {
                    range: range.clone(),
                    before: self.text[range].to_string(),
                    after: String::new(),
                })
            }
            Intent::ReplaceRange { range, value } => Ok(AppliedEdit {
                range: range.clone(),
                before: self.text[range.clone()].to_string(),
                after: value.clone(),
            }),
        }
    }
}

/// Refuse a value that would not reparse.
///
/// Every consumer builds values as text -- a widget formats a number, an agent
/// pastes a string through MCP -- and a single unbalanced paren would leave the
/// user's manuscript broken in a way that is hard to attribute to lilook. This
/// is the one place to catch it, and it is cheap: parsing an argument value is
/// microseconds against the reparse the edit triggers anyway.
///
/// It checks syntax, not semantics: `stroke: 3` parses and is rejected later by
/// typst, with a diagnostic that points at the argument. That is the right
/// division -- lilook does not own lilaq's type rules.
pub fn check_expr(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("empty value".into());
    }
    // Parsed *in an argument list*, which is where the value is going. Checking
    // it as free-standing code would accept `let x = 1` -- valid code, invalid
    // as an argument -- and would accept `1 2` as two expressions where the
    // call site can only take one.
    let node = typst_syntax::parse_code(&format!("__lilook_check({value})"));
    let (errors, _) = node.errors_and_warnings();
    match errors.first() {
        Some(e) => Err(format!(
            "`{value}` is not a valid argument value: {}",
            e.message
        )),
        None => Ok(()),
    }
}

/// Element ranges of a literal array node, or empty for anything else.
///
/// Typst writes a one-element array as `(1,)`, so the trailing comma is part of
/// the syntax rather than a stray token; filtering by kind rather than by
/// position is what keeps that case right.
fn array_elements(node: &LinkedNode) -> Vec<Range<usize>> {
    if node.kind() != SyntaxKind::Array {
        return vec![];
    }
    node.children()
        .filter(|c| {
            !c.kind().is_trivia()
                && !matches!(
                    c.kind(),
                    SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma
                )
        })
        .map(|c| c.range())
        .collect()
}

/// The smallest node covering exactly this byte range.
fn find_node<'a>(node: &LinkedNode<'a>, range: &Range<usize>) -> Option<LinkedNode<'a>> {
    if node.range() == *range {
        return Some(node.clone());
    }
    if !(node.range().start <= range.start && node.range().end >= range.end) {
        return None;
    }
    node.children().find_map(|c| find_node(&c, range))
}

fn find_binding(node: &LinkedNode, text: &str, name: &str, out: &mut Option<Range<usize>>) {
    if out.is_some() {
        return;
    }
    if node.kind() == SyntaxKind::LetBinding {
        let bound = node
            .children()
            .find(|c| matches!(c.kind(), SyntaxKind::Ident | SyntaxKind::Closure))
            .and_then(|c| match c.kind() {
                SyntaxKind::Ident => Some(c.range()),
                // `#let f(x) = ..` binds the closure's own name.
                _ => c
                    .children()
                    .find(|g| g.kind() == SyntaxKind::Ident)
                    .map(|g| g.range()),
            });
        if bound.is_some_and(|r| &text[r] == name) {
            *out = Some(node.range());
            return;
        }
    }
    for child in node.children() {
        find_binding(&child, text, name, out);
    }
}

/// Names Typst provides, on top of the colour and alignment constants the
/// editability table already lists. Not exhaustive -- an unknown name is
/// reported as free, which errs towards carrying a binding that was not needed
/// rather than pasting something that will not compile.
const BUILTIN_SCOPE: &[&str] = &[
    "calc",
    "sys",
    "std",
    "range",
    "int",
    "float",
    "str",
    "bool",
    "array",
    "dictionary",
    "type",
    "repr",
    "panic",
    "assert",
    "eval",
    "measure",
    "layout",
    "here",
    "locate",
    "query",
    "counter",
    "state",
    "context",
    "text",
    "par",
    "page",
    "figure",
    "image",
    "table",
    "grid",
    "stack",
    "place",
    "rect",
    "circle",
    "line",
    "path",
    "polygon",
    "box",
    "block",
    "pad",
    "align",
    "move",
    "scale",
    "rotate",
    "hide",
    "raw",
    "link",
    "label",
    "ref",
    "cite",
    "bibliography",
    "heading",
    "list",
    "enum",
    "terms",
    "emph",
    "strong",
    "sub",
    "super",
    "underline",
    "overline",
    "strike",
    "highlight",
    "smallcaps",
    "upper",
    "lower",
    "datetime",
    "duration",
    "symbol",
    "emoji",
    "color",
    "gradient",
    "pattern",
    "tiling",
    "stroke",
    "length",
    "angle",
    "ratio",
    "relative",
    "fraction",
    "alignment",
    "direction",
    "selector",
    "regex",
    "version",
    "bytes",
    "content",
    "arguments",
    "function",
    "module",
    "metadata",
    "numbering",
    "lorem",
    "read",
    "csv",
    "json",
    "toml",
    "yaml",
    "xml",
    "cbor",
    "plugin",
    "curve",
    "polygon",
];

fn free_idents(node: &LinkedNode, text: &str, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match node.kind() {
        SyntaxKind::Ident => {
            let name = &text[node.range()];
            let parent = node.parent();
            let is_field = parent.is_some_and(|p| {
                // `lq.plot`: the tail is a field of `lq`, not a name in scope.
                p.kind() == SyntaxKind::FieldAccess
                    && p.children().next().map(|c| c.range()) != Some(node.range())
            });
            let is_arg_name = parent.is_some_and(|p| {
                p.kind() == SyntaxKind::Named
                    && p.children().next().map(|c| c.range()) == Some(node.range())
            });
            if !is_field
                && !is_arg_name
                && !bound.iter().any(|b| b == name)
                && !BUILTIN_IDENTS.contains(&name)
                && !BUILTIN_SCOPE.contains(&name)
                && !matches!(name, "none" | "auto" | "true" | "false")
            {
                out.push(name.to_string());
            }
            return;
        }
        // A closure binds its parameters for its whole body, and a `#let` binds
        // its name for everything after it in the same scope. Both are handled
        // by pushing onto `bound` before descending.
        SyntaxKind::Closure | SyntaxKind::LetBinding => {
            let before = bound.len();
            for child in node.children() {
                if matches!(child.kind(), SyntaxKind::Params | SyntaxKind::Destructuring) {
                    collect_bound(&child, text, bound);
                } else if child.kind() == SyntaxKind::Ident && node.kind() == SyntaxKind::LetBinding
                {
                    bound.push(text[child.range()].to_string());
                    continue;
                }
                free_idents(&child, text, bound, out);
            }
            // A `#let` stays bound for the rest of the fragment; a closure's
            // parameters do not outlive it.
            if node.kind() == SyntaxKind::Closure {
                bound.truncate(before);
            }
            return;
        }
        _ => {}
    }
    for child in node.children() {
        free_idents(&child, text, bound, out);
    }
}

fn collect_bound(node: &LinkedNode, text: &str, bound: &mut Vec<String>) {
    if node.kind() == SyntaxKind::Ident {
        bound.push(text[node.range()].to_string());
    }
    for child in node.children() {
        collect_bound(&child, text, bound);
    }
}

/// Find `#show: <mod>.theme.<name>` and `#show: <ident>`.
fn collect_themes(node: &LinkedNode, out: &mut Vec<Theme>, doc: &Document) {
    if node.kind() == SyntaxKind::ShowRule {
        // The transform is the last expression in the rule; anything before it
        // is the `show`, the selector and the colon.
        if let Some(t) = node.children().rfind(|c| !c.kind().is_trivia()) {
            let text = doc.text[t.range()].trim().to_string();
            // The `#` is not part of the `ShowRule` node, and both replacing a
            // rule and removing one need it: without it a switch wrote `##show`
            // and a removal left a stray hash behind.
            let mut range = node.range();
            if doc.text[..range.start].ends_with('#') {
                range.start -= 1;
            }
            let mut document_level = true;
            let mut at = node.parent();
            while let Some(p) = at {
                if matches!(p.kind(), SyntaxKind::CodeBlock | SyntaxKind::ContentBlock) {
                    document_level = false;
                    break;
                }
                at = p.parent();
            }
            let named = |n: &str| Theme {
                document_level,
                range: range.clone(),
                transform: t.range(),
                name: n.to_string(),
                local: false,
            };
            match t.kind() {
                // `lq.theme.ocean`, whatever the module is aliased to.
                SyntaxKind::FieldAccess => {
                    let mut parts = text.rsplit('.');
                    if let (Some(name), Some("theme")) = (parts.next(), parts.next()) {
                        out.push(named(name));
                    }
                }
                SyntaxKind::Ident if doc.binds_a_theme(&text) => {
                    out.push(Theme {
                        local: true,
                        ..named(&text)
                    });
                }
                _ => {}
            }
        }
    }
    for child in node.children() {
        collect_themes(&child, out, doc);
    }
}

fn collect_set_rules(
    node: &LinkedNode,
    file_len: usize,
    out: &mut Vec<SetRule>,
    calls: &[CallSite],
) {
    if node.kind() == SyntaxKind::ShowRule {
        // `#show: lq.set-tick(..)` -- the transform is the last expression, and
        // it is already in the call index, so the rule reuses its node id and
        // every existing intent works on it unchanged.
        if let Some(call) = node
            .children()
            .rfind(|c| c.kind() == SyntaxKind::FuncCall)
            .and_then(|c| {
                let r = c.range();
                calls.iter().find(|k| k.range == r)
            })
        {
            if let Some(element) = call.short_name().strip_prefix("set-") {
                // The rule governs from here to the end of the block it is in.
                let mut at = node.parent();
                let mut scope_end = file_len;
                let mut document_level = true;
                while let Some(p) = at {
                    if matches!(p.kind(), SyntaxKind::CodeBlock | SyntaxKind::ContentBlock) {
                        scope_end = p.range().end;
                        document_level = false;
                        break;
                    }
                    at = p.parent();
                }
                out.push(SetRule {
                    node: call.id,
                    element: element.to_string(),
                    scope: node.range().start..scope_end.max(node.range().end),
                    document_level,
                });
            }
        }
    }
    for child in node.children() {
        collect_set_rules(&child, file_len, out, calls);
    }
}

fn classify(node: &SyntaxNode, text: &str, range: &Range<usize>) -> Editability {
    match node.kind() {
        SyntaxKind::Int
        | SyntaxKind::Float
        | SyntaxKind::Numeric
        | SyntaxKind::Str
        | SyntaxKind::Bool
        | SyntaxKind::None
        | SyntaxKind::Auto => Editability::Literal,
        SyntaxKind::Ident => {
            if BUILTIN_IDENTS.contains(&&text[range.clone()]) {
                Editability::Builtin
            } else {
                Editability::Binding
            }
        }
        SyntaxKind::Array | SyntaxKind::Dict | SyntaxKind::ContentBlock => Editability::Literal,
        // `-1` parses as a *unary negation* of `1`, not as a literal token, so
        // without this every negative number in a lilaq call was opaque and got
        // a raw source editor instead of a number control -- `place(1, -1)`,
        // `margin: -2pt`, any coordinate below zero. `split_numeric` has always
        // read the sign; the classifier simply never let it.
        SyntaxKind::Unary => {
            let numeric = node.children().any(|c| {
                matches!(
                    c.kind(),
                    SyntaxKind::Int | SyntaxKind::Float | SyntaxKind::Numeric
                )
            });
            if numeric {
                Editability::Literal
            } else {
                Editability::Opaque
            }
        }
        // A call to a builtin constructor is a value literal in everything but
        // syntax: `rgb("#4c72b0")` and `luma(50%)` are colours a swatch can
        // edit. A call to anything else -- `calc.sin(x)`, `lq.linspace(..)` --
        // stays opaque, because rewriting it would mean rewriting a program.
        SyntaxKind::FuncCall => {
            let callee = node.children().next();
            match callee.map(|c| (c.kind(), c.leaf_text())) {
                Some((SyntaxKind::Ident, name)) if BUILTIN_IDENTS.contains(&name.as_str()) => {
                    Editability::Builtin
                }
                _ => Editability::Opaque,
            }
        }
        _ => Editability::Opaque,
    }
}

fn collect(
    node: &LinkedNode,
    text: &str,
    out: &mut Vec<CallSite>,
    id: &mut usize,
    in_generator: bool,
    parent: Option<usize>,
) {
    // A closure or spread below this point means any call inside is generated.
    let generator = in_generator
        || matches!(
            node.kind(),
            SyntaxKind::Closure | SyntaxKind::Spread | SyntaxKind::ForLoop
        );
    let mut parent = parent;

    if node.kind() == SyntaxKind::FuncCall {
        // `SyntaxNode::into_text` went away in typst-syntax 0.15; slicing the
        // buffer by the node's range is equivalent and skips the clone.
        let callee = node
            .children()
            .next()
            .map(|c| text[c.range()].to_string())
            .unwrap_or_default();
        if callee.starts_with("lq.") {
            let mut named = vec![];
            let mut positional = vec![];
            for child in node.children() {
                if child.kind() != SyntaxKind::Args {
                    continue;
                }
                for arg in child.children() {
                    match arg.kind() {
                        SyntaxKind::Named => {
                            let name = arg
                                .children()
                                .next()
                                .map(|x| text[x.range()].to_string())
                                .unwrap_or_default();
                            if let Some(v) = arg
                                .children()
                                .rfind(|c| !c.kind().is_trivia() && c.kind() != SyntaxKind::Colon)
                            {
                                let r = v.range();
                                named.push(NamedArg {
                                    name,
                                    editability: classify(v.get(), text, &r),
                                    text: text[r.clone()].to_string(),
                                    value: r,
                                });
                            }
                        }
                        k if !k.is_trivia()
                            && !matches!(
                                k,
                                SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma
                            ) =>
                        {
                            let r = arg.range();
                            positional.push(PositionalArg {
                                editability: classify(arg.get(), text, &r),
                                text: text[r.clone()].to_string(),
                                elements: array_elements(&arg),
                                range: r,
                            });
                        }
                        _ => {}
                    }
                }
            }
            out.push(CallSite {
                id: *id,
                callee,
                range: node.range(),
                named,
                positional,
                generated: generator,
                parent,
            });
            // Anything nested below this call belongs to it.
            parent = Some(*id);
            *id += 1;
        }
    }
    for child in node.children() {
        collect(&child, text, out, id, generator, parent);
    }
}

/// Walk the tree once, emitting a span for anything worth colouring.
fn collect_tokens(node: &LinkedNode, out: &mut Vec<(Range<usize>, Token)>) {
    let token = match node.kind() {
        SyntaxKind::LineComment | SyntaxKind::BlockComment => Some(Token::Comment),
        SyntaxKind::Str => Some(Token::Str),
        SyntaxKind::Int | SyntaxKind::Float | SyntaxKind::Numeric => Some(Token::Number),
        // The hash belongs with the keyword it introduces: `#let` reads as one
        // word, and colouring only the `let` looks like a mistake.
        SyntaxKind::Hash
        | SyntaxKind::Let
        | SyntaxKind::Show
        | SyntaxKind::Set
        | SyntaxKind::Import
        | SyntaxKind::Include
        | SyntaxKind::If
        | SyntaxKind::Else
        | SyntaxKind::For
        | SyntaxKind::While
        | SyntaxKind::Return => Some(Token::Keyword),
        _ => None,
    };
    if let Some(t) = token {
        out.push((node.range(), t));
        // A comment or a string has nothing inside it worth colouring
        // separately, and descending would split it.
        if matches!(t, Token::Comment | Token::Str) {
            return;
        }
    }
    // The name a `#let` binds, so it reads as a definition rather than as any
    // other identifier.
    if node.kind() == SyntaxKind::LetBinding {
        if let Some(name) = node
            .children()
            .find(|c| matches!(c.kind(), SyntaxKind::Ident | SyntaxKind::Closure))
        {
            let ident = match name.kind() {
                SyntaxKind::Closure => name.children().find(|c| c.kind() == SyntaxKind::Ident),
                _ => Some(name),
            };
            if let Some(i) = ident {
                out.push((i.range(), Token::Binding));
            }
        }
    }
    if node.kind() == SyntaxKind::FuncCall {
        if let Some(callee) = node.children().next() {
            out.push((callee.range(), Token::Call));
        }
    }
    for child in node.children() {
        collect_tokens(&child, out);
    }
}
