//! lilook's egui layer: the schema-driven inspector and the figure canvas.
//!
//! Depends on `egui` only -- never `eframe`, never the compiler -- so the whole
//! surface runs and is testable headlessly. The windowing shell is a separate,
//! thin binary. This split has already paid for itself once: the crate crossed
//! six egui releases (0.29 -> 0.35) without a line changing, while the shell
//! absorbed the whole `eframe::App` break.
//!
//! Everything here emits `UiEvent`s rather than mutating a document. Mapping
//! events onto transactions is the caller's job, which keeps drag-coalescing
//! policy in one place and lets another frontend consume the same vocabulary.

pub mod canvas;
pub mod highlight;
pub mod inspector;
pub mod value;
pub mod viewport;

pub use canvas::{Canvas, CanvasEvent, CanvasInput, CanvasOutput, PageTexture};
pub use highlight::{layout_job, token_color};
pub use inspector::{
    add_argument_choice_id, control_for, refine, widget_control, Context, Control, Inspector,
    SlotSource, UiEvent,
};
pub use value::{num, parse_color, parse_stroke, split_numeric, Stroke};
pub use viewport::{stack_pages, stacked_size, PageBox, Viewport};
