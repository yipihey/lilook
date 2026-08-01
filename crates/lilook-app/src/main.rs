//! The desktop shell. Deliberately thin: a window, a file, a compile thread,
//! and a screenshot flag. Everything you can see and click lives in
//! `lilook-editor`, which the browser build runs unchanged.

use std::path::PathBuf;

use lilook_compile::CompileActor;
use lilook_core::Schema;
use lilook_editor::Editor;

const SCHEMA: &str = lilook_core::schema::BUNDLED;

/// Where transcoded sidecars and their manifest live, relative to the document.
///
/// Under the project root because a typst path cannot escape it, and in a dotted
/// directory because these are lilook's working files rather than the user's.
const SIDECAR_DIR: &str = ".lilook";

/// The user's library on disk: palettes, colour maps and themes they saved.
///
/// The shell's job, not the session's -- `lilook-core` has no file system, which
/// is what lets the same editor run in a browser where there is no file at all.
/// A desktop keeps it in the place the platform puts configuration, so it
/// follows the user between documents rather than between directories.
mod library {
    use std::path::PathBuf;

    /// `~/.config/lilook/prefs.toml`, or the platform's equivalent.
    ///
    /// Hand-rolled rather than a dependency: three environment variables and a
    /// join, against a crate that would have to be tracked and audited for the
    /// same six lines.
    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let dir = if cfg!(target_os = "macos") {
            home?.join("Library/Application Support/lilook")
        } else if cfg!(target_os = "windows") {
            PathBuf::from(std::env::var_os("APPDATA")?).join("lilook")
        } else {
            match std::env::var_os("XDG_CONFIG_HOME") {
                Some(x) => PathBuf::from(x).join("lilook"),
                None => home?.join(".config/lilook"),
            }
        };
        Some(dir.join("prefs.toml"))
    }

    /// Read it, or start empty. A file that exists but cannot be read is
    /// *reported*, and carried back so that [`store`] can set it aside rather
    /// than write over it: losing a library quietly is worse than being told
    /// why, and being told why is no comfort if the next save destroys it
    /// anyway.
    pub fn load() -> lilook_core::Loaded {
        let Some(path) = path() else {
            return lilook_core::Prefs::load(None);
        };
        // Not there yet is the ordinary case, not a problem to announce.
        let found = std::fs::read_to_string(&path).ok();
        let mut loaded = lilook_core::Prefs::load(found);
        if let Some(why) = &loaded.complaint {
            loaded.complaint = Some(format!("{}: {why}", path.display()));
        }
        loaded
    }

    /// Where an unreadable library goes, so that nothing is destroyed to make
    /// room for its replacement.
    ///
    /// Numbered, and never over one that is already there: a second bad load
    /// must not overwrite the rescue from the first, which may be the copy that
    /// actually holds a year of palettes.
    fn free_rescue_path(path: &std::path::Path) -> Option<PathBuf> {
        (1..100).find_map(|n| {
            let at = path.with_extension(match n {
                1 => "toml.saved".to_string(),
                n => format!("toml.saved.{n}"),
            });
            (!at.exists()).then_some(at)
        })
    }

    /// Write the library, first putting anything unreadable somewhere safe.
    ///
    /// `rescue` is whatever [`load`] could not parse. Taking it by `&mut` is the
    /// point: once it has been set aside it is cleared, so the next save is an
    /// ordinary one and rescue copies do not pile up.
    pub fn store(prefs: &lilook_core::Prefs, rescue: &mut Option<String>) -> Result<(), String> {
        let path = path().ok_or("no home directory to store a library in")?;
        store_at(&path, prefs, rescue)
    }

    /// The part that touches the disk, with the location handed in.
    ///
    /// Split out so that a test can exercise the rescue on a directory of its
    /// own -- the alternative is setting `XDG_CONFIG_HOME` from a test, which is
    /// process-wide state and a race against every other test in the binary.
    pub fn store_at(
        path: &std::path::Path,
        prefs: &lilook_core::Prefs,
        rescue: &mut Option<String>,
    ) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // Nothing is written until the unreadable library is safely elsewhere:
        // if setting it aside fails, the original is still the only copy and
        // still on disk.
        if let Some(text) = rescue.clone() {
            let at = free_rescue_path(path).ok_or_else(|| {
                format!(
                    "{}: no free name to set the old library aside under",
                    path.display()
                )
            })?;
            std::fs::write(&at, text).map_err(|e| format!("{}: {e}", at.display()))?;
            *rescue = None;
            std::fs::write(path, prefs.to_toml())
                .map_err(|e| format!("{}: {e}", path.display()))?;
            return Err(format!(
                "your library could not be read, so it was kept as {} and a new one started",
                at.display()
            ));
        }
        std::fs::write(path, prefs.to_toml()).map_err(|e| format!("{}: {e}", path.display()))
    }
}

struct App {
    editor: Editor,
    path: Option<PathBuf>,
    actor: CompileActor,
    /// Where the export in flight should be written when its bytes arrive.
    pending_export: Option<std::path::PathBuf>,
    /// Current window title, so it is only pushed when it changes.
    title: String,
    /// A library that was found but could not be read, held until the first
    /// save can put it somewhere safe. `Some` means the file on disk is *not*
    /// ours to overwrite.
    rescue: Option<String>,
    /// What was last written to (or read from) disk, so unsaved changes are a
    /// fact rather than a guess.
    saved_text: String,
    /// Modification time of the file when lilook last read or wrote it.
    saved_at: Option<std::time::SystemTime>,
    /// The file changed on disk under us and the user has to choose.
    conflict: bool,
    /// Frame time of the last staleness check.
    checked_at: f64,
    /// Frame time of the last linked-data check, and what those files looked
    /// like when they were last believed.
    data_checked_at: f64,
    watched: std::collections::HashMap<String, Watch>,
    /// `--screenshot PATH`: draw until the figure is on screen, save a PNG of
    /// the window, quit. The findings record that capturing the shell used to
    /// hang; an agent that cannot see its own UI cannot claim it renders.
    screenshot: Option<Screenshot>,
}

struct Screenshot {
    path: PathBuf,
    /// Frames drawn so far. The capture waits for the first compile to land,
    /// because a screenshot of an empty canvas proves nothing.
    countdown: u32,
    asked: bool,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        text: String,
        path: Option<PathBuf>,
        screenshot: Option<PathBuf>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let actor = CompileActor::spawn(lilook_compile::root_for(path.as_ref()), move || {
            ctx.request_repaint()
        });
        let mut editor = Editor::new(
            text.clone(),
            Schema::from_json(SCHEMA).expect("bundled schema"),
        );
        let loaded = library::load();
        let rescue = loaded.rescue;
        let complaint = loaded.complaint;
        editor.prefs = loaded.prefs;
        if let Some(why) = complaint {
            editor.status = format!("your library could not be read -- {why}");
        }
        App {
            editor,
            rescue,
            path: path.clone(),
            actor,
            pending_export: None,
            title: String::new(),
            saved_text: text,
            saved_at: mtime(path.as_ref()),
            conflict: false,
            checked_at: 0.0,
            data_checked_at: 0.0,
            watched: std::collections::HashMap::new(),
            screenshot: screenshot.map(|path| Screenshot {
                path,
                countdown: 0,
                asked: false,
            }),
        }
    }

    /// Feed the compile thread and take back whatever it finished.
    fn pump(&mut self, ctx: &egui::Context, request: Option<(String, f32)>, query: Option<String>) {
        if let Some((source, ppp)) = request {
            self.actor.request(source, ppp);
        }
        if let Some(expr) = query {
            self.actor.query(expr);
        }
        let found: Vec<lilook_core::Blame> =
            self.actor.take_blames().into_iter().flatten().collect();
        if !found.is_empty() {
            self.editor.accept_blame(found);
        }
        for e in self.actor.take_exports() {
            let path = self.pending_export.take();
            self.editor.status = match (e.bytes, path) {
                (Ok(bytes), Some(p)) => match std::fs::write(&p, &bytes) {
                    Ok(()) => format!("wrote {} ({} KB)", p.display(), bytes.len() / 1024),
                    Err(err) => format!("could not write {}: {err}", p.display()),
                },
                (Err(why), _) => why,
                (Ok(_), None) => "an export arrived with nowhere to go".into(),
            };
        }
        for a in self.actor.take_answers() {
            self.editor.accept_answer(&a.expr, a.answer, &a.diagnostics);
        }
        self.editor.set_busy(self.actor.busy());
        if let Some(frame) = self.actor.take_latest() {
            self.editor
                .accept(ctx, frame.render, frame.scenes, frame.data_files);
        }
    }

    fn save(&mut self) {
        let Some(p) = &self.path else {
            self.editor.status = "no path: this buffer has never been saved".into();
            return;
        };
        let text = self.editor.text().to_string();
        self.editor.status = match std::fs::write(p, &text) {
            Ok(()) => {
                self.saved_text = text;
                self.saved_at = mtime(self.path.as_ref());
                self.conflict = false;
                format!("saved {}", p.display())
            }
            Err(e) => e.to_string(),
        };
    }

    /// Write the figure beside the manuscript, in the format asked for.
    ///
    /// From the document the canvas is already showing, so what lands on disk is
    /// exactly what is on screen -- not a second compile that might differ.
    fn export(&mut self, format: &str, ppi: f32) {
        let Some(fmt) = lilook_compile::export::Format::of_path(&format!("x.{format}")) else {
            self.editor.status = format!("unknown format {format}");
            return;
        };
        // Beside the .typ under its own name, or in the working directory for a
        // buffer that has never been saved.
        let path = match &self.path {
            Some(p) => p.with_extension(fmt.extension()),
            None => std::path::PathBuf::from(format!("figure.{}", fmt.extension())),
        };
        // The compile thread owns the document, so this is a round trip: the
        // bytes come back through `take_exports` and land in `pending_export`.
        self.pending_export = Some(path);
        self.actor.export(fmt, ppi);
    }

    fn unsaved(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    /// The directory relative paths in the document resolve against.
    fn root(&self) -> PathBuf {
        lilook_compile::root_for(self.path.as_ref())
    }

    /// Bring a dropped file into the project.
    ///
    /// A typst path cannot escape the project root, so a file anywhere else is
    /// not linkable where it lies. Copying it in is the only thing that keeps the
    /// document compilable by plain `typst`, and it is done explicitly rather
    /// than silently: the status line says what happened.
    fn adopt(&mut self, dropped: Vec<lilook_editor::Dropped>) {
        let root = self.root();
        for d in dropped {
            let Some(from) = d.path.as_deref().map(PathBuf::from) else {
                self.editor
                    .adoption_failed(&d.name, "the drop carried no path");
                continue;
            };
            // Formats typst cannot read at all become a CBOR sidecar, wherever
            // the original lives. That also solves the root problem for them: the
            // original stays put and only the sidecar comes into the project.
            match self.transcode(&from, &d.name) {
                Some(Ok(rel)) => {
                    self.editor.file_adopted(rel);
                    continue;
                }
                Some(Err(why)) => {
                    self.editor.adoption_failed(&d.name, &why);
                    continue;
                }
                None => {}
            }
            // Already under the root: link it where it is, and do not touch it.
            if let Ok(rel) = from.strip_prefix(&root) {
                self.editor.file_adopted(rel.to_string_lossy().into_owned());
                continue;
            }
            let to = root.join(&d.name);
            if to.exists() {
                // Overwriting someone's data file to link it would be the worst
                // thing this feature could do.
                self.editor.adoption_failed(
                    &d.name,
                    "a file of that name is already in the project directory",
                );
                continue;
            }
            match std::fs::copy(&from, &to) {
                Ok(_) => self.editor.file_adopted(d.name.clone()),
                Err(e) => self.editor.adoption_failed(&d.name, &e.to_string()),
            }
        }
    }

    /// Turn a file typst cannot read into a CBOR sidecar it can.
    ///
    /// `None` for a format typst reads itself -- a CSV is linked directly, and
    /// transcoding it would add an artefact for nothing. Otherwise the sidecar
    /// goes in `.lilook/` beside the document, with a manifest recording where it
    /// came from so it can be regenerated and so a change to the *original* can
    /// be noticed.
    ///
    /// The original is never modified and never moved.
    fn transcode(&mut self, from: &std::path::Path, name: &str) -> Option<Result<String, String>> {
        let bytes = match std::fs::read(from) {
            Ok(b) => b,
            Err(e) => return Some(Err(e.to_string())),
        };
        let format = lilook_data::sniff(&bytes, name)?;
        if !format.available() {
            return Some(Err(format.unavailable_because().to_string()));
        }
        // HDF5 is the one format read from a path rather than from bytes, because
        // libhdf5's API is built that way. Everything else is portable.
        let decoded = match format {
            #[cfg(feature = "hdf5")]
            lilook_data::Format::Hdf5 => lilook_data::hdf5::read_path(from),
            _ => lilook_data::decode(&bytes, format),
        };
        let data = match decoded {
            Ok(d) => d,
            Err(e) => return Some(Err(e.to_string())),
        };
        if data.columns.is_empty() {
            return Some(Err("nothing in that file is a column of numbers".into()));
        }

        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        let rel = format!("{SIDECAR_DIR}/{stem}.cbor");
        let dir = self.root().join(SIDECAR_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Some(Err(e.to_string()));
        }
        if let Err(e) = std::fs::write(self.root().join(&rel), data.to_cbor()) {
            return Some(Err(e.to_string()));
        }
        self.record_link(&rel, from, &bytes, &data);
        self.editor.status = format!(
            "read {} column(s) from {name} into {rel}",
            data.columns.len()
        );
        Some(Ok(rel))
    }

    /// Note in `.lilook/links.toml` where a sidecar came from.
    ///
    /// Beside the data rather than in the `.typ`, deliberately. The document then
    /// contains only plain typst that the compiler fully validates, provenance
    /// cannot go stale against it, and copying a `#let` into another project
    /// carries an honest link rather than a claim about a file that is not there.
    /// The length and a cheap digest are what let a stale sidecar be detected.
    fn record_link(
        &self,
        sidecar: &str,
        origin: &std::path::Path,
        bytes: &[u8],
        data: &lilook_data::Dataset,
    ) {
        let path = self.root().join(SIDECAR_DIR).join("links.toml");
        let previous = std::fs::read_to_string(&path).unwrap_or_default();
        // Drop any earlier record of the same sidecar, then append.
        let kept: String = previous
            .split("\n[[link]]")
            .filter(|block| !block.contains(&format!("sidecar = {:?}", sidecar)))
            .collect::<Vec<_>>()
            .join("\n[[link]]");
        let mut out = if kept.trim().is_empty() {
            String::from(
                "# Written by lilook. Where each sidecar under this directory came\n\
                 # from, so it can be regenerated and so a change to the original\n\
                 # can be noticed. The figure itself does not read this file.\n",
            )
        } else {
            kept
        };
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!(
            "\n[[link]]\nsidecar = {:?}\norigin = {:?}\nbytes = {}\ndigest = \"{:016x}\"\ncolumns = [{}]\n",
            sidecar,
            origin.to_string_lossy(),
            bytes.len(),
            digest(bytes),
            data.names()
                .iter()
                .map(|n| format!("{n:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        ));
        let _ = std::fs::write(path, out);
    }

    /// Notice a linked data file changing under the figure.
    ///
    /// Polls, because there is no dependency worth adding for this and the files
    /// to watch change with every compile. Two guards make polling honest:
    ///
    /// - mtime *and* size, since a rewrite can preserve either one alone;
    /// - a change has to hold across two consecutive polls before it counts,
    ///   which is what keeps a file caught mid-write out of the figure.
    ///
    /// It still cannot see a sub-second rewrite, or a writer that preserves both
    /// (`rsync -t`). That is another reason the result is offered rather than
    /// applied.
    fn watch_data(&mut self, now: f64) {
        if now - self.data_checked_at < 1.0 {
            return;
        }
        self.data_checked_at = now;
        let root = self.root();
        let linked: Vec<String> = self
            .editor
            .data_files()
            .iter()
            .filter(|d| d.is_data())
            .map(|d| d.path.clone())
            .collect();
        // Forget files the figure no longer reads, or the list grows forever and
        // reports changes to data nothing plots.
        self.watched.retain(|p, _| linked.contains(p));

        let mut changed = vec![];
        for path in linked {
            let stamp = stamp(&root.join(&path));
            match self.watched.get_mut(&path) {
                // First sight: whatever it is now is the baseline, because the
                // compile that read it read *this*.
                None => {
                    self.watched.insert(
                        path,
                        Watch {
                            settled: stamp,
                            seen: stamp,
                        },
                    );
                }
                Some(w) => {
                    let steady = stamp == w.seen;
                    w.seen = stamp;
                    if steady && stamp != w.settled {
                        w.settled = stamp;
                        changed.push(path);
                    }
                }
            }
        }
        if !changed.is_empty() {
            self.editor.files_changed(&changed);
        }
    }

    /// Notice a file edited in another program.
    ///
    /// lilook is not the only thing that touches a manuscript -- the user has a
    /// text editor open on the same file. Reloading silently would throw away
    /// their edits here; ignoring it would let lilook save over theirs. So it
    /// asks, and only when there is something to lose.
    fn check_disk(&mut self, now: f64) {
        if self.conflict || now - self.checked_at < 1.0 {
            return;
        }
        self.checked_at = now;
        let Some(on_disk) = mtime(self.path.as_ref()) else {
            return;
        };
        if Some(on_disk) == self.saved_at {
            return;
        }
        if self.unsaved() {
            self.conflict = true;
        } else {
            self.reload();
        }
    }

    fn reload(&mut self) {
        let Some(p) = self.path.clone() else { return };
        match std::fs::read_to_string(&p) {
            Ok(text) => {
                // A fresh document: the edit history belongs to the text it was
                // recorded against, and keeping it across a reload would let an
                // undo write bytes from a file that no longer exists.
                self.editor.open(text.clone());
                self.saved_text = text;
                self.saved_at = mtime(Some(&p));
                self.conflict = false;

                self.editor.status = format!("reloaded {}", p.display());
            }
            Err(e) => self.editor.status = e.to_string(),
        }
    }

    /// Keep the window title honest about the file and whether it is saved.
    fn retitle(&mut self, ctx: &egui::Context) {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        let title = if self.unsaved() {
            format!("lilook — {name} •")
        } else {
            format!("lilook — {name}")
        };
        if self.title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Drive the `--screenshot` state machine: wait for pixels, ask, save, quit.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        // Keep drawing: the compile lands on another thread, and a screenshot
        // taken before the first frame arrives would show an empty canvas.
        ctx.request_repaint();

        let saved = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = saved {
            let [w, h] = [image.width() as u32, image.height() as u32];
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
            let result = tiny_skia::Pixmap::from_vec(
                rgba,
                tiny_skia::IntSize::from_wh(w, h).expect("non-empty window"),
            )
            .ok_or_else(|| "bad pixmap".to_string())
            .and_then(|p| p.encode_png().map_err(|e| e.to_string()))
            .and_then(|png| std::fs::write(&shot.path, png).map_err(|e| e.to_string()));
            match result {
                Ok(()) => eprintln!("wrote {}", shot.path.display()),
                Err(e) => eprintln!("screenshot failed: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Ask once there are pixels to capture, but never wait forever: a
        // document that cannot compile still has a window worth looking at.
        shot.countdown += 1;
        let ready = !self.editor.scenes().is_empty() && shot.countdown > 10;
        if !shot.asked && (ready || shot.countdown > 400) {
            shot.asked = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

/// A cheap content digest, for noticing that a sidecar no longer matches the file
/// it came from.
///
/// FNV-1a: not cryptographic and not meant to be. What it has to catch is a file
/// that was regenerated or edited, and for that a 64-bit non-cryptographic hash
/// is plenty -- the alternative is a dependency for one function.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The file's modification time, if it exists.
fn mtime(path: Option<&PathBuf>) -> Option<std::time::SystemTime> {
    std::fs::metadata(path?).ok()?.modified().ok()
}

/// Modification time and size together, since a rewrite can preserve either one
/// on its own. `None` for a file that is not there, which is itself a change
/// worth noticing.
fn stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What a watched data file looked like.
struct Watch {
    /// The last state that held still long enough to be believed.
    settled: Option<(std::time::SystemTime, u64)>,
    /// The state at the previous poll, which may be a file mid-write.
    seen: Option<(std::time::SystemTime, u64)>,
}

impl App {
    /// Write the library out when it changed, and say so once.
    ///
    /// Checked every frame and almost always false: the flag is set by saving or
    /// forgetting a palette, and cleared here whatever the outcome, so a failing
    /// disk cannot make this retry on every frame for the rest of the session.
    fn store_library(&mut self) {
        if !self.editor.prefs_dirty {
            return;
        }
        self.editor.prefs_dirty = false;
        if let Err(why) = library::store(&self.editor.prefs, &mut self.rescue) {
            self.editor.status = format!("your library could not be saved -- {why}");
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.check_disk(now);
        self.watch_data(now);
        self.retitle(&ctx);

        self.store_library();
        let mut reload = false;
        let mut keep = false;
        let conflict = self.conflict;
        let requests = self.editor.ui(ui, now, |ui| {
            if !conflict {
                return;
            }
            ui.horizontal(|ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "this file changed on disk, and you have unsaved changes",
                );
                reload |= ui.button("reload (discard mine)").clicked();
                keep |= ui.button("keep mine").clicked();
            });
            ui.separator();
        });
        if reload {
            self.reload();
        }
        if keep {
            self.conflict = false;
            self.saved_at = mtime(self.path.as_ref());
        }
        if requests.save {
            self.save();
        }
        if !requests.adopt.is_empty() {
            self.adopt(requests.adopt);
        }
        if let Some(e) = requests.write_figure {
            let path = self
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.join(&e.path))
                .unwrap_or_else(|| std::path::PathBuf::from(&e.path));
            self.editor.status = match std::fs::write(&path, &e.contents) {
                Ok(()) => format!("wrote {}", path.display()),
                Err(err) => format!("could not write {}: {err}", path.display()),
            };
        }
        for message in requests.blame {
            self.actor.blame(self.editor.text().to_string(), message);
        }
        if let Some((format, ppi)) = requests.export {
            self.export(&format, ppi);
        }
        self.pump(&ctx, requests.compile, requests.query);
        self.screenshot_step(&ctx);
    }
}

const SAMPLE: &str = r#"#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 8pt)

#let x = lq.linspace(0, 10)
#lq.diagram(
  width: 8cm, height: 5cm,
  xlabel: [time], ylabel: [signal],
  lq.plot(x, x.map(t => calc.sin(t)), mark: none, stroke: red),
  lq.plot(x, x.map(t => calc.cos(t)), mark: none, stroke: blue),
)
"#;

const USAGE: &str = "\
lilook -- edit a lilaq figure by pointing at it

    lilook [FILE]

FILE is a Typst document, or a `.lil` figure. With none, a sample opens.

    -h, --help            this
    -V, --version         the version
        --screenshot OUT  draw one frame to a PNG and exit
        --select N        select call site N first, for scripted screenshots

The figure is edited in place: every gesture is a small edit to the file, and
undo is a text-edit history. Try it in a browser first if you like:
https://yipihey.github.io/lilook/
";

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut screenshot = None;
    // `--select` exists for scripted screenshots and for driving the app from a
    // test: without it there is no way to reach a non-default selection
    // without a pointer.
    let mut select = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "--select" => select = args.next().and_then(|s| s.parse().ok()),
            // The first thing anyone types after downloading a binary. Without
            // these it read `--help` as a filename and reported that no such
            // file exists, which is a poor first impression and was found by
            // rehearsing the release rather than by using the program.
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("lilook {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other if other.starts_with('-') => {
                eprintln!("lilook: unknown option {other}\n\n{USAGE}");
                std::process::exit(2);
            }
            _ => path = Some(PathBuf::from(a)),
        }
    }
    let text = match path.as_ref() {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("lilook: {}: {e}", p.display());
            std::process::exit(1);
        }),
        None => SAMPLE.to_string(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("lilook"),
        ..Default::default()
    };

    eframe::run_native(
        "lilook",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc, text, path, screenshot);
            if let Some(id) = select {
                app.editor.selected = id;
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod library_tests {
    use super::library;
    use lilook_core::{Kind, Prefs};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lilook-library-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join("prefs.toml")
    }

    /// The whole point of the rescue: a library lilook cannot read survives the
    /// next save.
    ///
    /// Before this, a stray keystroke in the file cost the user everything in
    /// it -- the bad file was *reported*, the session carried on with an empty
    /// library, and the first thing saved afterwards was written straight over
    /// the original.
    #[test]
    fn an_unreadable_library_survives_the_next_save() {
        let path = scratch("unreadable");
        let original = "version = 1\n[[saved]]\nkind = \"cycle\"\nname = ";
        std::fs::write(&path, original).expect("a broken library");

        let loaded = Prefs::load(std::fs::read_to_string(&path).ok());
        let mut rescue = loaded.rescue;
        assert!(rescue.is_some(), "unreadable, so held for rescue");

        let mut prefs = loaded.prefs;
        prefs
            .save(Kind::Cycle, "new", "(red, blue)")
            .expect("saved");
        let told = library::store_at(&path, &prefs, &mut rescue)
            .expect_err("the user is told what happened to their file");
        assert!(told.contains("kept as"), "and where it went: {told}");
        assert!(rescue.is_none(), "done once, not on every save after");

        let kept = path.with_extension("toml.saved");
        assert_eq!(
            std::fs::read_to_string(&kept).ok().as_deref(),
            Some(original),
            "the original, byte for byte"
        );
        assert!(
            std::fs::read_to_string(&path)
                .expect("a new library")
                .contains("new"),
            "and the save the user asked for went through"
        );
    }

    /// A second bad load must not overwrite the first rescue -- that copy may be
    /// the one holding a year of palettes.
    #[test]
    fn a_rescue_never_lands_on_an_earlier_one() {
        let path = scratch("twice");
        std::fs::write(path.with_extension("toml.saved"), "the first rescue").expect("written");
        let mut rescue = Some("the second".to_string());
        let _ = library::store_at(&path, &Prefs::default(), &mut rescue);
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.saved")).ok(),
            Some("the first rescue".into()),
            "left alone"
        );
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.saved.2")).ok(),
            Some("the second".into()),
            "and the new one beside it"
        );
    }

    /// An ordinary save is still ordinary: no rescue copies pile up beside a
    /// library that was read without trouble.
    #[test]
    fn a_good_library_is_written_in_place() {
        let path = scratch("good");
        let mut prefs = Prefs::default();
        prefs
            .save(Kind::Colormap, "mine", "(red, blue)")
            .expect("saved");
        library::store_at(&path, &prefs, &mut None).expect("stored");
        library::store_at(&path, &prefs, &mut None).expect("stored again");
        assert!(
            !path.with_extension("toml.saved").exists(),
            "nothing set aside"
        );
        let back = Prefs::load(std::fs::read_to_string(&path).ok());
        assert_eq!(back.prefs.saved.len(), 1, "and it reads back");
    }
}
