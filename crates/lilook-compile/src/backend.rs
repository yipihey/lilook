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

    pub fn world(&self) -> &LilookWorld<L> {
        &self.world
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
        let render = self.render(&source, pixel_per_pt);
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

fn rasterize(doc: &PagedDocument, pixel_per_pt: f32) -> Vec<Page> {
    let opts = typst_render::RenderOptions {
        pixel_per_pt: typst::utils::Scalar::new(pixel_per_pt as f64),
        render_bleed: false,
    };
    doc.pages()
        .iter()
        .enumerate()
        .map(|(index, page)| {
            let pixmap = typst_render::render(page, &opts);
            let size = page.frame.size();
            Page {
                index,
                size_pt: (size.x.to_pt(), size.y.to_pt()),
                image: Image {
                    width: pixmap.width(),
                    height: pixmap.height(),
                    rgba: pixmap.take(),
                },
            }
        })
        .collect()
}
