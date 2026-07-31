//! What a compile produced, as plain data.
//!
//! These types live in core rather than in the compile backend so that the
//! editor can consume a compiled figure without depending on a typesetter --
//! which is what lets one editor serve the desktop shell, the browser shell,
//! and anything else that can produce pixels for a source string.

use std::ops::Range;
use std::time::Duration;

/// Raw premultiplied RGBA, ready to become a texture. Deliberately not an
/// `egui::ColorImage`: the compile backend must not depend on a UI toolkit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub index: usize,
    /// Page size in typographic points, which is the space probe positions and
    /// the data<->page transform are expressed in.
    pub size_pt: (f64, f64),
    pub image: Image,
}

/// One diagnostic, with its span already resolved to a byte range in the main
/// buffer so the caller never needs a compiler to interpret it.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub range: Option<Range<usize>>,
    pub hints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Default)]
pub struct Render {
    pub pages: Vec<Page>,
    pub diagnostics: Vec<Diagnostic>,
    /// Points per pixel this was rasterised at, needed to map a click back into
    /// page space.
    pub pixel_per_pt: f32,
    pub compile_time: Duration,
    pub render_time: Duration,
}

impl Render {
    pub fn failed(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }
}

/// What in the document is responsible for a diagnostic that names no location.
///
/// lilaq validates inside its own package, so four of the six commonest failures
/// arrive with no span at all -- see `docs/findings.md`. The compiler can still
/// answer the question, just not by being asked directly: remove one thing, see
/// whether the error survives, and the removal that clears it names the culprit.
///
/// At ~4 ms a variant that is affordable on demand, and it produces the byte
/// range the diagnostic was missing. Blame is *evidence*, not a guess: the
/// document without this argument does not have this error.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Blame {
    /// The call site involved.
    pub node: usize,
    /// The named argument, when removing one argument was enough.
    pub argument: Option<String>,
    /// Where to point in the user's own buffer.
    pub range: std::ops::Range<usize>,
    /// What the user would call it.
    pub label: String,
}
