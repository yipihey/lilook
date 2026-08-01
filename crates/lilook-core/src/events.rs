//! What a frontend asks the session to do.
//!
//! These are the *gesture vocabulary*: what a canvas or an inspector reports,
//! in terms of call sites and data coordinates, with no pixels and no toolkit
//! anywhere in them. They live in the core rather than beside the egui widgets
//! that happen to produce them today, because they are the interface a Swift
//! view or an MCP tool speaks too -- a drag is a drag whether a mouse or an
//! agent asked for it.
//!
//! Mapping them onto intents and transactions is [`crate::Session`]'s job.

/// What the canvas asks the shell to do. Mapping these onto intents and
/// transactions is the shell's job, exactly as with `UiEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasEvent {
    /// A call site was clicked: a series, or a diagram's background.
    Select(usize),
    /// A gesture started: open a transaction.
    Begin,
    /// It finished: commit, so the whole gesture is one undo step.
    Commit,
    /// New axis limits for a diagram, from a pan or a zoom.
    SetLimits {
        figure: usize,
        x: (f64, f64),
        y: (f64, f64),
    },
    /// A data point was dragged to a new position.
    MovePoint {
        node: usize,
        index: usize,
        to: (f64, f64),
    },
    /// A rule line was dragged. Its coordinate is a whole positional argument,
    /// not an element of an array, so this is a different edit from a point move.
    MoveRule {
        node: usize,
        /// Which positional argument -- `hlines(1, 2, 3)` has three.
        slot: usize,
        to: f64,
    },
    /// A legend was dragged. lilaq names nine places a legend may sit, so the
    /// drag snaps to the nearest rather than inventing an offset -- a legend half
    /// a millimetre off a corner reads as a mistake.
    MoveLegend { figure: usize, to: (f64, f64) },
    /// A title or an axis label was nudged. Unlike a legend these have no named
    /// places, so the drag becomes `dx`/`dy` in points.
    MoveDecoration {
        figure: usize,
        kind: crate::scene::Decoration,
        dx: f64,
        dy: f64,
    },
    /// The diagram was resized by its frame. Sizes are in typographic points --
    /// `width` and `height` on `lq.diagram` *are* the data area's dimensions,
    /// which is what makes dragging the axis frame mean what it looks like it
    /// means. `None` is an axis the gesture did not touch.
    SetSize {
        figure: usize,
        width_pt: Option<f64>,
        height_pt: Option<f64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// Pointer went down on a continuous control: open a coalescing transaction.
    Begin { node: usize, param: String },
    Set {
        node: usize,
        param: String,
        value: String,
    },
    /// Pointer released: commit, making the whole drag one undo step.
    Commit,
    /// Add an argument the call does not have yet.
    Insert {
        node: usize,
        param: String,
        value: String,
    },
    /// Remove an argument, returning the parameter to its default.
    Remove { node: usize, param: String },
    /// Write the evaluated data of a computed slot into the source, so its
    /// points become editable.
    Materialize { node: usize, index: usize },
    /// User asked to jump to the `#let` a value is bound to.
    GoToBinding {
        node: usize,
        param: String,
        name: String,
    },
    /// Keep a value in the user's own library, under a name.
    ///
    /// Not an edit: the document is untouched until the saved value is *chosen*,
    /// which is an ordinary `Set`. A library is a shelf of offers, and putting
    /// something on the shelf changes no figure -- see [`crate::Prefs`].
    SavePref {
        kind: crate::Kind,
        name: String,
        value: String,
    },
    /// Take one off the shelf. Also not an edit: a figure that already uses it
    /// carries the value, not the name.
    RemovePref { kind: crate::Kind, name: String },
}
