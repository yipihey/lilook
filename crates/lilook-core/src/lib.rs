//! lilook-core -- document model, intents and history for lilaq figures.
//!
//! The Typst source is the model. There is no intermediate format: every GUI
//! action becomes a surgical byte-range replacement on the source, and undo is
//! a text-edit history rather than a widget-tree history.
//!
//! Nothing in this crate knows about impress, implore, egui or Swift.

pub mod compile;
pub mod data;
pub mod doc;
pub mod edit;
pub mod events;
pub mod intent;
pub mod policy;
pub mod render;
pub mod scene;
pub mod schema;
pub mod session;

pub use compile::{AxisMap, AxisScale, CliCompiler, Compiler, Hit, Transform};
pub use data::{
    binding_name_for, binding_source, column_source, columns_of, csv_binding_source,
    csv_column_source, data_array_source, data_num, gesture_num, is_plain_identifier, parse_text,
    split_numeric, string_literal, Answer, Columns, DataFile, FileRoot, SourceKind, TextShape,
    TooManyValues, MAX_EMBEDDED_VALUES,
};
pub use doc::{
    check_expr, series_shape_of, Axis, CallSite, Cursor, Document, Editability, Figure, NamedArg,
    PositionalArg, SeriesShape, SetRule, Theme, Token, XY_SERIES,
};
pub use edit::{minimal_replacement, Anchor, AppliedEdit, CoalesceKey, History, Transaction};
pub use events::{CanvasEvent, UiEvent};
pub use intent::Intent;
pub use policy::{
    seed_for_test, sentinel_of, widget_control, Control, COLORMAPS, CYCLES, FIGURE_TEXT_SIZES,
    FIGURE_WIDTHS,
};
pub use render::{Blame, Diagnostic, Image, Page, Render, Severity};
pub use scene::{Bounds, Scene, SceneHit, SeriesGeom};
pub use schema::{ElementSchema, FunctionSchema, ParamSchema, Schema};
pub use session::{
    Action, Clip, Completion, Dropped, Extraction, Hint, Link, Requests, Session, Signature,
    SlotSource, IDLE_COMMIT_SECONDS,
};
