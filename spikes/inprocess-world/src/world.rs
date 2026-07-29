
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


fn ms(t: Instant) -> f64 { t.elapsed().as_secs_f64() * 1000.0 }
