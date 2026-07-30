//! lilook-compile -- the in-process typst backend.
//!
//! Split out of `lilook-core` on purpose: `typst` pulls in a few hundred
//! transitive crates, and the document model, the CLI, the MCP server and the
//! FFI have no business compiling a typesetter. Core keeps its three
//! dependencies; anything that needs pixels or a laid-out frame comes here.
//!
//! `lilook_core::Compiler` stays the abstraction the *probe* path is written
//! against, so `recover_transform` works identically over the subprocess and
//! over this backend. Everything that would be wasteful to route through JSON
//! -- rasterised pages, per-series point data -- is returned directly instead.

#[cfg(feature = "system")]
pub mod actor;
pub mod backend;
pub mod files;
pub mod probe;
pub mod query;
pub mod world;

#[cfg(feature = "system")]
pub use actor::{CompileActor, Frame};
pub use backend::{Backend, Image, Page, Render};
pub use files::{MainOverlay, MemoryFiles};
pub use lilook_core::data::Answer;
pub use probe::{inject, scenes};
#[cfg(feature = "system")]
pub use world::root_for;
pub use world::{main_id, Diagnostic, LilookWorld, Severity};
