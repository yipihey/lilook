//! The intent vocabulary.
//!
//! Deliberately fine-grained: the GUI emits one intent per slider tick and two
//! per frame of a pan, and the transaction layer decides what becomes a single
//! undo step. The CLI and MCP wrappers each open and commit a transaction per
//! command, so they get atomic behaviour for free without the core having to
//! expose a coarser API.

use crate::edit::CoalesceKey;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Intent {
    /// Change the value of an existing named argument.
    SetNamedArg {
        node: usize,
        param: String,
        value: String,
    },
    /// Add a named argument that is not currently present.
    InsertNamedArg {
        node: usize,
        param: String,
        value: String,
    },
    /// Remove a named argument, and the comma that separated it.
    RemoveNamedArg { node: usize, param: String },
    /// Replace a whole positional argument -- a data slot.
    SetPositionalArg {
        node: usize,
        index: usize,
        value: String,
    },
    /// Replace one element of a literal array in a positional slot. This is
    /// what dragging a single data point comes down to.
    SetArrayElement {
        node: usize,
        /// Which positional slot: 0 is x, 1 is y for every `XY_SERIES` call.
        arg: usize,
        element: usize,
        value: String,
    },
    /// Append a positional argument -- how a pasted or duplicated series joins
    /// a diagram.
    InsertPositionalArg { node: usize, value: String },
    /// Delete a whole call site (a series, say).
    RemoveNode { node: usize },
    /// Escape hatch: replace an arbitrary byte range.
    #[serde(skip)]
    ReplaceRange { range: Range<usize>, value: String },
}

impl Intent {
    /// The Typst source this intent would write, if any. Validated once in
    /// `Document::resolve` so no consumer can put an unparsable value into the
    /// user's buffer.
    pub fn value(&self) -> Option<&str> {
        match self {
            Intent::SetNamedArg { value, .. }
            | Intent::InsertNamedArg { value, .. }
            | Intent::SetPositionalArg { value, .. }
            | Intent::InsertPositionalArg { value, .. }
            | Intent::SetArrayElement { value, .. } => Some(value),
            // `ReplaceRange` is the escape hatch: it may legitimately write a
            // fragment that is not an expression on its own.
            Intent::ReplaceRange { .. }
            | Intent::RemoveNamedArg { .. }
            | Intent::RemoveNode { .. } => None,
        }
    }

    /// Two intents with the same key, inside one open transaction, collapse
    /// into a single edit.
    ///
    /// The key names a *target*, not a transaction: one gesture routinely
    /// rewrites several. A pan sets `xlim` and `ylim` on every frame, and
    /// dragging a point sets an x element and a y element, so a per-transaction
    /// key would collapse neither.
    pub fn coalesce_key(&self) -> Option<CoalesceKey> {
        match self {
            Intent::SetNamedArg { node, param, .. } => Some(CoalesceKey {
                node: *node,
                param: param.clone(),
            }),
            Intent::SetPositionalArg { node, index, .. } => Some(CoalesceKey {
                node: *node,
                param: format!("#{index}"),
            }),
            Intent::SetArrayElement {
                node, arg, element, ..
            } => Some(CoalesceKey {
                node: *node,
                param: format!("#{arg}[{element}]"),
            }),
            _ => None,
        }
    }
}
