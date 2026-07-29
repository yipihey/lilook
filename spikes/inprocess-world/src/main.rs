//! Spike: in-process typst compile of a lilaq figure, measuring
//! cold / warm-edit recompile latency, probe query, and raster render.

use std::sync::Mutex;
use std::time::Instant;

use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime};
use typst_layout::PagedDocument;
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::introspection::Introspector as _;
use typst::{Library, LibraryExt, World};
use typst_kit::files::{FileLoader, FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::FontStore;
use typst_kit::packages::SystemPackages;
use typst_syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};

/// Loader that serves the edited main buffer from memory and everything else
/// (packages, data files) through typst-kit's system loader.
struct MemMain {
    main: FileId,
    text: Mutex<String>,
    fallback: SystemFiles,
}

impl FileLoader for MemMain {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main {
            return Ok(Bytes::new(self.text.lock().unwrap().clone().into_bytes()));
        }
        self.fallback.load(id)
    }
}

struct SpikeWorld {
    library: LazyHash<Library>,
    fonts: FontStore,
    files: FileStore<MemMain>,
    main: FileId,
}

impl World for SpikeWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }
    fn main(&self) -> FileId {
        self.main
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        self.files.source(id)
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }
    fn today(&self, _: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}

impl SpikeWorld {
    fn new(root: std::path::PathBuf, text: String) -> Self {
        let main =
            RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap()).intern();
        let mut fonts = FontStore::new();
        fonts.extend(typst_kit::fonts::embedded());
        let packages = SystemPackages::new(typst_kit::downloader::SystemDownloader::new("lilook"));
        let files = FileStore::new(MemMain {
            main,
            text: Mutex::new(text),
            fallback: SystemFiles::new(FsRoot::new(root), packages),
        });
        SpikeWorld {
            library: LazyHash::new(Library::default()),
            fonts,
            files,
            main,
        }
    }

    /// Replace the main buffer and mark files stale, as an editor would on each
    /// drag frame.
    fn edit(&mut self, text: String) {
        *self.files.loader().text.lock().unwrap() = text;
        self.files.reset();
    }

    fn compile(&self) -> Result<PagedDocument, String> {
        typst::compile::<PagedDocument>(self)
            .output
            .map_err(|e| format!("{:?}", e.first().map(|d| d.message.clone())))
    }
}

fn figure(n: usize, probes: &str, stroke: &str) -> String {
    format!(
        r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let __lilook_probe(id, x, y) = lq.place(x, y, context [#metadata((id: id, pos: here().position()))<lilook-probe>])
#let x = lq.linspace(0, 10, num: {n})
#lq.diagram(width: 6cm, height: 4cm,
  lq.plot(x, x.map(t => calc.sin(t)), mark: none, stroke: {stroke}){probes}
)
"#
    )
}

const PROBES: &str = r#", __lilook_probe("d0", 1, -0.8), __lilook_probe("d1", 9, 0.8), __lilook_probe("r0", 0%, 0%), __lilook_probe("r1", 100%, 100%)"#;

fn main() {
    let root = std::env::current_dir().unwrap();
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    let mut world = SpikeWorld::new(root, figure(n, "", "red"));

    let t = Instant::now();
    let doc = world.compile().expect("cold compile");
    println!("cold compile ({n} pts)        {:>8.1} ms", ms(t));

    // Warm recompile with one argument changed -- the drag-rate path.
    let colors = ["blue", "green", "orange", "purple", "teal"];
    let mut best = f64::MAX;
    for (i, c) in colors.iter().enumerate() {
        world.edit(figure(n, "", c));
        let t = Instant::now();
        world.compile().expect("warm compile");
        let dt = ms(t);
        best = best.min(dt);
        println!("warm recompile #{i} ({c:>6})  {dt:>8.1} ms");
    }
    println!("warm best                    {best:>8.1} ms");

    // Data-changing edit: worst case for memoisation.
    world.edit(figure(n + 1, "", "red"));
    let t = Instant::now();
    world.compile().expect("data edit");
    println!("data-changing recompile      {:>8.1} ms", ms(t));

    // Probe pass through the in-process introspector, no subprocess.
    world.edit(figure(n, PROBES, "red"));
    let t = Instant::now();
    let doc2 = world.compile().expect("probe compile");
    let label = typst::foundations::Label::new(typst::utils::PicoStr::intern("lilook-probe")).unwrap();
    let hits = doc2
        .introspector()
        .query(&typst::foundations::Selector::Label(label));
    println!(
        "probe compile + query        {:>8.1} ms  ({} probes)",
        ms(t),
        hits.len()
    );
    for h in &hits {
        println!("   {:?}", h.field_by_name("value"));
    }

    // Raster render of page 1 for the egui texture path.
    let t = Instant::now();
    let opts = typst_render::RenderOptions {
        pixel_per_pt: typst::utils::Scalar::new(2.0),
        render_bleed: false,
    };
    let pix = typst_render::render(&doc.pages()[0], &opts);
    println!(
        "render {}x{} @2x           {:>8.1} ms",
        pix.width(),
        pix.height(),
        ms(t)
    );
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
