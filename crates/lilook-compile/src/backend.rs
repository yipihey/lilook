//! Compile and rasterise, in process.
//!
//! Shelling out to the `typst` binary costs ~570 ms per invocation, almost all
//! of it process startup, which rules it out for anything attached to a
//! pointer. In process, the same figure recompiles in 20 ms after a style edit
//! and rasterises in under a millisecond.

use std::collections::HashMap;
use web_time::Instant;

use lilook_core::scene::{Bounds, Scene};
use lilook_core::Document;
use typst_layout::PagedDocument;

use crate::probe;
use crate::world::LilookWorld;

pub use lilook_core::render::{Image, Page, Render};

/// The compiler, over whatever supplies files.
pub struct Backend<L: typst_kit::files::FileLoader + Send + Sync = crate::files::MemoryFiles> {
    world: LilookWorld<L>,
    /// The most recent document that compiled, kept so probe queries and the
    /// canvas can still work while the buffer is transiently broken.
    last: Option<PagedDocument>,
}

#[cfg(feature = "system")]
impl Backend<typst_kit::files::SystemFiles> {
    pub fn new(root: impl AsRef<std::path::Path>, text: impl Into<String>) -> Self {
        Backend {
            world: LilookWorld::new(root, text),
            last: None,
        }
    }
}

impl<L: typst_kit::files::FileLoader + Send + Sync> Backend<L> {
    /// A backend over any file source: a bundle in memory, a host's storage.
    pub fn with_loader(loader: L, text: impl Into<String>) -> Self {
        Backend {
            world: LilookWorld::with_loader(loader, text),
            last: None,
        }
    }

    /// Register a font from raw bytes. See `LilookWorld::add_font_data`.
    pub fn add_font_data(&mut self, data: impl Into<Vec<u8>>) -> usize {
        self.world.add_font_data(data)
    }

    pub fn world(&self) -> &LilookWorld<L> {
        &self.world
    }

    /// Mutable access to whatever supplies files -- how the browser build adds a
    /// file the user dropped onto the page.
    pub fn loader_mut(&mut self) -> &mut L {
        self.world.loader_mut()
    }

    /// Ask the compiler about a file. See `LilookWorld::query`.
    ///
    /// Note this does *not* touch `self.last`: a query's document has no figure
    /// in it, and the canvas must keep drawing the one that does.
    pub fn query(
        &mut self,
        expr: &str,
    ) -> (
        Option<lilook_core::data::Answer>,
        Vec<lilook_core::Diagnostic>,
    ) {
        self.world.query(expr)
    }

    /// Every file the last compile read. See `LilookWorld::dependencies`.
    pub fn dependencies(&mut self) -> Vec<lilook_core::DataFile> {
        self.world.dependencies()
    }

    pub fn document(&self) -> Option<&PagedDocument> {
        self.last.as_ref()
    }

    /// Compile `source` and rasterise every page at `pixel_per_pt`.
    pub fn render(&mut self, source: &str, pixel_per_pt: f32) -> Render {
        let t = Instant::now();
        self.world.set_source(source);
        let (doc, diagnostics) = self.world.compile();
        let compile_time = t.elapsed();

        let t = Instant::now();
        let pages = doc
            .as_ref()
            .map(|d| rasterize(d, pixel_per_pt))
            .unwrap_or_default();
        let render_time = t.elapsed();

        if let Some(d) = doc {
            self.last = Some(d);
        }

        // comemo's cache is what makes the warm path fast, so evict lazily:
        // anything untouched for 20 compiles goes. typst-cli uses 10 in watch
        // mode; a drag produces far more compiles per second than a file save,
        // and dropping a live figure's layout mid-drag is the expensive mistake.
        comemo::evict(20);

        Render {
            pages,
            diagnostics,
            pixel_per_pt,
            compile_time,
            render_time,
        }
    }
}

/// Axis limits recovered per figure, fed back into the next probe placement.
/// Keeping them is what makes the steady state one compile instead of two:
/// probes at 10%/90% of the limits lilaq actually used cannot fall outside them.
pub type Hints = HashMap<usize, Bounds>;

impl<L: typst_kit::files::FileLoader + Send + Sync> Backend<L> {
    /// Compile with probes injected, and recover a `Scene` per diagram.
    ///
    /// The probes go into a derived copy of the buffer; the user's text is never
    /// touched. If a figure's probes turn out to have landed outside its axis
    /// limits -- which leaves the scale right but displaces the origin -- this
    /// compiles once more with probes derived from what the first pass learnt.
    pub fn render_scenes(
        &mut self,
        doc: &Document,
        pixel_per_pt: f32,
        hints: &mut Hints,
    ) -> (Render, Vec<Scene>) {
        let (source, injection) = probe::inject(doc, hints);
        let mut render = self.render(&source, pixel_per_pt);

        // A page too large to rasterise means the *probes* blew the layout up,
        // not the user's figure: placing a number like `0.1` on a `datetime` axis
        // makes an `auto`-sized page grow without bound. Retry without the scale
        // probes -- the figure then lays out and draws normally, and what is lost
        // is a data<->page transform that was never recoverable for that axis.
        if render.pages.iter().any(|p| p.image.width == 0) {
            let (plain, plain_injection) = probe::inject_with(doc, hints, false);
            let retry = self.render(&plain, pixel_per_pt);
            if retry.pages.iter().all(|p| p.image.width > 0) {
                let scenes = self
                    .document()
                    .map(|d| probe::scenes(d, &plain_injection))
                    .unwrap_or_default();
                return (retry, scenes);
            }
            render = retry;
        }

        let mut scenes = self
            .document()
            .map(|d| probe::scenes(d, &injection))
            .unwrap_or_default();

        let stale: Vec<&Scene> = scenes
            .iter()
            .filter(|s| !probe::probes_were_in_range(s, &injection))
            .collect();
        if stale.is_empty() {
            learn(hints, &scenes);
            return (render, scenes);
        }
        for s in stale {
            hints.insert(s.figure, limits(s));
        }

        let (source, injection) = probe::inject(doc, hints);
        let render = self.render(&source, pixel_per_pt);
        scenes = self
            .document()
            .map(|d| probe::scenes(d, &injection))
            .unwrap_or_default();
        learn(hints, &scenes);
        (render, scenes)
    }
}

fn limits(s: &Scene) -> Bounds {
    Bounds {
        x: (s.transform.x.min, s.transform.x.max),
        y: (s.transform.y.min, s.transform.y.max),
    }
}

fn learn(hints: &mut Hints, scenes: &[Scene]) {
    for s in scenes {
        hints.insert(s.figure, limits(s));
    }
}

/// The most pixels lilook will ask for in one page: 64 megapixels, or 256 MB of
/// RGBA. Far beyond any figure, and far below what makes the allocator fail.
pub(crate) const MAX_RASTER_PIXELS: f64 = 64.0 * 1024.0 * 1024.0;

fn rasterize(doc: &PagedDocument, pixel_per_pt: f32) -> Vec<Page> {
    let opts = typst_render::RenderOptions {
        pixel_per_pt: typst::utils::Scalar::new(pixel_per_pt as f64),
        render_bleed: false,
    };
    doc.pages()
        .iter()
        .enumerate()
        .map(|(index, page)| {
            let size = page.frame.size();
            let size_pt = (size.x.to_pt(), size.y.to_pt());
            // `typst_render::render` allocates a pixmap and *unwraps* it, so an
            // implausibly large page panics inside the renderer with no way to
            // catch it. The only guard is not calling it.
            //
            // This is not hypothetical: a `datetime` axis blows the layout up,
            // because lilook's own scale probes are placed at numeric data
            // coordinates and lilaq maps those onto a calendar. An
            // `auto`-sized page then grows without bound and the whole editor
            // died on a document that compiles perfectly well by itself.
            let pixels = (size_pt.0 * pixel_per_pt as f64) * (size_pt.1 * pixel_per_pt as f64);
            if !pixels.is_finite() || pixels > MAX_RASTER_PIXELS {
                return Page {
                    index,
                    size_pt,
                    image: Image {
                        width: 0,
                        height: 0,
                        rgba: vec![],
                    },
                };
            }
            let pixmap = typst_render::render(page, &opts);
            Page {
                index,
                size_pt,
                image: Image {
                    width: pixmap.width(),
                    height: pixmap.height(),
                    rgba: pixmap.take(),
                },
            }
        })
        .collect()
}
