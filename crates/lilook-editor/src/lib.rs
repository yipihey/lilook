//! The editor: everything between a `Document` and a pair of eyes.
//!
//! Panels, selection, gestures, intents and transactions live here, over `egui`
//! and `lilook-ui` alone. It never compiles anything and never touches a file
//! system: the shell hands it a compiled frame and asks it for the next source
//! to compile. That is what lets the desktop window and the browser page be the
//! same editor rather than two that drift.

use lilook_core::render::{Diagnostic, Render, Severity};
use lilook_core::scene::Scene;
use lilook_core::{DataFile, Document, Intent, Schema};
use lilook_ui::{Canvas, CanvasInput, Inspector, PageTexture, UiEvent};

/// A gesture, as the canvas reports it. Re-exported because `handle_canvas` is
/// the editor's public entry for one, and a shell should not have to depend on
/// `lilook-ui` to name what it is passing.
pub use lilook_ui::CanvasEvent;

/// Re-rasterise when the view zoom has drifted this far from the resolution the
/// current textures were produced at. Rendering is under a millisecond, but a
/// recompile is not, and every re-render costs one.
const RESOLUTION_SLACK: f32 = 1.35;
const MAX_PIXEL_PER_PT: f32 = 6.0;
/// How long after the last keystroke or picker frame an auto-opened transaction
/// closes. Long enough to type a word, short enough that undo feels immediate.
const IDLE_COMMIT_SECONDS: f64 = 0.4;

/// A copied fragment and the definitions it depends on.
#[derive(Debug, Clone)]
pub struct Clip {
    pub source: String,
    /// `name` -> the whole `#let name = ..` that defines it, where the source
    /// document had one.
    pub bindings: Vec<(String, String)>,
    /// Names the source document did not define either, carried so a paste can
    /// say what will be missing rather than failing silently.
    pub unresolved: Vec<String>,
}

/// What the editor wants from its shell this frame. Everything platform-shaped
/// -- writing a file, closing a window -- is a request rather than an action,
/// because the browser cannot honour most of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Requests {
    pub save: bool,
    /// Source to compile, and at what resolution. Latest-wins: the shell may
    /// drop it if it is already busy.
    pub compile: Option<(String, f32)>,
    /// An expression for the shell to evaluate through the compiler, answered
    /// with `Editor::accept_answer`. The editor cannot ask directly: it does not
    /// depend on the compiler, and in the desktop shell the compiler is on
    /// another thread anyway.
    pub query: Option<String>,
    /// Files dropped onto the window that are not already under the project
    /// root. Typst cannot read a path that escapes the root, so the shell has to
    /// bring the file in -- copying it on the desktop, or into the in-memory
    /// file system in the browser -- and then call `Editor::file_adopted` with
    /// the project-relative path it ended up at.
    pub adopt: Vec<Dropped>,
}

/// A file the user dropped onto the window.
///
/// Either form can arrive: a desktop drop carries a path, a browser drop carries
/// the bytes, and neither shell gets to see the other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// The file's own name, without any directory.
    pub name: String,
    pub path: Option<String>,
    pub bytes: Option<Vec<u8>>,
}

/// What to draw around the figure. A browser page has room for a gallery where
/// a window has a menu bar, so the shell decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub tree: bool,
    pub inspector: bool,
    pub source: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            tree: true,
            inspector: true,
            source: true,
        }
    }
}

pub struct Editor {
    pub doc: Document,
    pub schema: Schema,
    pub selected: usize,
    pub status: String,
    pub layout: Layout,

    canvas: Canvas,
    /// Held only to keep the textures alive; the canvas draws by id.
    textures: Vec<egui::TextureHandle>,
    pages: Vec<PageTexture>,
    /// The finest scale this machine's textures can hold for the current page
    /// size. Recomputed from every render, so it relaxes again when the figure
    /// shrinks.
    max_pixel_per_pt: f32,
    scenes: Vec<Scene>,
    /// Files the last compile read. Not derived from the source text: a path
    /// built by an expression, or one read only on some branch, is invisible to
    /// any amount of parsing but perfectly visible to the compiler.
    data_files: Vec<DataFile>,
    diagnostics: Vec<Diagnostic>,
    /// Points per pixel the live textures were rendered at.
    rendered_at: f32,
    /// The source has changed since the last compile request.
    dirty: bool,
    /// A gesture (drag) opened the current transaction and will close it.
    explicit_tx: bool,
    /// A transaction lilook opened on the user's behalf, and the time of the
    /// last edit in it. Typing in a text box or dragging a colour picker emits
    /// one edit per frame with no press or release to bracket them, and one
    /// undo step per keystroke makes undo useless.
    idle_tx: Option<f64>,
    /// True while the shell has a compile in flight.
    busy: bool,
    timing: String,
    /// The last thing lilook copied, kept so a paste inside lilook can carry
    /// the bindings the fragment needs. The clipboard itself is plain Typst
    /// source, so a copy is still useful in any editor.
    clipboard: Option<Clip>,
    requests: Requests,
    /// The link-a-data-file flow, when one is open.
    link: Option<Link>,
    /// A query the shell has not been handed yet. Held here rather than written
    /// straight into `requests`, because `ui` clears those at the top of every
    /// frame and a link can be started from outside a frame.
    queued_query: Option<String>,
    /// Linked files that changed on disk and have not been reread.
    changed_files: Vec<String>,
    /// Reread linked files as soon as they change, rather than offering to.
    follow_files: bool,
    /// What the user typed into the link field.
    link_path: String,
}

/// Linking a file to a series, one step at a time.
///
/// The middle state exists because lilook has to *ask the compiler* what is in
/// the file -- it has no CSV parser of its own, and does not need one -- and the
/// answer arrives through the shell a frame or more later.
#[derive(Debug, Clone, PartialEq)]
enum Link {
    /// Waiting for the answer to `expr`.
    Asking { path: String, expr: String },
    /// The file described itself; the user picks which column goes where.
    Ready {
        path: String,
        kind: lilook_core::SourceKind,
        columns: lilook_core::Columns,
        x: usize,
        y: usize,
        /// Which entry supplies a mesh's field, when the target is a mesh and
        /// the file holds a 2-D one. `None` means link x and y only, which is
        /// every other series.
        z: Option<usize>,
    },
    /// The file could not be read, with the compiler's reason.
    Failed { path: String, why: String },
}

impl Editor {
    pub fn new(text: impl Into<String>, schema: Schema) -> Self {
        let doc = Document::new(text);
        let selected = doc
            .calls()
            .iter()
            .find(|c| c.callee.ends_with("diagram"))
            .or_else(|| doc.calls().first())
            .map(|c| c.id)
            .unwrap_or(0);
        Editor {
            doc,
            schema,
            selected,
            status: String::new(),
            layout: Layout::default(),
            canvas: Canvas::new(),
            textures: vec![],
            pages: vec![],
            max_pixel_per_pt: MAX_PIXEL_PER_PT,
            scenes: vec![],
            data_files: vec![],
            diagnostics: vec![],
            rendered_at: 1.0,
            dirty: true,
            explicit_tx: false,
            idle_tx: None,
            busy: false,
            timing: String::new(),
            clipboard: None,
            requests: Requests::default(),
            link: None,
            queued_query: None,
            changed_files: vec![],
            follow_files: false,
            link_path: String::new(),
        }
    }

    /// Tell the editor its document changed underneath it -- after an intent
    /// applied by something other than the editor itself.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn text(&self) -> &str {
        self.doc.text()
    }

    pub fn scenes(&self) -> &[Scene] {
        &self.scenes
    }

    /// Files the last compile read, so a shell can watch the ones that matter.
    pub fn data_files(&self) -> &[DataFile] {
        &self.data_files
    }

    /// Linked files that changed and have not been reread yet.
    pub fn changed_files(&self) -> &[String] {
        &self.changed_files
    }

    /// Reread linked files as soon as they change, rather than offering to.
    pub fn set_follow_files(&mut self, follow: bool) {
        self.follow_files = follow;
    }

    /// A shell has noticed these linked files change on disk.
    ///
    /// Reported rather than acted on: someone is editing that file, and a figure
    /// that redraws itself from a half-written file is worse than one that waits
    /// to be told. `check_disk` already decided that question for the manuscript;
    /// this is the same answer for its data. Following changes automatically is
    /// opt-in, and even then never lands mid-gesture.
    pub fn files_changed(&mut self, paths: &[String]) {
        for p in paths {
            if !self.changed_files.contains(p) {
                self.changed_files.push(p.clone());
            }
        }
        if self.follow_files && !self.explicit_tx && self.idle_tx.is_none() {
            self.reload_data();
        }
    }

    /// Recompile so linked files are read again.
    ///
    /// Not an edit: the document does not change, so the undo history stays
    /// valid and there is nothing to coalesce. That is the whole reason a live
    /// link is preferable to embedded values.
    pub fn reload_data(&mut self) {
        if self.changed_files.is_empty() {
            return;
        }
        self.status = match self.changed_files.len() {
            1 => format!("reread {}", self.changed_files[0]),
            n => format!("reread {n} files"),
        };
        self.changed_files.clear();
        self.dirty = true;
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Load a different document. The history goes with the old text: an undo
    /// recorded against a buffer that is gone would write bytes from it back
    /// into the live one.
    pub fn open(&mut self, text: impl Into<String>) {
        let schema = std::mem::replace(
            &mut self.schema,
            Schema {
                lilaq_version: String::new(),
                functions: Default::default(),
                elements: Default::default(),
                themes: Default::default(),
            },
        );
        let layout = self.layout;
        let follow_files = self.follow_files;
        *self = Editor::new(text, schema);
        self.layout = layout;
        self.follow_files = follow_files;
    }

    /// Tell the editor a compile is running, so it does not ask again.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Take a compiled frame: upload the pixels, adopt the scenes.
    ///
    /// A failed compile keeps the last good pixels on screen. A figure that
    /// blanks whenever the buffer is transiently invalid is unusable, and every
    /// keystroke in the source pane passes through invalid states.
    pub fn accept(
        &mut self,
        ctx: &egui::Context,
        render: Render,
        scenes: Vec<Scene>,
        data_files: Vec<DataFile>,
    ) {
        self.diagnostics = render.diagnostics.clone();
        // Before the failure check, deliberately. A compile that failed *because*
        // `csv("run.csv")` was not there is the case this list exists to explain,
        // and dropping it here would leave the panel showing the last compile
        // that worked -- i.e. claiming the file is fine.
        self.data_files = data_files;
        self.timing = format!(
            "compile {:.0} ms · raster {:.1} ms",
            render.compile_time.as_secs_f64() * 1000.0,
            render.render_time.as_secs_f64() * 1000.0,
        );
        if render.failed() {
            return;
        }
        // egui uploads a page as a single texture, and a texture wider than the
        // GPU allows is a *panic inside egui*, not an error anyone can handle.
        // Limits are real and low -- 2048 on some mobile GPUs -- and resizing a
        // diagram past one is an ordinary thing to do with the frame handles.
        //
        // Found by the random gesture walk on its first run: six resizes and a
        // gallery example was 2196 px across.
        let widest = render
            .pages
            .iter()
            .map(|p| p.size_pt.0.max(p.size_pt.1))
            .fold(0.0f64, f64::max);
        self.max_pixel_per_pt = match widest > 1.0 {
            true => (ctx.input(|i| i.max_texture_side) as f64 / widest) as f32,
            false => MAX_PIXEL_PER_PT,
        };
        if render.pixel_per_pt > self.max_pixel_per_pt {
            // Keep the last good pixels, exactly as a failed compile does, and
            // let `want_compile` ask again at a scale that fits -- `rendered_at`
            // is deliberately left alone so it sees the difference.
            return;
        }
        self.scenes = scenes;
        self.rendered_at = render.pixel_per_pt;
        self.textures.clear();
        self.pages.clear();
        for page in &render.pages {
            let image = egui::ColorImage::from_rgba_premultiplied(
                [page.image.width as usize, page.image.height as usize],
                &page.image.rgba,
            );
            let handle = ctx.load_texture(
                format!("lilook-page-{}", page.index),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.pages.push(PageTexture {
                texture: handle.id(),
                size_pt: page.size_pt,
            });
            self.textures.push(handle);
        }
    }

    /// Ask for a compile when the source changed, or when the view has zoomed
    /// far enough that the current pixels would show it.
    fn want_compile(&mut self, ctx: &egui::Context) {
        let want = (ctx.pixels_per_point() * self.canvas.zoom())
            .min(MAX_PIXEL_PER_PT)
            .min(self.max_pixel_per_pt);
        let stale_pixels = want > self.rendered_at * RESOLUTION_SLACK
            || want < self.rendered_at / RESOLUTION_SLACK;
        if self.dirty || (stale_pixels && !self.busy) {
            self.requests.compile = Some((self.doc.text().to_string(), want));
            self.dirty = false;
        }
    }

    /// Keyboard. Save is returned as a request rather than done here: a browser
    /// tab has nowhere to put the file.
    pub fn keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut as Sc, Modifiers as M};
        let mut hits = [false; 8];
        let mut pasted = None;
        ctx.input_mut(|i| {
            hits[0] = i.consume_shortcut(&Sc::new(M::COMMAND, Key::Z));
            hits[1] = i.consume_shortcut(&Sc::new(M::COMMAND | M::SHIFT, Key::Z));
            hits[2] = i.consume_shortcut(&Sc::new(M::COMMAND, Key::S));
            hits[3] = i.consume_shortcut(&Sc::new(M::COMMAND, Key::Num0));
            hits[4] = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            hits[5] = i.consume_shortcut(&Sc::new(M::COMMAND, Key::C));
            hits[6] = i.consume_shortcut(&Sc::new(M::COMMAND, Key::D));
            // Paste arrives as an event rather than a shortcut, because the
            // platform delivers the clipboard contents with it.
            pasted = i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            });
        });
        // Nothing below should fire while a text field has the keyboard.
        let typing = ctx.memory(|m| m.focused().is_some());
        if hits[0] && self.doc.undo() {
            self.dirty = true;
        }
        if hits[1] && self.doc.redo() {
            self.dirty = true;
        }
        if hits[2] {
            self.requests.save = true;
        }
        if hits[3] {
            self.canvas.fit();
        }
        if hits[4] && !typing {
            self.delete_selection();
        }
        if hits[5] && !typing {
            self.copy_selection(ctx);
        }
        if hits[6] && !typing {
            self.duplicate_selection();
        }
        if let Some(text) = pasted.filter(|_| !typing) {
            self.paste(text);
        }
    }

    /// Draw a frame and return what the shell has to act on.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        now: f64,
        banner: impl FnOnce(&mut egui::Ui),
    ) -> Requests {
        let ctx = ui.ctx().clone();
        self.requests = Requests::default();
        self.keys(&ctx);
        self.take_dropped(&ctx);
        self.tick_idle(&ctx, now);

        if self.layout.tree {
            egui::containers::Panel::left(egui::Id::new("calls"))
                .default_size(190.0)
                .resizable(true)
                .show(ui, |ui| self.call_list(ui));
        }

        let mut events = vec![];
        let mut source_edit: Option<(std::ops::Range<usize>, String)> = None;
        if self.layout.inspector {
            egui::containers::Panel::right(egui::Id::new("inspector"))
                .default_size(280.0)
                .resizable(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let call = self.doc.call(self.selected).cloned();
                            match call {
                                Some(call) => {
                                    // A set rule is an ordinary call site, so the
                                    // inspector renders it from the element's field
                                    // list with no special case.
                                    let element = self.schema.element_as_function(&call.callee);
                                    let f = element
                                        .as_ref()
                                        .or_else(|| self.schema.function_for_callee(&call.callee));
                                    let sources = self.slot_sources(&call);
                                    let context = lilook_ui::Context {
                                        recovered_points: self
                                            .scenes
                                            .iter()
                                            .flat_map(|s| &s.series)
                                            .find(|s| s.node == call.id)
                                            // Only a paired-point series has a
                                            // flat array to embed. A mesh has
                                            // axes, a rule has one coordinate per
                                            // argument, a distribution has whole
                                            // datasets -- and `points` is empty
                                            // for all three, so offering
                                            // "materialise" would write `()` into
                                            // the slot and break the figure. The
                                            // same shape of bug as seeding `xlim`
                                            // with an empty array.
                                            .filter(|s| {
                                                s.shape == lilook_core::SeriesShape::Points
                                                    && !s.points.is_empty()
                                            })
                                            .map(|s| s.points.len()),
                                        slot_sources: &sources,
                                    };
                                    let mut insp = Inspector::new(f).with_context(context);
                                    insp.ui(ui, &call);
                                    events = std::mem::take(&mut insp.events);
                                }
                                None => {
                                    ui.label("no call site selected");
                                }
                            }
                        });
                });
        }
        if self.layout.source {
            egui::containers::Panel::bottom(egui::Id::new("source"))
                .default_size(200.0)
                .resizable(true)
                .show(ui, |ui| {
                    if !self.diagnostics.is_empty() {
                        self.diagnostics_ui(ui);
                        ui.separator();
                    }
                    // The shell's slot: a file that changed on disk, a link back to a
                    // gallery. The editor has no idea what its host can offer.
                    banner(ui);
                    if !self.status.is_empty() {
                        ui.label(&self.status);
                        ui.separator();
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let id = egui::Id::new("source-pane");
                            let editing = ui.memory(|m| m.has_focus(id));
                            // Adopt the document whenever the pane is not being
                            // typed in, so a canvas drag or an undo shows up here.
                            let mut buf = match editing {
                                true => ui
                                    .data(|d| d.get_temp::<String>(id))
                                    .unwrap_or_else(|| self.doc.text().to_string()),
                                false => self.doc.text().to_string(),
                            };
                            let r = ui.add(
                                egui::TextEdit::multiline(&mut buf)
                                    .id(id)
                                    .code_editor()
                                    .desired_width(f32::INFINITY),
                            );
                            if r.changed() {
                                // Typing is a direct text edit, not model
                                // regeneration -- and a figure editor whose source
                                // pane is read-only is a strange object.
                                if let Some((range, value)) =
                                    lilook_core::minimal_replacement(self.doc.text(), &buf)
                                {
                                    source_edit = Some((range, value));
                                }
                            }
                            ui.data_mut(|d| d.insert_temp(id, buf));
                        });
                });
        }

        let mut canvas_events = vec![];
        egui::CentralPanel::default().show(ui, |ui| {
            let editable = self.editable_series();
            let out = self.canvas.ui(
                ui,
                CanvasInput {
                    pages: &self.pages,
                    scenes: &self.scenes,
                    selected: Some(self.selected),
                    editable: &editable,
                },
            );
            canvas_events = out.events.clone();
            if let Some((page, pt)) = out.hover {
                // Data coordinates when the pointer is inside a diagram, page
                // points otherwise: the first is what the user is thinking in.
                let text = match self
                    .scenes
                    .iter()
                    .find(|s| s.page == page && s.contains_page_point(pt))
                {
                    Some(s) => {
                        let d = s.transform.to_data(pt);
                        match &out.hovered {
                            Some((_, hit)) => {
                                // A mesh's field is the whole point of hovering
                                // one: the position is already on the axes, the
                                // value is not written anywhere.
                                // `gesture_num`, not `data_num`: this is a
                                // readout, not a value going into the document,
                                // and the shortest *round-tripping* form of a
                                // field value is seventeen digits of noise under
                                // a moving cursor.
                                let z = match s.field_at(hit) {
                                    Some(z) => format!("   z {}", lilook_core::gesture_num(z)),
                                    None => String::new(),
                                };
                                format!("{:.4}, {:.4}{z}   [#{}]", hit.data.0, hit.data.1, hit.node)
                            }
                            None => format!("{:.4}, {:.4}", d.0, d.1),
                        }
                    }
                    None => format!("p{page}  {:.1}, {:.1} pt", pt.0, pt.1),
                };
                ui.painter().text(
                    out.response.rect.left_bottom() + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    text,
                    egui::FontId::monospace(11.0),
                    ui.visuals().weak_text_color(),
                );
            }
            if self.pages.is_empty() {
                ui.painter().text(
                    out.response.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if self.diagnostics.is_empty() {
                        "compiling…"
                    } else {
                        "nothing rendered — see the diagnostics below"
                    },
                    egui::FontId::proportional(14.0),
                    ui.visuals().weak_text_color(),
                );
            }
        });

        self.handle_canvas(canvas_events);
        if let Some((range, value)) = source_edit {
            self.open_idle(now);
            self.apply(Intent::ReplaceRange { range, value });
        }
        self.handle(events, now);

        self.want_compile(&ctx);
        self.requests.query = self.queued_query.take();
        std::mem::take(&mut self.requests)
    }

    /// The single place UI events become transactions.
    fn handle(&mut self, events: Vec<UiEvent>, now: f64) {
        for e in events {
            match e {
                UiEvent::Begin { .. } => {
                    self.doc.begin("drag");
                    self.explicit_tx = true;
                    self.idle_tx = None;
                }
                UiEvent::Commit => {
                    self.doc.commit();
                    self.explicit_tx = false;
                }
                UiEvent::Set { node, param, value } => {
                    self.open_idle(now);
                    self.apply(Intent::SetNamedArg { node, param, value });
                }
                UiEvent::Insert { node, param, value } => {
                    self.open_idle(now);
                    self.apply(Intent::InsertNamedArg { node, param, value });
                }
                UiEvent::Remove { node, param } => {
                    self.open_idle(now);
                    self.apply(Intent::RemoveNamedArg { node, param });
                }
                UiEvent::Materialize { node, index } => self.unlock(node, index),
                UiEvent::GoToBinding { name, .. } => {
                    self.status = match self.doc.text().find(&format!("#let {name}")) {
                        Some(at) => format!("`{name}` bound at byte {at}"),
                        None => format!("`{name}` is not bound in this file"),
                    };
                }
            }
        }
    }

    /// Apply one intent through the editor, so its bookkeeping stays in step.
    ///
    /// Public for the frontends that edit a theme's body directly -- a `set-*`
    /// rule inside a `#let` is not reachable through any named-argument path.
    pub fn apply_intent(&mut self, intent: Intent) {
        self.apply(intent);
    }

    fn apply(&mut self, intent: Intent) {
        match self.doc.apply(intent) {
            Ok(()) => {
                self.dirty = true;
                self.status.clear();
            }
            Err(err) => self.status = err,
        }
    }

    /// Start (or extend) the transaction that closes itself once the user stops.
    fn open_idle(&mut self, now: f64) {
        if self.explicit_tx {
            return;
        }
        if self.idle_tx.is_none() {
            self.doc.begin("edit");
        }
        self.idle_tx = Some(now);
    }

    fn tick_idle(&mut self, ctx: &egui::Context, now: f64) {
        let Some(last) = self.idle_tx else { return };
        let left = IDLE_COMMIT_SECONDS - (now - last);
        if left <= 0.0 {
            self.doc.commit();
            self.idle_tx = None;
        } else {
            // Without this the commit would wait for the next input event, and
            // an undo pressed in between would take the half-finished edit.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    /// Write a computed data slot's evaluated values into the source.
    ///
    /// This is a large replacement rather than a surgical one, and it is
    /// deliberately an explicit user action: the values come from the compile,
    /// so nothing is regenerated from a model, and the expression the user
    /// wrote is what gets replaced.
    pub fn unlock(&mut self, node: usize, index: usize) {
        let Some(points) = self
            .scenes
            .iter()
            .flat_map(|s| &s.series)
            .find(|s| s.node == node)
            .map(|s| s.points.clone())
        else {
            self.status = "no evaluated data for that series yet".into();
            return;
        };
        let values: Vec<f64> = match index {
            0 => points.iter().map(|p| p.0).collect(),
            1 => points.iter().map(|p| p.1).collect(),
            _ => {
                self.status = format!("slot {index} has no recovered data");
                return;
            }
        };
        // The data emitter, not the geometry one: these values came from a file
        // or from an evaluated series, so six decimal places would silently
        // flatten anything smaller than a microunit and turn a masked sample
        // into a real zero. It also refuses rather than making the buffer
        // unusable, which is why this can fail.
        let value = match lilook_core::data_array_source(&values) {
            Ok(v) => v,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };

        // If this slot was linked, unlocking it may leave the file's binding with
        // nothing reading it. Leaving that behind would be worse than untidy: the
        // document would go on reading the file, so the Data panel would keep
        // listing it and the figure would look linked when it is not.
        let linked = self
            .doc
            .call(node)
            .and_then(|c| c.positional.get(index).cloned())
            .and_then(|slot| self.binding_behind(&slot));

        self.doc.begin(if linked.is_some() {
            "unlock data"
        } else {
            "materialise"
        });
        self.apply(Intent::SetPositionalArg { node, index, value });
        if let Some(name) = linked {
            self.drop_binding_if_unused(&name);
        }
        self.doc.commit();
    }

    /// Remove a `#let` nothing refers to any more, taking its line with it.
    ///
    /// Part of the same transaction as whatever orphaned it, so one undo puts
    /// both back.
    fn drop_binding_if_unused(&mut self, name: &str) {
        let Some(range) = self.doc.binding_of(name) else {
            return;
        };
        // Count mentions outside the binding itself. A plain substring scan would
        // match `run` inside `running`, so this walks identifiers instead.
        let text = self.doc.text().to_string();
        let used = self
            .doc
            .free_identifiers(0..text.len())
            .iter()
            .any(|n| n == name);
        if used {
            return;
        }
        // Take the newline before the binding too, so removing it does not leave
        // a blank line where the data used to come from.
        let start = text[..range.start].rfind('\n').map_or(range.start, |i| i);
        self.apply(Intent::ReplaceRange {
            range: start..range.end,
            value: String::new(),
        });
    }

    /// The name of the binding a slot reads its data through.
    fn binding_behind(&self, slot: &lilook_core::PositionalArg) -> Option<String> {
        if !slot.elements.is_empty() {
            return None;
        }
        self.doc
            .free_identifiers(slot.range.clone())
            .into_iter()
            .find(|name| {
                self.doc
                    .binding_of(name)
                    .is_some_and(|r| read_path(&self.doc.text()[r]).is_some())
            })
    }

    /// Apply what the canvas reported: a gesture, in the editor's own vocabulary.
    ///
    /// Public because a gesture is the editor's interface and not an internal
    /// step -- the same events reach it from the desktop window, the browser, and
    /// a test driving a random walk over a corpus of figures without an `egui`
    /// context in sight.
    pub fn handle_canvas(&mut self, events: Vec<CanvasEvent>) {
        for e in events {
            match e {
                CanvasEvent::Select(node) => self.selected = node,
                CanvasEvent::Begin => {
                    self.doc.begin("canvas");
                    self.explicit_tx = true;
                    self.idle_tx = None;
                }
                CanvasEvent::Commit => {
                    self.doc.commit();
                    self.explicit_tx = false;
                }
                CanvasEvent::SetLimits { figure, x, y } => {
                    // `gesture_num`, not `num`: a limit is a value on a data axis,
                    // and six decimal places writes `3e-9` as `0`. On a log axis
                    // that is a limit a pan reaches legitimately, and lilaq then
                    // refuses the figure -- "value must be strictly positive".
                    //
                    // And the axis's own scale decides what is writable at all.
                    // The canvas pans through `AxisMap::shifted`, which cannot
                    // leave a log axis, but `SetLimits` is the editor's public
                    // vocabulary and reaches it from three shells -- so the
                    // check belongs here, where the document is written, not in
                    // one of the callers. A log axis simply keeps the limits it
                    // had rather than being given a made-up positive number: the
                    // gesture overshot, and inventing a bound the user did not
                    // ask for is worse than declining the one they did.
                    let scales = self
                        .scenes
                        .iter()
                        .find(|s| s.figure == figure)
                        .map(|s| (s.transform.x.kind, s.transform.y.kind));
                    for (param, (lo, hi), scale) in [
                        ("xlim", x, scales.map(|s| s.0)),
                        ("ylim", y, scales.map(|s| s.1)),
                    ] {
                        let logarithmic = scale == Some(lilook_core::AxisScale::Log);
                        if logarithmic && (lo <= 0.0 || hi <= 0.0) {
                            continue;
                        }
                        let value = format!(
                            "({}, {})",
                            lilook_core::gesture_num(lo),
                            lilook_core::gesture_num(hi)
                        );
                        self.set_or_insert(figure, param, value);
                    }
                }
                CanvasEvent::SetSize {
                    figure,
                    width_pt,
                    height_pt,
                } => {
                    for (param, value) in [("width", width_pt), ("height", height_pt)] {
                        let Some(pt) = value else { continue };
                        self.set_length(figure, param, pt);
                    }
                }
                CanvasEvent::MoveRule { node, slot, to } => {
                    // One whole positional argument, not an array element: a rule
                    // *is* its coordinate.
                    self.apply(Intent::SetPositionalArg {
                        node,
                        index: slot,
                        value: lilook_core::gesture_num(to),
                    });
                }
                CanvasEvent::MovePoint { node, index, to } => {
                    // Where the coordinates *live* depends on the shape, and the
                    // edit has to match: a plot keeps parallel arrays, an
                    // annotation keeps two scalar arguments, a line keeps an
                    // `(x, y)` array per vertex. All three are two intents with
                    // two coalesce keys, and so one undo step.
                    let shape = self
                        .doc
                        .call(node)
                        .map(|c| c.series_shape())
                        .unwrap_or(lilook_core::SeriesShape::Points);
                    for (which, v) in [(0usize, to.0), (1, to.1)] {
                        let value = lilook_core::gesture_num(v);
                        let intent = match shape {
                            // `place(x, y, ..)`: the coordinates are the arguments.
                            lilook_core::SeriesShape::Anchor => Intent::SetPositionalArg {
                                node,
                                index: which,
                                value,
                            },
                            // `line(start, end)`: vertex `index` is a slot, and the
                            // coordinate is an element inside it.
                            lilook_core::SeriesShape::Vertices => Intent::SetArrayElement {
                                node,
                                arg: index,
                                element: which,
                                value,
                            },
                            // Parallel arrays: slot `which`, element `index`.
                            _ => Intent::SetArrayElement {
                                node,
                                arg: which,
                                element: index,
                                value,
                            },
                        };
                        self.apply(intent);
                    }
                }
            }
        }
    }

    /// Write a length back in the unit the user was already using.
    ///
    /// A figure written in centimetres should stay in centimetres: rewriting
    /// `width: 8cm` as `width: 226.77pt` is technically the same figure and a
    /// visible loss to whoever has to read the source afterwards.
    fn set_length(&mut self, node: usize, param: &str, points: f64) {
        let unit = self
            .doc
            .call(node)
            .and_then(|c| {
                let named = |name: &str| {
                    c.named
                        .iter()
                        .find(|a| a.name == name)
                        .and_then(|a| lilook_ui::split_numeric(&a.text))
                        .map(|(_, u)| u)
                };
                // This argument's unit, or the other dimension's -- a figure
                // with `height: 5cm` and no width should gain `width: 8cm`.
                named(param).or_else(|| named(if param == "width" { "height" } else { "width" }))
            })
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "cm".to_string());

        // 1pt is 1/72in, and typst's `pt` is the same.
        let value = match unit.as_str() {
            "cm" => points / 28.346_456_7,
            "mm" => points / 2.834_645_67,
            "in" => points / 72.0,
            "pt" => points,
            // `em` depends on the font size, and `%` on a container lilook
            // cannot see. Both would be a guess, so write points instead.
            _ => points,
        };
        let unit = match unit.as_str() {
            "cm" | "mm" | "in" | "pt" => unit,
            _ => "pt".to_string(),
        };
        self.set_or_insert(node, param, format!("{}{unit}", num(value)));
    }

    /// Rewrite a named argument, adding it when the user never wrote one. The
    /// first frame of a pan on a figure with automatic limits has to insert.
    fn set_or_insert(&mut self, node: usize, param: &str, value: String) {
        let present = self
            .doc
            .call(node)
            .is_some_and(|c| c.named.iter().any(|a| a.name == param));
        let intent = if present {
            Intent::SetNamedArg {
                node,
                param: param.into(),
                value,
            }
        } else {
            Intent::InsertNamedArg {
                node,
                param: param.into(),
                value,
            }
        };
        self.apply(intent);
    }

    fn delete_selection(&mut self) {
        let Some(call) = self.doc.call(self.selected) else {
            return;
        };
        if call.generated {
            self.status = "that call is produced by a loop or spread; edit the loop".into();
            return;
        }
        let what = call.callee.clone();
        self.doc.begin("delete");
        match self.doc.apply(Intent::RemoveNode {
            node: self.selected,
        }) {
            Ok(()) => {
                self.doc.commit();
                self.dirty = true;
                self.selected = 0;
                self.status = format!("deleted {what}");
            }
            Err(err) => self.status = err,
        }
    }

    /// Copy the selected call site: its source, plus the definitions it reads
    /// from outside itself.
    fn copy_selection(&mut self, ctx: &egui::Context) {
        let Some(call) = self.doc.call(self.selected) else {
            return;
        };
        let source = self.doc.text()[call.range.clone()].to_string();
        let module = call.module().map(str::to_string);
        let mut bindings = vec![];
        let mut unresolved = vec![];
        for name in self.doc.free_identifiers(call.range.clone()) {
            // The lilaq alias is resolved by the destination's own import, not
            // by carrying a binding across.
            if Some(&name) == module.as_ref() {
                continue;
            }
            match self.doc.binding_of(&name) {
                Some(r) => bindings.push((name, self.doc.text()[r].to_string())),
                None => unresolved.push(name),
            }
        }
        ctx.copy_text(source.clone());
        self.status = match (bindings.len(), unresolved.len()) {
            (0, 0) => format!("copied {}", call.callee),
            (b, 0) => format!("copied {} with {b} binding(s)", call.callee),
            (b, u) => format!("copied {} with {b} binding(s), {u} unresolved", call.callee),
        };
        self.clipboard = Some(Clip {
            source,
            bindings,
            unresolved,
        });
    }

    /// Paste into the selected figure (or the figure the selected series is in).
    ///
    /// Free-variable capture is the whole problem here: a copied series usually
    /// reads a `#let` that may not exist at the destination. lilook carries
    /// those definitions rather than inlining values, because inlining would
    /// turn a two-line figure into a wall of numbers and would silently drop
    /// the relationship the user wrote. Anything it cannot resolve is named in
    /// the status line instead of being discovered as a compile error.
    fn paste(&mut self, text: String) {
        let Some(figure) = self.paste_target() else {
            self.status = "select a figure to paste into".into();
            return;
        };
        let clip = self
            .clipboard
            .clone()
            .filter(|c| c.source == text)
            .unwrap_or(Clip {
                source: text,
                bindings: vec![],
                unresolved: vec![],
            });

        if let Err(e) = lilook_core::check_expr(&clip.source) {
            self.status = format!("clipboard is not a lilaq call: {e}");
            return;
        }

        self.doc.begin("paste");
        // The call goes in first. Call-site ids are indices into a
        // document-order walk, so inserting a binding above the figure
        // renumbers it -- `figure` would name something else by the time the
        // insertion ran.
        self.apply(Intent::InsertPositionalArg {
            node: figure,
            value: clip.source.clone(),
        });
        // Then the definitions it needs, each after the import so it is in
        // scope wherever the figure is.
        let mut carried = 0;
        for (name, definition) in &clip.bindings {
            if self.doc.binding_of(name).is_some() {
                continue;
            }
            let Some(at) = self.import_end() else {
                continue;
            };
            self.apply(Intent::ReplaceRange {
                range: at..at,
                value: format!("\n{definition}"),
            });
            carried += 1;
        }
        self.doc.commit();

        if self.status.is_empty() {
            self.status = match (carried, clip.unresolved.len()) {
                (0, 0) => "pasted".into(),
                (c, 0) => format!("pasted, carrying {c} binding(s)"),
                (c, _) => format!(
                    "pasted, carrying {c} binding(s); unresolved: {}",
                    clip.unresolved.join(", ")
                ),
            };
        }
        // Select what was just pasted: it is the last call inside the figure.
        if let Some(f) = self.doc.figures().into_iter().find(|f| f.node == figure) {
            if let Some(last) = f.series.last() {
                self.selected = *last;
            }
        }
    }

    /// Where a paste goes: the selected diagram, or the one the selection is in.
    fn paste_target(&self) -> Option<usize> {
        let call = self.doc.call(self.selected)?;
        if call.short_name() == "diagram" {
            return Some(call.id);
        }
        self.doc
            .figure_of(self.selected)
            .or_else(|| self.doc.figures().first().map(|f| f.node))
    }

    /// Duplicate in place: the same machinery, without the clipboard.
    fn duplicate_selection(&mut self) {
        let Some(call) = self.doc.call(self.selected) else {
            return;
        };
        if call.short_name() == "diagram" {
            self.status = "duplicating a whole figure is not supported yet".into();
            return;
        }
        let Some(figure) = self.doc.figure_of(self.selected) else {
            self.status = "that call is not inside a figure".into();
            return;
        };
        let source = self.doc.text()[call.range.clone()].to_string();
        self.doc.begin("duplicate");
        self.apply(Intent::InsertPositionalArg {
            node: figure,
            value: source,
        });
        self.doc.commit();
        if let Some(f) = self.doc.figures().into_iter().find(|f| f.node == figure) {
            if let Some(last) = f.series.last() {
                self.selected = *last;
            }
        }
    }

    /// Series the user could actually drag: their data is a literal array they
    /// wrote, not an expression lilook would have to rewrite.
    fn editable_series(&self) -> Vec<usize> {
        self.doc
            .calls()
            .iter()
            .filter(|c| {
                if c.generated {
                    return false;
                }
                // A rules series is movable when *every* coordinate is a literal
                // number, because the canvas gets one flag per call and a partly
                // computed `hlines(1, threshold)` would offer a drag it cannot
                // honour for one of the two lines.
                let rules = c.literal_rules();
                if !rules.is_empty() {
                    return rules.len() == c.positional.len();
                }
                // An annotation is movable when its two coordinates are literal;
                // a line or path when every vertex is a literal pair. Same rule
                // throughout: what lilook cannot rewrite, it does not offer.
                if c.has_literal_anchor() {
                    return true;
                }
                let vertices = c.literal_vertices();
                if !vertices.is_empty() {
                    return vertices.len() == c.positional.len();
                }
                c.has_literal_points()
            })
            .map(|c| c.id)
            .collect()
    }

    /// The document as a tree of figures and the series inside them, which is
    /// the structure the canvas selects in. Calls that belong to no diagram
    /// (`lq.linspace`, a stray helper) come last rather than being hidden.
    fn call_list(&mut self, ui: &mut egui::Ui) {
        let figures = self.doc.figures();
        let mut in_a_figure: Vec<usize> = vec![];
        for f in &figures {
            in_a_figure.push(f.node);
            in_a_figure.extend(&f.series);
        }
        let rules = self.doc.set_rules();
        let loose: Vec<(usize, String, bool)> = self
            .doc
            .calls()
            .iter()
            .filter(|c| !in_a_figure.contains(&c.id) && !rules.iter().any(|r| r.node == c.id))
            .map(|c| (c.id, c.callee.clone(), c.generated))
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for f in &figures {
                    let name = self
                        .doc
                        .call(f.node)
                        .map(|c| c.callee.clone())
                        .unwrap_or_default();
                    if ui
                        .selectable_label(self.selected == f.node, format!("{name}  #{}", f.node))
                        .clicked()
                    {
                        self.selected = f.node;
                    }
                    for &s in &f.series {
                        let Some(call) = self.doc.call(s) else {
                            continue;
                        };
                        let geom = self
                            .scenes
                            .iter()
                            .flat_map(|sc| &sc.series)
                            .find(|g| g.node == s);
                        // `SeriesGeom::summary` decides the wording, so a mesh
                        // cannot be described one way here and another way
                        // elsewhere -- and so a test can check it without a UI.
                        let label = match (call.generated, geom) {
                            (true, _) => format!("      {}  (generated)", call.callee),
                            (false, Some(g)) => {
                                format!("      {}  · {}", call.callee, g.summary())
                            }
                            (false, None) => format!("      {}", call.callee),
                        };
                        if ui.selectable_label(self.selected == s, label).clicked() {
                            self.selected = s;
                        }
                    }
                }
                if !loose.is_empty() {
                    ui.separator();
                    for (id, callee, generated) in loose {
                        let label = if generated {
                            format!("{callee}  (gen)")
                        } else {
                            callee
                        };
                        if ui.selectable_label(self.selected == id, label).clicked() {
                            self.selected = id;
                        }
                    }
                }

                // Set rules are document-level styling, not a property of any
                // one figure: `#show: lq.set-tick(..)` applies from where it
                // appears to the end of its scope. Listing them here, apart
                // from the figure tree, is what keeps the figure inspector
                // honest about what it is editing.
                ui.separator();
                self.theme_ui(ui);
                ui.horizontal(|ui| {
                    ui.label("document styles");
                    ui.weak("?").on_hover_text(
                        "`#show: lq.set-*(..)` show rules. Each applies from where it \
                         appears to the end of its enclosing scope, so one rule can \
                         restyle several figures.",
                    );
                });
                for r in &rules {
                    let scope = if r.document_level {
                        "rest of file"
                    } else {
                        "enclosing block"
                    };
                    let label = format!("{}  · {scope}", r.element);
                    if ui
                        .selectable_label(self.selected == r.node, label)
                        .clicked()
                    {
                        self.selected = r.node;
                    }
                }
                self.add_set_rule(ui, &rules);
                self.data_list(ui);
            });

        ui.separator();
        let (undo, redo) = self.doc.history_depth();
        ui.label(format!("undo {undo} · redo {redo}"));
        if !self.timing.is_empty() {
            ui.label(&self.timing);
        }
    }

    /// Start linking `path`, which must already be readable from the project
    /// root. Asks the compiler what columns it has.
    pub fn begin_link(&mut self, path: impl Into<String>) {
        let path = path.into();
        // A delimited file describes itself with its first row; a keyed one --
        // CBOR, JSON -- with its keys. A transcoded HDF5, npz or FITS file is the
        // second, because that is what lilook wrote it as.
        let expr = lilook_core::SourceKind::of(&path).columns_expr(&path);
        self.queued_query = Some(expr.clone());
        self.link = Some(Link::Asking { path, expr });
    }

    /// Take the answer to the query in `Requests::query`.
    ///
    /// `expr` is echoed back so a stale answer -- one for a link the user has
    /// since abandoned -- is discarded rather than applied to the wrong file.
    pub fn accept_answer(
        &mut self,
        expr: &str,
        answer: Option<lilook_core::Answer>,
        diagnostics: &[Diagnostic],
    ) {
        let Some(Link::Asking { path, expr: asked }) = &self.link else {
            return;
        };
        if asked != expr {
            return;
        }
        let path = path.clone();
        match answer {
            // A delimited file answers with its first row.
            Some(lilook_core::Answer::Strings(row)) if !row.is_empty() => {
                self.link_ready(path, lilook_core::columns_of(&row));
            }
            // A keyed file answers entry by entry, with each one's shape.
            Some(lilook_core::Answer::Fields(entries)) if !entries.is_empty() => {
                // Entries that are not arrays are not columns. A JSON file
                // commonly carries scalar metadata beside its data --
                // `{"title": "run 4", "age": [..]}` -- and offering `title` as
                // something to plot would produce a series of nothing.
                let entries: Vec<_> = entries.into_iter().filter(|(_, n, _)| *n > 0).collect();
                if entries.is_empty() {
                    self.link = Some(Link::Failed {
                        path,
                        why: "no arrays in it -- every entry is a single value".into(),
                    });
                    return;
                }
                let columns = lilook_core::Columns {
                    names: entries.iter().map(|(k, _, _)| k.clone()).collect(),
                    // A keyed file's answer *is* its names; there is no header
                    // row to tell from data.
                    has_header: true,
                    grids: entries
                        .iter()
                        // Outer length is rows, inner is columns -- the same
                        // row-major reading a mesh's field gets everywhere else.
                        .map(|(_, n, m)| (*m > 0).then_some((*m, *n)))
                        .collect(),
                };
                self.link_ready(path, columns);
            }
            _ => {
                let why = diagnostics
                    .iter()
                    .find(|d| d.severity == Severity::Error)
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "no rows, or not a delimited file".into());
                self.link = Some(Link::Failed { path, why });
            }
        }
    }

    /// The columns of the file an open link is waiting on, if it is waiting.
    pub fn link_columns(&self) -> Option<&[String]> {
        match &self.link {
            Some(Link::Ready { columns, .. }) => Some(&columns.names),
            _ => None,
        }
    }

    /// Why an open link failed, if it did.
    pub fn link_error(&self) -> Option<&str> {
        match &self.link {
            Some(Link::Failed { why, .. }) => Some(why),
            _ => None,
        }
    }

    /// Finish an open link, taking column `x` against column `y`.
    ///
    /// Returns false when no link is waiting for columns, so a caller that has
    /// lost track of the flow finds out rather than silently doing nothing.
    pub fn confirm_link(&mut self, x: usize, y: usize) -> bool {
        let z = match &self.link {
            Some(Link::Ready { z, .. }) => *z,
            _ => None,
        };
        self.confirm_field_link(x, y, z)
    }

    /// As [`confirm_link`](Self::confirm_link), also giving a mesh its field.
    pub fn confirm_field_link(&mut self, x: usize, y: usize, z: Option<usize>) -> bool {
        let Some(Link::Ready {
            path,
            kind,
            columns,
            ..
        }) = self.link.clone()
        else {
            return false;
        };
        self.commit_link(&path, kind, &columns, x, y, z);
        true
    }

    /// Abandon an open link.
    pub fn cancel_link(&mut self) {
        self.link = None;
    }

    /// A dropped file now lives at `path`, relative to the project root.
    pub fn file_adopted(&mut self, path: impl Into<String>) {
        let path = path.into();
        self.status = format!("{path} is in the project now");
        self.begin_link(path);
    }

    /// Report that a dropped file could not be brought in.
    pub fn adoption_failed(&mut self, name: &str, why: &str) {
        self.status = format!("could not add {name}: {why}");
    }

    /// Offer the file's entries, with sensible slots already chosen.
    ///
    /// A mesh is the only shape that can take a 2-D entry, so a field is offered
    /// as `z` only when the selected series is one; otherwise a file that holds
    /// nothing but a field has nothing to link and says so.
    fn link_ready(&mut self, path: String, columns: lilook_core::Columns) {
        let kind = lilook_core::SourceKind::of(&path);
        let (fields, plain) = columns.split_fields();
        let z = self
            .link_target()
            .filter(|_| self.target_is_mesh())
            .and(fields.first().copied());
        // x from the first column and y from the next: the overwhelmingly common
        // shape. For a mesh the field is `z`, so the axes come from what is left,
        // and when nothing is left `commit_link` uses the grid indices.
        let (x, y) = match z {
            Some(_) => (
                plain.first().copied().unwrap_or(0),
                plain
                    .get(1)
                    .copied()
                    .or(plain.first().copied())
                    .unwrap_or(0),
            ),
            None => (0, usize::from(columns.names.len() > 1)),
        };
        if z.is_none() && plain.is_empty() {
            self.link = Some(Link::Failed {
                path,
                why: "holds only two-dimensional data, which needs a colormesh, \
                      contour or mesh to link it to"
                    .into(),
            });
            return;
        }
        self.link = Some(Link::Ready {
            path,
            kind,
            columns,
            x,
            y,
            z,
        });
    }

    /// Is the series a link would land on a mesh?
    fn target_is_mesh(&self) -> bool {
        self.link_target()
            .and_then(|n| self.doc.calls().iter().find(|c| c.id == n))
            .is_some_and(|c| c.series_shape() == lilook_core::SeriesShape::Mesh)
    }

    /// Write the link: a `#let` for the file, and the two slots that read it.
    ///
    /// One transaction, so undo takes the whole thing back. The order matters for
    /// the same reason paste's does -- call-site ids are indices into a
    /// document-order walk, so the *slots* are set before the binding is inserted
    /// above them, or the node they name would have moved.
    fn commit_link(
        &mut self,
        path: &str,
        kind: lilook_core::SourceKind,
        columns: &lilook_core::Columns,
        x: usize,
        y: usize,
        z: Option<usize>,
    ) {
        let Some(node) = self.link_target() else {
            self.status = "select a series to link the file to".into();
            return;
        };
        let name = lilook_core::binding_name_for(path, |n| self.doc.binding_of(n).is_some());
        // A field's axes may not be in the file at all -- a FITS image is pixels
        // and nothing else -- so an axis that would otherwise name the field
        // becomes the grid's own indices, which is what the pixels are numbered
        // by anyway.
        let axis = |i: usize, n: usize| match columns.grid(i) {
            Some(_) => Some(format!("range({n})")),
            None => lilook_core::column_source(&name, kind, columns, i),
        };
        let (cols, rows) = z.and_then(|i| columns.grid(i)).unwrap_or((0, 0));
        let (Some(xs), Some(ys)) = (axis(x, cols), axis(y, rows)) else {
            self.status = "that column is not in the file".into();
            return;
        };
        let Some(at) = self.import_end() else {
            self.status = "no `#import` to put the data binding after".into();
            return;
        };

        self.doc.begin("link data file");
        self.apply(Intent::SetPositionalArg {
            node,
            index: 0,
            value: xs,
        });
        self.apply(Intent::SetPositionalArg {
            node,
            index: 1,
            value: ys,
        });
        if let Some(zs) = z.and_then(|i| lilook_core::column_source(&name, kind, columns, i)) {
            self.apply(Intent::SetPositionalArg {
                node,
                index: 2,
                value: zs,
            });
        }
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!(
                "\n#let {name} = {}",
                lilook_core::binding_source(path, kind, columns.has_header)
            ),
        });
        self.doc.commit();

        self.link = None;
        self.link_path.clear();
        if self.status.is_empty() {
            let label = |i: usize| {
                columns
                    .names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| i.to_string())
            };
            self.status = match z {
                Some(i) => format!("linked {path}: {} over {}×{}", label(i), cols, rows),
                None => format!("linked {path}: {} against {}", label(y), label(x)),
            };
        }
    }

    /// Where each of a call's data slots reads its numbers from.
    ///
    /// Worked out from the source, not from anything lilook stored: the slot
    /// expression mentions a binding, and the binding's own text says
    /// `csv("run.csv")`. So provenance cannot drift out of step with the
    /// document, cannot survive a copy into another project as a lie, and needs
    /// no lilook-only format for the compiler to be blind to.
    fn slot_sources(&self, call: &lilook_core::CallSite) -> Vec<lilook_ui::SlotSource> {
        call.positional
            .iter()
            .map(|slot| {
                let file = self.file_behind(slot);
                let (missing, stale) = match &file {
                    Some(f) => (
                        self.data_files.iter().any(|d| &d.path == f && !d.loaded),
                        self.changed_files.contains(f),
                    ),
                    None => (false, false),
                };
                lilook_ui::SlotSource {
                    file,
                    missing,
                    stale,
                }
            })
            .collect()
    }

    /// The file a slot's data comes from, if the source says so.
    ///
    /// One hop: the slot names a binding, and that binding reads a file. Deeper
    /// chains are not followed -- a wrong answer about where data came from is
    /// worse than no answer, and one hop is the shape lilook itself writes.
    fn file_behind(&self, slot: &lilook_core::PositionalArg) -> Option<String> {
        if !slot.elements.is_empty() {
            return None; // Literal values: nothing behind them.
        }
        for name in self.doc.free_identifiers(slot.range.clone()) {
            let Some(range) = self.doc.binding_of(&name) else {
                continue;
            };
            if let Some(path) = read_path(&self.doc.text()[range]) {
                return Some(path);
            }
        }
        None
    }

    /// Which series a link writes into: the selected one, if it is a series.
    fn link_target(&self) -> Option<usize> {
        let call = self.doc.call(self.selected)?;
        // A generated call is not the user's to edit -- the rule that keeps
        // lilook from writing into a `for` loop's output.
        (call.is_xy_series() && !call.generated).then_some(call.id)
    }

    /// The files this figure's data comes from.
    ///
    /// Veusz's equivalent is a list of datasets; lilook's is a list of *files*,
    /// because the datasets themselves are already visible in the figure tree
    /// above as the series that plot them. What is not visible anywhere else is
    /// which file each compile actually read, and whether it was there.
    fn data_list(&mut self, ui: &mut egui::Ui) {
        let files: Vec<DataFile> = self
            .data_files
            .iter()
            .filter(|d| d.is_data())
            .cloned()
            .collect();
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("data files");
            ui.weak("?").on_hover_text(
                "Files the last compile read. A figure that reads a file follows \
                 that file: change it and recompile, and the figure changes with \
                 no edit to the document.",
            );
            if !files.is_empty() {
                ui.checkbox(&mut self.follow_files, "follow").on_hover_text(
                    "Reread linked files as soon as they change, instead of \
                         offering to. Never lands mid-drag, and never while you \
                         are typing.",
                );
            }
        });

        // What changed, and the offer to act on it. Above the list, because it is
        // the one thing here that wants a decision.
        if !self.changed_files.is_empty() {
            let what = match self.changed_files.len() {
                1 => format!("{} changed on disk", self.changed_files[0]),
                n => format!("{n} linked files changed on disk"),
            };
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, what);
                if ui.small_button("reread").clicked() {
                    self.reload_data();
                }
            });
        }
        for f in &files {
            let label = match f.extension() {
                Some(ext) => format!("{}  · {ext}", f.path),
                None => f.path.clone(),
            };
            ui.horizontal(|ui| {
                if f.loaded {
                    ui.label(label);
                } else {
                    // The failure a diagnostic states in compiler terms ("file
                    // not found"), said in terms of the thing the user wanted.
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{label}  · missing"))
                        .on_hover_text(
                            "The figure asked for this file and it was not there. \
                             Typst resolves paths against the directory the \
                             document is in, and cannot read anything above it.",
                        );
                }
                if f.loaded && ui.small_button("link…").clicked() {
                    self.link_path = f.path.clone();
                    self.begin_link(f.path.clone());
                }
            });
        }
        self.link_ui(ui, files.is_empty());
    }

    /// The link flow: name a file, choose two columns, write the binding.
    fn link_ui(&mut self, ui: &mut egui::Ui, no_files_yet: bool) {
        // Two ways in, because a browser tab has no file picker worth the name
        // and a desktop window has no gallery: type a path, or drop the file on
        // the window.
        if self.link.is_none() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.link_path)
                        .hint_text("run.csv")
                        .desired_width(110.0),
                );
                let named = !self.link_path.trim().is_empty();
                if ui
                    .add_enabled(named, egui::Button::new("link…").small())
                    .clicked()
                {
                    let path = self.link_path.trim().to_string();
                    self.begin_link(path);
                }
            });
            if no_files_yet {
                ui.weak("or drop a data file on the window").on_hover_text(
                    "A figure reads files at compile time, so a linked file \
                         stays the source of truth: change it and the figure \
                         changes with it.",
                );
            }
            return;
        }

        let mut cancel = false;
        let mut confirm = None;
        match self.link.clone() {
            Some(Link::Asking { path, .. }) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(format!("reading {path}"));
                });
            }
            Some(Link::Failed { path, why }) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("{path}: {why}"));
                cancel |= ui.small_button("close").clicked();
            }
            Some(Link::Ready {
                path,
                columns,
                x,
                y,
                z,
                ..
            }) => {
                ui.weak(&path);
                let mut x = x;
                let mut y = y;
                let mut z = z;
                let (fields, plain) = columns.split_fields();
                let all: Vec<usize> = (0..columns.names.len()).collect();
                // Only entries of the right rank are offered for each slot: a
                // 2-D field is not an axis, and a column is not a field.
                let pick = |ui: &mut egui::Ui, label: &str, from: &[usize], sel: &mut usize| {
                    egui::ComboBox::from_label(label)
                        .selected_text(columns.names.get(*sel).cloned().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for i in from {
                                ui.selectable_value(sel, *i, &columns.names[*i]);
                            }
                        });
                };
                if let Some(sel) = &mut z {
                    egui::ComboBox::from_label("field")
                        .selected_text(columns.names.get(*sel).cloned().unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for i in &fields {
                                let (c, r) = columns.grid(*i).unwrap_or_default();
                                ui.selectable_value(
                                    sel,
                                    *i,
                                    format!("{} ({c}×{r})", columns.names[*i]),
                                );
                            }
                        });
                }
                if z.is_some() && plain.is_empty() {
                    ui.weak("axes: grid indices").on_hover_text(
                        "The file holds the field and nothing else -- an image \
                         is pixels -- so the axes are numbered by cell, which is \
                         what the pixels are numbered by anyway.",
                    );
                } else {
                    let from = if z.is_some() { &plain } else { &all };
                    pick(ui, "x", from, &mut x);
                    pick(ui, "y", from, &mut y);
                }
                if !columns.has_header {
                    ui.weak("no header row: columns are positional")
                        .on_hover_text(
                            "The first row parsed as numbers, so it is data \
                             rather than names, and every row is plotted.",
                        );
                }
                ui.horizontal(|ui| {
                    let target = self.link_target().is_some();
                    if ui
                        .add_enabled(target, egui::Button::new("link").small())
                        .on_disabled_hover_text("select a series in the tree above")
                        .clicked()
                    {
                        confirm = Some((x, y, z));
                    }
                    cancel |= ui.small_button("cancel").clicked();
                });
                // Keep the choices across frames.
                if let Some(Link::Ready {
                    x: sx,
                    y: sy,
                    z: sz,
                    ..
                }) = &mut self.link
                {
                    *sx = x;
                    *sy = y;
                    *sz = z;
                }
            }
            None => {}
        }
        if let Some((x, y, z)) = confirm {
            self.confirm_field_link(x, y, z);
        }
        if cancel {
            self.cancel_link();
        }
    }

    /// Files dropped on the window: link the ones typst can already read, and
    /// ask the shell to bring in the ones it cannot.
    ///
    /// A path that escapes the project root is not expressible in a document
    /// plain typst can compile, so there is no way to link `/scratch/run.h5`
    /// where it lies -- it has to come into the project first.
    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let name = f
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .or_else(|| Some(f.name.clone()))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "dropped".into());
            if name.ends_with(".typ") {
                self.status = "dropping a document is not linking data".into();
                continue;
            }
            self.requests.adopt.push(Dropped {
                name,
                path: f.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                bytes: f.bytes.as_ref().map(|b| b.to_vec()),
            });
        }
    }

    /// Insert a document-level `#show: lq.set-*(..)` after the lilaq import.
    ///
    /// Deliberately document-level and deliberately explicit: a scoped rule
    /// wrapped around one figure would change how the user's manuscript reads,
    /// and that is their decision to make in the source, not a side effect of
    /// touching an inspector.
    fn add_set_rule(&mut self, ui: &mut egui::Ui, existing: &[lilook_core::SetRule]) {
        let elements: Vec<String> = {
            let mut v: Vec<String> = self
                .schema
                .elements
                .keys()
                .filter(|e| !existing.iter().any(|r| &&r.element == e))
                .cloned()
                .collect();
            v.sort();
            v
        };
        if elements.is_empty() {
            return;
        }
        let mut picked = None;
        egui::ComboBox::from_id_salt("add-set-rule")
            .selected_text("add style…")
            .show_ui(ui, |ui| {
                for e in &elements {
                    if ui.selectable_label(false, e).clicked() {
                        picked = Some(e.clone());
                    }
                }
            });
        let Some(element) = picked else { return };

        let lq = self.doc.lilaq_alias();
        let Some(at) = self.import_end() else {
            self.status = "no lilaq import to place a style rule after".into();
            return;
        };
        let text = format!("\n#show: {lq}.set-{element}()");
        self.doc.begin("add style");
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: text,
        });
        self.doc.commit();
        // Select the new rule so its fields are there to fill in.
        if let Some(r) = self
            .doc
            .set_rules()
            .into_iter()
            .find(|r| r.element == element)
        {
            self.selected = r.node;
        }
    }

    /// The theme picker: lilaq's own, plus any the user has made here.
    fn theme_ui(&mut self, ui: &mut egui::Ui) {
        let active = self.active_theme();
        let current = active.as_ref().map(|t| t.name.clone());
        let mine: Vec<String> = self
            .doc
            .themes()
            .into_iter()
            .filter(|t| t.local)
            .map(|t| t.name)
            .collect();
        let mut pick: Option<Option<String>> = None;
        ui.horizontal(|ui| {
            ui.label("theme");
            ui.weak("?").on_hover_text(
                "A lilaq theme is a show rule -- `#show: lq.theme.ocean` -- so                  switching one is a single line in the document and nothing is                  stored outside it. Fork one to get a copy you can change.",
            );
            egui::ComboBox::from_id_salt("theme")
                .selected_text(current.clone().unwrap_or_else(|| "none".into()))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "none").clicked() {
                        pick = Some(None);
                    }
                    for name in self.schema.themes.iter().chain(&mine) {
                        let on = current.as_deref() == Some(name.as_str());
                        if ui.selectable_label(on, name).clicked() {
                            pick = Some(Some(name.clone()));
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            // Forking is how a theme becomes editable: lilaq's are functions in
            // a package, and a copy here is a `#let` whose overrides are the
            // same `set-*` rules listed below.
            if ui
                .small_button("fork…")
                .on_hover_text(
                    "Make a theme of your own that starts from this one. Its                      overrides appear below as ordinary style rules.",
                )
                .clicked()
            {
                let base = current.clone().unwrap_or_else(|| "theme".into());
                self.fork_theme(&format!("my-{base}"));
            }
            let is_mine = active.as_ref().is_some_and(|t| t.local);
            ui.add_enabled_ui(is_mine, |ui| {
                let id = ui.id().with("rename");
                let mut name = ui
                    .data(|d| d.get_temp::<String>(id))
                    .unwrap_or_else(|| current.clone().unwrap_or_default());
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .desired_width(90.0)
                        .hint_text("rename"),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.rename_theme(&name);
                    ui.data_mut(|d| d.remove::<String>(id));
                } else {
                    ui.data_mut(|d| d.insert_temp(id, name));
                }
            })
            .response
            .on_disabled_hover_text("fork a theme first -- lilaq's own cannot be renamed");
        });
        if let Some(name) = pick {
            self.set_theme(name.as_deref());
            self.mark_dirty();
        }
    }

    /// The theme in force, and where it is written.
    ///
    /// The *last* one, because show rules stack and the later transform wraps
    /// the earlier: what the reader sees on top is what the panel should name.
    pub fn active_theme(&self) -> Option<lilook_core::Theme> {
        self.doc.themes().into_iter().rfind(|t| t.document_level)
    }

    /// Apply a theme, replace the one in force, or remove it.
    ///
    /// One transaction and one show rule: `#show: lq.theme.ocean` is the entire
    /// representation, so switching is a byte-range replacement and removing is
    /// a deletion. Nothing is stored outside the document, which is why a themed
    /// figure pasted into another manuscript stays themed.
    pub fn set_theme(&mut self, name: Option<&str>) {
        let current = self.active_theme();
        let lq = self.doc.lilaq_alias();
        let rule = name.map(|n| match self.doc.binding_of(n) {
            // A theme of the user's own is named directly; lilaq's live under
            // the module.
            Some(_) => format!("#show: {n}"),
            None => format!("#show: {lq}.theme.{n}"),
        });
        self.doc.begin("set theme");
        match (current, rule) {
            (Some(t), Some(rule)) => self.apply(Intent::ReplaceRange {
                range: t.range,
                value: rule,
            }),
            // Removing takes the newline with it, or an empty line is left.
            (Some(t), None) => {
                let mut range = t.range;
                if self.doc.text()[..range.start].ends_with('\n') {
                    range.start -= 1;
                }
                self.apply(Intent::ReplaceRange {
                    range,
                    value: String::new(),
                })
            }
            (None, Some(rule)) => {
                let Some(at) = self.import_end() else {
                    self.status = "no lilaq import to place a theme after".into();
                    self.doc.commit();
                    return;
                };
                self.apply(Intent::ReplaceRange {
                    range: at..at,
                    value: format!("\n{rule}"),
                })
            }
            (None, None) => {}
        }
        self.doc.commit();
        self.status = match name {
            Some(n) => format!("theme: {n}"),
            None => "theme removed".into(),
        };
    }

    /// Derive a theme of the user's own from the one in force, under `name`.
    ///
    /// The new theme *composes* rather than copies:
    ///
    /// ```typst
    /// #let mine = it => { show: lq.theme.ocean; it }
    /// #show: mine
    /// ```
    ///
    /// Copying lilaq's body would mean chasing its imports -- `schoolbook` pulls
    /// in `@preview/tiptoe` -- and would silently go stale when lilaq revised a
    /// theme. Composing keeps the base authoritative, and every override added
    /// afterwards is an ordinary `set-*` rule the styles panel already edits.
    pub fn fork_theme(&mut self, name: &str) -> bool {
        let name = lilook_core::binding_name_for(name, |n| self.doc.binding_of(n).is_some());
        let lq = self.doc.lilaq_alias();
        let base = match self.active_theme() {
            Some(t) if t.local => format!("  show: {},\n", t.name),
            Some(t) => format!("  show: {lq}.theme.{},\n", t.name),
            // Deriving from nothing is still a theme -- an empty one to fill in.
            None => String::new(),
        };
        let base = base.replace(",\n", "\n");
        let Some(at) = self.import_end() else {
            self.status = "no lilaq import to place a theme after".into();
            return false;
        };
        self.doc.begin("fork theme");
        // The show rule first: inserting the binding above it would move the
        // range the replacement is about to name.
        match self.active_theme() {
            Some(t) => self.apply(Intent::ReplaceRange {
                range: t.range,
                value: format!("#show: {name}"),
            }),
            None => self.apply(Intent::ReplaceRange {
                range: at..at,
                value: format!("\n#show: {name}"),
            }),
        }
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!("\n#let {name} = it => {{\n{base}  it\n}}"),
        });
        self.doc.commit();
        self.status = format!("theme {name} is yours to edit");
        true
    }

    /// Rename a theme of the user's own, binding and show rule together.
    pub fn rename_theme(&mut self, to: &str) -> bool {
        let Some(theme) = self.active_theme().filter(|t| t.local) else {
            self.status = "only a theme of your own can be renamed".into();
            return false;
        };
        let to = lilook_core::binding_name_for(to, |n| {
            n != theme.name && self.doc.binding_of(n).is_some()
        });
        let Some(binding) = self.doc.binding_of(&theme.name) else {
            return false;
        };
        // The name inside the `#let`, found without touching anything else that
        // happens to spell it the same way.
        let text = self.doc.text();
        let Some(off) = text[binding.clone()].find(&theme.name) else {
            return false;
        };
        let at = binding.start + off;
        self.doc.begin("rename theme");
        // Later range first, so the earlier edit does not move it.
        self.apply(Intent::ReplaceRange {
            range: theme.transform.clone(),
            value: to.clone(),
        });
        self.apply(Intent::ReplaceRange {
            range: at..at + theme.name.len(),
            value: to.clone(),
        });
        self.doc.commit();
        self.status = format!("theme renamed to {to}");
        true
    }

    /// End of the line that imports lilaq: a set rule has to come after it, or
    /// the alias is not in scope.
    fn import_end(&self) -> Option<usize> {
        let text = self.doc.text();
        let at = text.find("#import")?;
        let end = text[at..].find('\n').map(|i| at + i)?;
        Some(end)
    }

    fn diagnostics_ui(&self, ui: &mut egui::Ui) {
        for d in &self.diagnostics {
            let (color, tag) = match d.severity {
                Severity::Error => (ui.visuals().error_fg_color, "error"),
                Severity::Warning => (ui.visuals().warn_fg_color, "warning"),
            };
            let where_ = match &d.range {
                Some(r) => format!(" (byte {})", r.start),
                None => String::new(),
            };
            ui.colored_label(color, format!("{tag}: {}{where_}", d.message));
            for h in &d.hints {
                ui.weak(format!("    hint: {h}"));
            }
        }
    }
}

/// Numbers going into Typst source: enough precision to be faithful to the
/// gesture, no trailing zeros to clutter the user's file.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// The path a file-reading expression names, if it names one literally.
///
/// Recognises typst's own readers, which is exactly the set a document can use to
/// get data from a file: `csv`, `cbor`, `json`, `toml`, `yaml`, `xml` and `read`.
/// A path built by an expression -- `csv("runs/" + name)` -- deliberately does
/// not match: the *file* is still tracked, because the compiler reports what it
/// read, but claiming to know which literal it was would be a guess.
fn read_path(source: &str) -> Option<String> {
    const READERS: [&str; 7] = ["csv", "cbor", "json", "toml", "yaml", "xml", "read"];
    for reader in READERS {
        let mut from = 0;
        while let Some(at) = source[from..].find(reader) {
            let at = from + at;
            from = at + reader.len();
            // Not a longer identifier that happens to end in `csv`.
            if source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                continue;
            }
            let rest = source[from..].trim_start();
            let Some(rest) = rest.strip_prefix('(') else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('"') else {
                continue; // Bytes, or a computed path: not a literal to name.
            };
            let mut path = String::new();
            let mut chars = rest.chars();
            loop {
                match chars.next() {
                    // The literal has to *be* the whole argument. In
                    // `csv("runs/" + name)` the path is computed, and answering
                    // "runs/" would be a confident wrong answer.
                    Some('"') => {
                        let after = chars.as_str().trim_start();
                        let whole = after.starts_with(')') || after.starts_with(',');
                        return whole.then_some(path);
                    }
                    Some('\\') => match chars.next() {
                        Some('n') => path.push('\n'),
                        Some(e) => path.push(e),
                        None => return None,
                    },
                    Some(c) => path.push(c),
                    None => return None,
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::read_path;

    #[test]
    fn a_files_path_is_read_out_of_the_expression_that_reads_it() {
        assert_eq!(
            read_path(r#"#let run = csv("run.csv", row-type: dictionary)"#).as_deref(),
            Some("run.csv")
        );
        assert_eq!(
            read_path(r#"#let d = cbor(".lilook/flux.cbor")"#).as_deref(),
            Some(".lilook/flux.cbor")
        );
        assert_eq!(
            read_path(r#"#let t = read("notes.txt")"#).as_deref(),
            Some("notes.txt")
        );
        // Escapes survive, since a file name can contain a quote.
        assert_eq!(
            read_path(r#"csv("a\"b.csv")"#).as_deref(),
            Some(r#"a"b.csv"#)
        );

        // Nothing to name.
        assert_eq!(read_path("#let x = (1, 2, 3)"), None);
        assert_eq!(read_path("#let x = lq.linspace(0, 10)"), None);
        // A computed path is a real link, but not a literal one, so lilook says
        // nothing rather than guessing which file it was.
        assert_eq!(read_path(r#"csv("runs/" + name)"#), None);
        assert_eq!(read_path("cbor(read(p, encoding: none))"), None);
        // An identifier that merely ends in a reader's name is not that reader.
        assert_eq!(read_path(r#"#let x = my-csv("run.csv")"#), None);
        assert_eq!(read_path(r#"#let x = d.read("run.csv")"#), None);
    }
}
