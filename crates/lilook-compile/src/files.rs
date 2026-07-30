//! Where lilook gets files from.
//!
//! Two implementations, one seam. `SystemFiles` (typst-kit's) reads the project
//! directory and downloads packages, which is what a desktop editor wants.
//! [`MemoryFiles`] serves everything from a map, which is what a build with no
//! file system needs -- the browser, and any host that keeps a manuscript in
//! its own storage rather than on disk.
//!
//! `docs/plan.md` §5 promises implore a workspace/filesystem seam. This is it:
//! the abstraction WASM forces is the same one a host application needs, so it
//! is built once rather than twice.

use std::collections::HashMap;

use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst_kit::files::FileLoader;
use typst_syntax::{FileId, VirtualRoot};

/// A read-only file system held in memory.
#[derive(Debug, Default, Clone)]
pub struct MemoryFiles {
    files: HashMap<String, Bytes>,
}

impl MemoryFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// The key a file id maps to. Stable and printable, so a bundle can be
    /// built by one program and read by another:
    ///
    /// - `project/figure.typ`
    /// - `package/preview/lilaq/0.6.0/src/lilaq.typ`
    pub fn key(id: FileId) -> String {
        let path = id.vpath().get_without_slash();
        match id.root() {
            VirtualRoot::Project => format!("project/{path}"),
            VirtualRoot::Package(spec) => format!(
                "package/{}/{}/{}/{path}",
                spec.namespace, spec.name, spec.version
            ),
        }
    }

    pub fn insert(&mut self, key: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        self.files.insert(key.into(), Bytes::new(bytes.into()));
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

impl FileLoader for MemoryFiles {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        let key = Self::key(id);
        self.files
            .get(&key)
            .cloned()
            .ok_or_else(|| FileError::NotFound(key.into()))
    }
}

/// Serve the main buffer from memory and everything else from `inner`.
///
/// Every frontend needs this: the whole point is to compile text that has not
/// been saved, so the main file can never come from wherever the rest does.
pub struct MainOverlay<L> {
    main: FileId,
    text: std::sync::Mutex<String>,
    /// A second in-memory document, for asking the compiler a one-off question
    /// -- "what are the column names in this file?" -- without disturbing the
    /// buffer being edited. It gets its own id so the editing buffer's parsed
    /// `Source` survives, which is the whole reason warm recompiles are 20 ms
    /// and not 108.
    query: std::sync::Mutex<Option<(FileId, String)>>,
    inner: L,
}

impl<L: FileLoader> MainOverlay<L> {
    pub fn new(main: FileId, text: impl Into<String>, inner: L) -> Self {
        MainOverlay {
            main,
            text: std::sync::Mutex::new(text.into()),
            query: std::sync::Mutex::new(None),
            inner,
        }
    }

    pub fn set_text(&self, text: impl Into<String>) {
        *self.text.lock().unwrap() = text.into();
    }

    /// Park a query document at `id`, or clear it with `None`.
    pub fn set_query(&self, doc: Option<(FileId, String)>) {
        *self.query.lock().unwrap() = doc;
    }

    pub fn inner(&self) -> &L {
        &self.inner
    }

    /// Mutable access to whatever supplies the non-main files -- how a browser
    /// build adds a file the user dropped onto the page.
    pub fn inner_mut(&mut self) -> &mut L {
        &mut self.inner
    }
}

impl<L: FileLoader> FileLoader for MainOverlay<L> {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main {
            return Ok(Bytes::new(self.text.lock().unwrap().clone().into_bytes()));
        }
        if let Some((qid, text)) = &*self.query.lock().unwrap() {
            if *qid == id {
                return Ok(Bytes::new(text.clone().into_bytes()));
            }
        }
        self.inner.load(id)
    }
}

/// Build a `MemoryFiles` from directories on disk -- the packaging step for a
/// build that will not have a file system. Native only, by definition.
#[cfg(feature = "system")]
pub fn bundle_from_dir(
    root: &std::path::Path,
    prefix: &str,
    into: &mut MemoryFiles,
) -> std::io::Result<usize> {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            // Only what a compile can actually read.
            let keep = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some(
                    "typ"
                        | "toml"
                        | "csv"
                        // The sidecar a transcoded binary file becomes, so a
                        // bundle can carry linked data too.
                        | "cbor"
                        | "json"
                        | "yaml"
                        | "yml"
                        | "txt"
                        | "svg"
                        | "png"
                        | "jpg"
                        | "jpeg"
                        | "gif"
                        | "wasm"
                        | "otf"
                        | "ttf"
                )
            );
            if !keep {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            into.insert(format!("{prefix}/{rel}"), std::fs::read(&path)?);
            n += 1;
        }
    }
    Ok(n)
}
