//! The `World` lilook compiles against.
//!
//! The user's buffer is served from memory rather than from disk: the whole
//! point is to compile text that has not been saved. Everything else --
//! packages, data files the figure reads, images -- goes through typst-kit's
//! system loader, so a figure behaves exactly as it does under the CLI.
//!
//! `FileStore::reset()` between compiles is what makes the incremental path
//! fast: it edits the cached `Source` in place rather than reparsing from
//! scratch. Measured on a 1k-point lilaq figure: 108 ms cold, 20 ms for a
//! subsequent style edit.

#[cfg(feature = "system")]
use std::path::{Path, PathBuf};

use typst::diag::{FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_kit::files::{FileLoader, FileStore};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;
use typst_syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};

use crate::files::{MainOverlay, MemoryFiles};

pub use lilook_core::render::{Diagnostic, Severity};

pub struct LilookWorld<L: FileLoader + Send + Sync = MemoryFiles> {
    library: LazyHash<Library>,
    fonts: FontStore,
    files: FileStore<MainOverlay<L>>,
    main: FileId,
}

/// The id of the buffer being edited. It is never on disk -- the whole point is
/// to compile text that has not been saved -- so it gets a name no real file
/// would collide with.
pub fn main_id() -> FileId {
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new("<lilook>.typ").unwrap(),
    )
    .intern()
}

impl<L: FileLoader + Send + Sync> LilookWorld<L> {
    /// Compile `text` with everything else coming from `loader`.
    pub fn with_loader(loader: L, text: impl Into<String>) -> Self {
        let main = main_id();
        #[allow(unused_mut)]
        let mut fonts = FontStore::new();
        #[cfg(feature = "embedded-fonts")]
        fonts.extend(typst_kit::fonts::embedded());
        LilookWorld {
            library: LazyHash::new(Library::default()),
            fonts,
            files: FileStore::new(MainOverlay::new(main, text, loader)),
            main,
        }
    }

    /// Register a font from raw bytes, returning how many faces it contained.
    ///
    /// This is how a browser build gets its fonts: typst's embedded set is
    /// 9.6 MB, which is 60% of the download and mostly faces a lilaq figure
    /// never asks for. Fetching the two or three it does need, separately and
    /// cached, is both smaller and faster to start.
    pub fn add_font_data(&mut self, data: impl Into<Vec<u8>>) -> usize {
        let bytes = typst::foundations::Bytes::new(data.into());
        let mut n = 0;
        for font in typst::text::Font::iter(bytes) {
            let info = font.info().clone();
            self.fonts.push((font, info));
            n += 1;
        }
        n
    }

    /// Add more fonts, e.g. the system's.
    pub fn with_fonts(
        mut self,
        fonts: impl IntoIterator<Item = (impl typst_kit::fonts::FontSource, typst::text::FontInfo)>,
    ) -> Self {
        self.fonts.extend(fonts);
        self
    }

    /// Swap the main buffer. Sources are edited in place rather than rebuilt,
    /// which is where the incremental win comes from.
    pub fn set_source(&mut self, text: impl Into<String>) {
        self.files.loader().set_text(text);
        self.files.reset();
    }

    pub fn compile(&self) -> (Option<PagedDocument>, Vec<Diagnostic>) {
        let warned = typst::compile::<PagedDocument>(self);
        let mut diags: Vec<Diagnostic> = warned
            .warnings
            .iter()
            .map(|d| self.diagnostic(d, Severity::Warning))
            .collect();
        match warned.output {
            Ok(doc) => (Some(doc), diags),
            Err(errors) => {
                diags.extend(errors.iter().map(|d| self.diagnostic(d, Severity::Error)));
                (None, diags)
            }
        }
    }

    fn diagnostic(&self, d: &SourceDiagnostic, severity: Severity) -> Diagnostic {
        Diagnostic {
            severity,
            message: d.message.to_string(),
            range: (d.span.id() == Some(self.main))
                .then(|| self.range(d.span))
                .flatten(),
            hints: d.hints.iter().map(|h| h.v.to_string()).collect(),
        }
    }
}

#[cfg(feature = "system")]
impl LilookWorld<typst_kit::files::SystemFiles> {
    /// The desktop world: `root` is the directory relative paths resolve
    /// against, i.e. where the `.typ` file lives.
    pub fn new(root: impl AsRef<Path>, text: impl Into<String>) -> Self {
        use typst_kit::files::{FsRoot, SystemFiles};
        use typst_kit::packages::SystemPackages;

        let packages = SystemPackages::new(typst_kit::downloader::SystemDownloader::new(concat!(
            "lilook/",
            env!("CARGO_PKG_VERSION")
        )));
        let files = SystemFiles::new(FsRoot::new(root.as_ref().to_path_buf()), packages);
        // System fonts too: a manuscript that sets a font family should render
        // the same here as under the CLI. This is the one slow part of startup.
        Self::with_loader(files, text).with_fonts(typst_kit::fonts::system())
    }
}

impl<L: FileLoader + Send + Sync> World for LilookWorld<L> {
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
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // Only where there is a clock to ask. A figure that calls `datetime`
        // gets a clear error rather than a wrong date.
        #[cfg(feature = "system")]
        {
            typst_kit::datetime::Time::system().today(_offset)
        }
        #[cfg(not(feature = "system"))]
        {
            None
        }
    }
}

#[cfg(feature = "system")]
/// Where the document lives on disk, if anywhere. A never-saved buffer still
/// needs a root for relative paths; the working directory is the honest guess.
pub fn root_for(path: Option<&PathBuf>) -> PathBuf {
    path.and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}
