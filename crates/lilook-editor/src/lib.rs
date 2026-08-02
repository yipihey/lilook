//! The editor: everything between a `Document` and a pair of eyes.
//!
//! Panels, selection, gestures, intents and transactions live here, over `egui`
//! and `lilook-ui` alone. It never compiles anything and never touches a file
//! system: the shell hands it a compiled frame and asks it for the next source
//! to compile. That is what lets the desktop window and the browser page be the
//! same editor rather than two that drift.

use lilook_core::render::{Render, Severity};
use lilook_core::scene::Scene;
use lilook_core::{Clip, DataFile, Intent, Schema, IDLE_COMMIT_SECONDS};
use lilook_ui::{Canvas, CanvasInput, Inspector, PageTexture};

/// A gesture, as the canvas reports it. Re-exported because `handle_canvas` is
/// the editor's public entry for one, and a shell should not have to depend on
/// `lilook-ui` to name what it is passing.
pub use lilook_core::{
    CanvasEvent, Clip as ClipData, Dropped, Link, Requests, Session, SlotSource,
};

/// Re-rasterise when the view zoom has drifted this far from the resolution the
/// current textures were produced at. Rendering is under a millisecond, but a
/// recompile is not, and every re-render costs one.
const RESOLUTION_SLACK: f32 = 1.35;
const MAX_PIXEL_PER_PT: f32 = 6.0;
/// Below this width the panels stop being panels. Three side-by-side columns
/// that each want 200 points leave nothing for the figure on a phone.
const NARROW_WIDTH: f32 = 640.0;

/// When this build was made, for the about box.
const BUILD_DATE: &str = env!("LILOOK_BUILD_DATE");

/// What to draw around the figure. A browser page has room for a gallery where
/// a window has a menu bar, so the shell decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub tree: bool,
    pub inspector: bool,
    pub source: bool,
    /// Whether the editor puts its own name at the top of the panel. A shell
    /// with a toolbar has already said `lilook` up there, and saying it twice --
    /// once as a heading and once as a row with a version beside it -- is one
    /// name too many. Such a shell turns this off and calls [`Editor::about_ui`]
    /// wherever its own name goes.
    pub about: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            tree: true,
            inspector: true,
            source: true,
            about: true,
        }
    }
}

/// The egui frontend: a [`Session`] and the pixels for it.
///
/// Everything that changes the document lives on the session; everything here
/// draws. `Deref` forwards the session's whole vocabulary, so a shell writes
/// `editor.set_theme(..)` or `editor.doc` exactly as before -- and so does any
/// other frontend, against `Session` directly and without egui.
pub struct Editor {
    pub session: Session,
    pub layout: Layout,
    canvas: Canvas,
    textures: Vec<egui::TextureHandle>,
    pages: Vec<PageTexture>,
    max_pixel_per_pt: f32,
    /// This frame's egui context, so the shared tail can ask for a compile
    /// without every layout threading it through.
    ctx: Option<egui::Context>,
    /// Where the source pane last scrolled to, so following the selection does
    /// not fight the user's own scrolling every frame.
    followed: Option<usize>,
    rendered_at: f32,
}

impl std::ops::Deref for Editor {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl std::ops::DerefMut for Editor {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

impl Editor {
    pub fn new(text: impl Into<String>, schema: Schema) -> Self {
        Editor {
            session: Session::new(text, schema),
            layout: Layout::default(),
            canvas: Canvas::new(),
            textures: vec![],
            pages: vec![],
            max_pixel_per_pt: MAX_PIXEL_PER_PT,
            ctx: None,
            followed: None,
            rendered_at: 1.0,
        }
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
        self.ctx = Some(ctx.clone());
        self.requests = Requests::default();
        self.keys(&ctx);
        self.take_dropped(&ctx);
        self.tick_idle(&ctx, now);

        let mut events = vec![];
        let mut source_edit: Option<(std::ops::Range<usize>, String)> = None;
        let mut canvas_events = vec![];

        // On a narrow screen the four parts stack in the order they read:
        // tree, inspector, figure, source. Not a squeezed desktop -- panels that
        // each want 200 points leave nothing for the figure on a phone -- but the
        // same sequence, top to bottom, in one scroll.
        if ui.available_width() < NARROW_WIDTH {
            egui::ScrollArea::vertical()
                .id_salt("stack")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.layout.tree {
                        self.call_list(ui);
                        ui.separator();
                    }
                    if self.layout.inspector {
                        events = self.inspector_ui(ui);
                        ui.separator();
                    }
                    // A fixed slice rather than "whatever is left": inside a
                    // scroll area the available height is unbounded, so the
                    // figure would take the whole column and push the source out
                    // of reach.
                    let tall = (ui.available_width() * 0.75).clamp(200.0, 420.0);
                    ui.allocate_ui(egui::vec2(ui.available_width(), tall), |ui| {
                        canvas_events = self.canvas_ui(ui);
                    });
                    if self.layout.source {
                        ui.separator();
                        source_edit = self.source_ui(ui, banner);
                    }
                });
            return self.finish(source_edit, events, canvas_events, now);
        }

        // Tree above, inspector below, in one column: what a thing *is* and what
        // it *can be set to* belong together, and it leaves the whole right side
        // to the figure with its source underneath.
        if self.layout.tree || self.layout.inspector {
            egui::containers::Panel::left(egui::Id::new("calls"))
                .default_size(250.0)
                .resizable(true)
                .show(ui, |ui| {
                    if self.layout.about {
                        self.about_ui(ui);
                    }
                    if self.layout.tree {
                        // Bounded when the inspector shares the column, or a long
                        // figure pushes it off the bottom.
                        let cap = match self.layout.inspector {
                            true => (ui.available_height() * 0.45).max(120.0),
                            false => f32::INFINITY,
                        };
                        egui::ScrollArea::vertical()
                            .id_salt("tree-scroll")
                            .max_height(cap)
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.call_list(ui));
                    }
                    if self.layout.inspector {
                        ui.separator();
                        events = self.inspector_ui(ui);
                    }
                });
        }

        if self.layout.source {
            egui::containers::Panel::bottom(egui::Id::new("source"))
                .default_size(200.0)
                .resizable(true)
                .show(ui, |ui| {
                    source_edit = self.source_ui(ui, banner);
                });
        }

        let mut canvas_events = vec![];
        egui::CentralPanel::default().show(ui, |ui| {
            canvas_events = self.canvas_ui(ui);
        });

        self.finish(source_edit, events, canvas_events, now)
    }

    /// Apply a frame's gestures and hand the shell its requests.
    ///
    /// One tail for both layouts, so the narrow stack cannot drift from the wide
    /// one in what it *does* -- only in where it draws.
    fn finish(
        &mut self,
        source_edit: Option<(std::ops::Range<usize>, String)>,
        events: Vec<lilook_core::UiEvent>,
        canvas_events: Vec<CanvasEvent>,
        now: f64,
    ) -> Requests {
        self.handle_canvas(canvas_events);
        if let Some((range, value)) = source_edit {
            self.open_idle(now);
            self.apply(Intent::ReplaceRange { range, value });
        }
        self.handle(events, now);

        let ctx = self.ctx.clone().expect("a context for this frame");
        self.want_compile(&ctx);
        // Anything imported and not yet put to the compiler goes now, if the
        // slot is free.
        self.pump_checks();
        self.requests.query = self.queued_query.take();
        self.requests.blame = std::mem::take(&mut self.queued_blame);
        self.requests.write_figure = self.queued_write.take();
        std::mem::take(&mut self.requests)
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

    /// The document as a tree of figures and the series inside them, which is
    /// the structure the canvas selects in. Calls that belong to no diagram
    /// (`lq.linspace`, a stray helper) come last rather than being hidden.
    /// The inspector's contents, with no panel around them.
    ///
    /// Factored out because where it goes now depends on the window: under the
    /// tree in the left column when there is room, and third in a single stack
    /// when there is not. Tree, inspector, figure, source reads the same either
    /// way -- what a thing is, what it can be set to, what it looks like, and
    /// what that is in Typst.
    fn inspector_ui(&mut self, ui: &mut egui::Ui) -> Vec<lilook_core::UiEvent> {
        let mut events = vec![];
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
                            saved: &self.prefs.saved,
                            map_stops: self.map_stops.as_ref(),
                        };
                        let mut insp = Inspector::new(f).with_context(context);
                        insp.ui(ui, &call);
                        events = std::mem::take(&mut insp.events);
                        // Handed over once. Left standing, it would re-seed the
                        // editor on every frame and wipe whatever the user had
                        // just changed in it.
                        self.map_stops = None;
                    }
                    None => {
                        ui.label("no call site selected");
                    }
                }
            });
        events
    }

    /// The figure itself, with no panel around it.
    ///
    /// Factored out for the same reason the inspector was: on a narrow screen it
    /// is the third thing in one scrolling column rather than whatever is left
    /// after the panels have taken their share.
    fn canvas_ui(&mut self, ui: &mut egui::Ui) -> Vec<CanvasEvent> {
        let mut canvas_events = vec![];
        let editable = self.editable_series();
        let out = self.canvas.ui(
            ui,
            CanvasInput {
                pages: &self.pages,
                // `self.session.scenes`, not `self.scenes`: the `Deref` would
                // borrow the whole editor and collide with `&mut self.canvas`.
                // Naming the field keeps the two borrows disjoint.
                scenes: &self.session.scenes,
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
        canvas_events
    }

    /// The source pane's contents, with no panel around them.
    fn source_ui(
        &mut self,
        ui: &mut egui::Ui,
        banner: impl FnOnce(&mut egui::Ui),
    ) -> Option<(std::ops::Range<usize>, String)> {
        let mut source_edit = None;
        // Copy the whole figure to the clipboard. lilook is often used
        // to compose a figure that then lives in someone else's
        // manuscript, and for that the source *is* the deliverable --
        // there is no file to hand over.
        ui.horizontal(|ui| {
            // Export first: it is what the whole tool is for. PDF
            // leads because that is what a journal takes -- SVG when a
            // co-author needs to nudge a label, PNG for slides.
            ui.menu_button("export…", |ui| {
                for (label, fmt) in [
                    ("PDF — for a paper", "pdf"),
                    ("SVG — vector, editable", "svg"),
                    ("PNG at 300 ppi — slides", "png"),
                ] {
                    if ui.button(label).clicked() {
                        self.request_export(fmt, 300.0);
                        ui.close();
                    }
                }
            });
            if ui
                .button("⇥")
                .on_hover_text(
                    "Tidy the source. A gesture appends an argument wherever the \
                     call happens to end, so a figure edited by pointing at it \
                     drifts out of shape.",
                )
                .clicked()
            {
                self.tidy();
            }
            if ui
                .button("⧉")
                .on_hover_text("copy the Typst source to the clipboard")
                .clicked()
            {
                ui.ctx().copy_text(self.doc.text().to_string());
                self.status = format!(
                    "copied {} lines of Typst to the clipboard",
                    self.doc.text().lines().count()
                );
            }
            ui.weak("source");
        });
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
                // Coloured from the document's own spans. The layouter runs on
                // every layout pass, so the spans are computed once per frame
                // outside it rather than per call.
                let spans = self.doc.spans();
                // What each series is really drawn in: its own `color:` or the
                // colour out of its `stroke:`, falling back to the cycle.
                let explicit: Vec<Option<egui::Color32>> = self
                    .doc
                    .figures()
                    .iter()
                    .flat_map(|f| f.series.clone())
                    .map(|id| {
                        let call = self.doc.call(id)?;
                        let named = |n: &str| call.named.iter().find(|a| a.name == n);
                        named("color")
                            .and_then(|a| lilook_ui::parse_color(&a.text))
                            .or_else(|| {
                                named("stroke")
                                    .and_then(|a| lilook_ui::parse_stroke(&a.text))
                                    .and_then(|s| s.paint)
                                    .and_then(|p| lilook_ui::parse_color(&p))
                            })
                    })
                    .collect();
                let series_color = move |i: usize| explicit.get(i).copied().flatten();
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let visuals = ui.visuals().clone();
                let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                    let job = lilook_ui::layout_job(
                        text.as_str(),
                        &spans,
                        font.clone(),
                        &visuals,
                        wrap,
                        &series_color,
                    );
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                // Follow the selection: clicking a curve should show the line
                // that drew it, or the source pane and the figure are two views
                // that happen to share a window rather than one document.
                let follow = self.doc.call(self.selected).map(|c| c.range.start);
                let changed = follow != self.followed;
                if changed {
                    self.followed = follow;
                }
                let out = egui::TextEdit::multiline(&mut buf)
                    .id(id)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui);
                // Where the error is, not just that there is one. A byte offset
                // in a message is a number to go and count; a mark under the
                // characters is the answer. Painted rather than styled through
                // the layouter so it survives the buffer being mid-edit.
                for d in self.diagnostics.clone() {
                    let Some(range) = d.range.clone() else {
                        continue;
                    };
                    if range.end > buf.len()
                        || !buf.is_char_boundary(range.start)
                        || !buf.is_char_boundary(range.end)
                    {
                        continue;
                    }
                    let color = match d.severity {
                        Severity::Error => ui.visuals().error_fg_color,
                        Severity::Warning => ui.visuals().warn_fg_color,
                    };
                    let ccursor = |b: usize| egui::text::CCursor::new(buf[..b].chars().count());
                    let (from, to) = (
                        out.galley.pos_from_cursor(ccursor(range.start)),
                        out.galley.pos_from_cursor(ccursor(range.end)),
                    );
                    let same_row = (from.top() - to.top()).abs() < 0.5;
                    let rect = egui::Rect::from_min_max(
                        out.galley_pos + from.left_top().to_vec2(),
                        out.galley_pos
                            + match same_row {
                                true => to.left_bottom().to_vec2(),
                                // Spilling over a line break: mark the first row
                                // to its end rather than boxing the whole
                                // paragraph, which would obscure more than it
                                // explains.
                                false => egui::vec2(out.galley.rect.right(), from.bottom()),
                            },
                    );
                    ui.painter()
                        .rect_filled(rect, 2.0, color.gamma_multiply(0.18));
                    ui.painter().line_segment(
                        [rect.left_bottom(), rect.right_bottom()],
                        egui::Stroke::new(1.5, color.gamma_multiply(0.8)),
                    );
                }
                // What the compiler resolved, painted at the end of the line it
                // belongs to. In the margin rather than inline: `TextEdit` lays
                // out the buffer and nothing else, and a number the user cannot
                // accidentally type into is the safer readout anyway.
                for hint in self.session.hints() {
                    if hint.at > buf.len() || !buf.is_char_boundary(hint.at) {
                        continue;
                    }
                    // At the end of the line it belongs to, so a hint never sits
                    // on top of the code it is describing.
                    let eol = buf[hint.at..]
                        .find('\n')
                        .map(|i| hint.at + i)
                        .unwrap_or(buf.len());
                    // `CCursor` counts characters, not bytes.
                    let chars = buf[..eol].chars().count();
                    let row = out.galley.pos_from_cursor(egui::text::CCursor::new(chars));
                    let at = out.galley_pos
                        + row.right_top().to_vec2()
                        + egui::vec2(ui.spacing().item_spacing.x * 2.0, 0.0);
                    ui.painter().text(
                        at,
                        egui::Align2::LEFT_TOP,
                        format!("⟨{}⟩", hint.text),
                        egui::TextStyle::Monospace.resolve(ui.style()),
                        ui.visuals().weak_text_color(),
                    );
                }
                // Completion and signature help, at the caret.
                //
                // The popup is an `egui::Area` placed from the galley's cursor
                // rect, which is why no new widget was needed: `TextEdit` already
                // reports where the caret is and which character it sits at.
                // Everything offered comes from `Session::completions`, which is
                // schema and parse only -- a completion that waited on the
                // compiler would arrive after the user had moved on.
                //
                // The caret is remembered rather than only read, because clicking
                // an offer takes the pane's focus -- and with it `cursor_range`,
                // which egui only reports for a focused field -- in the frame
                // between the press and the release. Without the memory the popup
                // vanishes mid-click and nothing in it can be chosen.
                let caret = out.cursor_range.filter(|_| has_focus(ui, id)).map(|range| {
                    let chars: usize = range.primary.index.into();
                    buf.char_indices()
                        .nth(chars)
                        .map(|(b, _)| b)
                        .unwrap_or(buf.len())
                });
                let caret_id = id.with("caret");
                if let Some(at) = caret {
                    ui.data_mut(|d| d.insert_temp(caret_id, at));
                }
                let last = ui.data(|d| d.get_temp::<usize>(caret_id));
                let accepted = caret
                    .or(last)
                    .filter(|a| *a <= buf.len())
                    .and_then(|at| self.completion_ui(ui, &buf, at, &out, caret.is_some()));
                // An accepted offer changed the document, and this pane shows its
                // own copy of the text while it has focus -- so the copy has to be
                // brought along, with the caret after what was written. Without
                // this the argument is in the document and the source pane still
                // shows the half-typed word that asked for it.
                if let Some(at) = accepted {
                    buf = self.doc.text().to_string();
                    ui.data_mut(|d| d.insert_temp(caret_id, at.min(buf.len())));
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                        let chars = buf[..at.min(buf.len())].chars().count();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(chars),
                            )));
                        state.store(ui.ctx(), id);
                    }
                }
                // Hovering a call explains it, so nobody has to go and find the
                // documentation for a function they are looking straight at.
                if let Some(pos) = ui.ctx().pointer_hover_pos() {
                    if out.response.rect.contains(pos) {
                        let rel = pos - out.galley_pos;
                        let ccursor = out.galley.cursor_from_pos(rel);
                        let chars: usize = ccursor.index.into();
                        let at = buf
                            .char_indices()
                            .nth(chars)
                            .map(|(b, _)| b)
                            .unwrap_or(buf.len());
                        if let Some(text) = self.describe_at(at) {
                            out.response.clone().show_tooltip_ui(|ui| {
                                ui.set_max_width(340.0);
                                ui.label(egui::RichText::new(text).monospace().size(11.0));
                            });
                        }
                    }
                }
                if changed {
                    if let Some(at) = follow.filter(|a| *a <= buf.len()) {
                        let row = out
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(buf[..at].chars().count()));
                        ui.scroll_to_rect(
                            egui::Rect::from_min_size(
                                out.galley_pos + row.left_top().to_vec2(),
                                egui::vec2(1.0, row.height()),
                            )
                            .expand2(egui::vec2(0.0, 40.0)),
                            None,
                        );
                    }
                }
                let r = out.response;
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
        source_edit
    }

    /// The completion popup and the signature line.
    ///
    /// Filtered by whatever word is already typed, so the list narrows as the
    /// user goes rather than making them read it. Accepting replaces that word,
    /// which is why the prefix is measured rather than assumed.
    ///
    /// `focused` is the caret's own gate: the signature line belongs to a pane
    /// someone is typing in, while the popup outlives focus by a frame so that a
    /// click on it can land -- see `lilook_ui::pick::popup`.
    ///
    /// Returns where the caret belongs when an offer was taken: after the text it
    /// wrote, so typing carries on from there.
    fn completion_ui(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        at: usize,
        out: &egui::text_edit::TextEditOutput,
        focused: bool,
    ) -> Option<usize> {
        // The word being typed: what a lilaq parameter or a colour map is spelt
        // with.
        let start = text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '-' || *c == '_')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(at);
        let prefix = &text[start..at];

        // What the caret is inside, always -- it costs a lookup, and it is the
        // thing that stops someone leaving for the documentation.
        if let Some(sig) = self.signature(at).filter(|_| focused) {
            let params = sig
                .params
                .iter()
                .map(|p| match Some(p) == sig.active.as_ref() {
                    true => format!("[{p}]"),
                    false => p.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            ui.weak(format!("{}({params})", sig.name));
            if !sig.doc.is_empty() {
                ui.weak(&sig.doc);
            }
        }

        // The same list, the same filter and the same popup the inspector's "add
        // argument" field uses -- see `lilook_ui::pick`. Two implementations of
        // one interaction is how they came to behave differently.
        let offers = self.completions(at);
        let matching: Vec<&lilook_core::Completion> = offers
            .iter()
            .filter(|c| {
                lilook_ui::pick::matches(
                    &lilook_ui::pick::haystack(&c.label, c.choices.iter().map(|(l, _)| l.as_str())),
                    prefix,
                )
            })
            .collect();
        let labels: Vec<Vec<String>> = matching
            .iter()
            .map(|c| c.choices.iter().map(|(l, _)| l.clone()).collect())
            .collect();
        let rows: Vec<lilook_ui::pick::Offer> = matching
            .iter()
            .zip(&labels)
            .map(|(c, l)| lilook_ui::pick::Offer::new(&c.label, &c.note, &c.insert).choices(l))
            .collect();
        let anchor = out.galley_pos
            + out
                .galley
                .pos_from_cursor(egui::text::CCursor::new(text[..at].chars().count()))
                .left_bottom()
                .to_vec2();
        let taken = lilook_ui::pick::popup(
            ui.ctx(),
            egui::Id::new("completions"),
            anchor,
            &rows,
            focused,
        )?;
        let c = matching[taken.row];
        // The value the pointer was on, where a row named several.
        let insert = match taken.choice.and_then(|k| c.choices.get(k)) {
            Some((_, insert)) => insert.clone(),
            None => c.insert.clone(),
        };
        // Replace the word being typed rather than appending to it -- and let the
        // core fit the result into the argument list, which is where the commas
        // come from.
        let (range, insert) = lilook_core::fit_argument(text, start..at, &insert);
        let start = range.start;
        // After the value, not after the comma that follows it.
        let written = insert.len() - usize::from(insert.ends_with(','));
        self.doc.begin("completion");
        self.apply(Intent::ReplaceRange {
            range,
            value: insert,
        });
        self.doc.commit();
        self.mark_dirty();
        // Back to the buffer: accepting an offer is the middle of typing, not the
        // end of it, and the click that accepted it took the focus.
        ui.memory_mut(|m| m.request_focus(out.response.id));
        Some(start + written)
    }

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
                self.figure_ui(ui);
                self.theme_ui(ui);
                self.library_ui(ui);
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
            // A library, not data. Dropping the file is the whole import
            // gesture -- there is no file dialog in either shell, and this one
            // works the same in the browser, where the alternative would be a
            // second way of getting bytes in.
            if name.ends_with(".toml") {
                let text = f
                    .bytes
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .or_else(|| {
                        f.path
                            .as_ref()
                            .and_then(|p| std::fs::read_to_string(p).ok())
                    });
                self.status = match text {
                    Some(text) => self.import_library(&text),
                    None => format!("could not read {name}"),
                };
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

    /// Who made this, under what licence, and where to find it -- **on the name
    /// itself**.
    ///
    /// The answer to a question asked once, so it costs no permanent space: the
    /// word `lilook` is the menu. It used to be a row of its own -- the name, the
    /// version, and an ⓘ button that rendered as a tofu box in the browser
    /// because the bundled fonts have no such glyph -- directly under a shell
    /// that had already said `lilook` at the top of the window. One name, one
    /// place, and a shell that draws its own can put this behind that one.
    pub fn about_ui(&mut self, ui: &mut egui::Ui) -> egui::Response {
        ui.menu_button(egui::RichText::new("lilook").strong(), |ui| {
            ui.set_max_width(260.0);
            ui.label(format!("lilook {}", env!("CARGO_PKG_VERSION")));
            ui.weak(format!("built {}", BUILD_DATE));
            ui.separator();
            ui.label("© Tom Abel");
            ui.weak("MIT licence");
            ui.separator();
            ui.hyperlink_to(
                "github.com/yipihey/lilook",
                "https://github.com/yipihey/lilook",
            );
            ui.weak("figures for lilaq, in Typst");
        })
        .response
        .on_hover_text(format!("lilook {}", env!("CARGO_PKG_VERSION")))
    }

    /// The two settings that decide whether a figure survives being put in a
    /// paper: how wide the column is, and how big the type is.
    ///
    /// Neither is exotic and both are routinely got wrong -- a figure drawn at
    /// whatever size the window happened to be, then rescaled on import, is how a
    /// paper ends up with six-point axis labels. The width goes on the diagram;
    /// the size is a typst `set text`, which is why the styles menu below cannot
    /// reach it: that lists lilaq's elements, and this is not one.
    fn figure_ui(&mut self, ui: &mut egui::Ui) {
        let diagrams: Vec<usize> = self.doc.figures().iter().map(|f| f.node).collect();
        if diagrams.is_empty() {
            return;
        }
        let mut width = None;
        let mut size = None;
        ui.horizontal(|ui| {
            ui.label("for print");
            ui.weak("?").on_hover_text(
                "Set the figure to the width of the column it will sit in, and its \
                 type to what the journal asks for. A figure scaled on import is \
                 the usual cause of unreadable axis labels.",
            );
            egui::ComboBox::from_id_salt("figure-width")
                .selected_text("width…")
                .show_ui(ui, |ui| {
                    for (label, value, note) in lilook_core::FIGURE_WIDTHS {
                        if ui
                            .selectable_label(false, format!("{label} — {value}"))
                            .on_hover_text(*note)
                            .clicked()
                        {
                            width = Some(*value);
                        }
                    }
                });
            egui::ComboBox::from_id_salt("figure-text")
                .selected_text("type…")
                .show_ui(ui, |ui| {
                    for (label, value, note) in lilook_core::FIGURE_TEXT_SIZES {
                        if ui
                            .selectable_label(false, *label)
                            .on_hover_text(*note)
                            .clicked()
                        {
                            size = Some(*value);
                        }
                    }
                });
        });
        // A figure can live in its own file, so it is findable, reusable and
        // openable on its own. Reversible, so it is a preference rather than a
        // commitment.
        if let Some(fig) = self.doc.figures().first().map(|f| f.node) {
            let imported = self.doc.text().contains(".lil\"");
            ui.horizontal(|ui| {
                if !imported
                    && ui
                        .small_button("to its own file…")
                        .on_hover_text(
                            "Move this figure to a `.lil` beside the document and \
                             import it. A `.lil` is a typst file -- the extension \
                             is only so your system knows to open it here.",
                        )
                        .clicked()
                {
                    let name = format!(
                        "{}.lil",
                        self.doc
                            .call(fig)
                            .map(|_| "figure".to_string())
                            .unwrap_or_default()
                    );
                    self.extract_figure(fig, &name);
                    self.mark_dirty();
                }
                if imported {
                    ui.weak("figure is in its own file")
                        .on_hover_text("open the .lil to edit it");
                }
            });
            // Only offered where a legend already exists to move -- lilaq
            // draws one automatically the moment a series has a `label:`,
            // so this is common, but a diagram with nothing labelled has no
            // legend for auto-placement to act on.
            let has_legend = self
                .scenes
                .iter()
                .find(|s| s.figure == fig)
                .is_some_and(|s| {
                    s.decorations
                        .iter()
                        .any(|(k, _)| *k == lilook_core::scene::Decoration::Legend)
                });
            if has_legend
                && ui
                    .small_button("auto-position legend")
                    .on_hover_text(
                        "Move the legend to whichever of the nine positions covers \
                         the least drawn data -- lilaq's own `loc=\"best\"`, spiked \
                         here ahead of a possible upstream patch. Any fill or stroke \
                         already set is kept.",
                    )
                    .clicked()
            {
                self.auto_position_legend(fig);
                self.mark_dirty();
            }
        }
        // Twin axis, offered where the series is: a second y-axis is a property
        // of one series, not of the diagram.
        if let Some(call) = self.doc.call(self.selected).cloned() {
            if call.is_xy_series() {
                let on = self.doc.on_secondary_axis(call.id);
                let mut want = on;
                ui.horizontal(|ui| {
                    ui.checkbox(&mut want, "second y-axis").on_hover_text(
                        "Draw this series against its own axis on the right, \
                             for a quantity in different units. It stays readable \
                             and editable here; it cannot be dragged on the canvas, \
                             because the canvas knows one scale per diagram.",
                    );
                });
                if want != on {
                    self.set_secondary_axis(call.id, want);
                    self.mark_dirty();
                }
            }
        }
        if let Some(w) = width {
            // Every diagram in the document: a figure made of panels is still one
            // figure, and setting the width of only the selected panel is never
            // what anyone means.
            self.doc.begin("figure width");
            for node in diagrams {
                self.set_or_insert(node, "width", w.to_string());
            }
            self.doc.commit();
            self.status = format!("figures set to {w} wide");
            self.mark_dirty();
        }
        if let Some(sz) = size {
            self.set_text_size(sz);
            self.mark_dirty();
        }
    }

    /// Set the type size, for a test that cannot open a menu.
    pub fn set_text_size_for_test(&mut self, size: &str) {
        self.set_text_size(size);
    }

    /// Set the figure's type size, replacing any `#set text(size: ..)` already
    /// there rather than stacking a second one.
    fn set_text_size(&mut self, size: &str) {
        let text = self.doc.text();
        let existing = text.find("#set text(").and_then(|at| {
            let end = text[at..].find(')')? + at + 1;
            text[at..end].contains("size:").then_some(at..end)
        });
        let Some(after) = self.import_end() else {
            self.status = "no lilaq import to place a text size after".into();
            return;
        };
        self.doc.begin("figure type size");
        match existing {
            Some(range) => self.apply(Intent::ReplaceRange {
                range,
                value: format!("#set text(size: {size})"),
            }),
            None => self.apply(Intent::ReplaceRange {
                range: after..after,
                value: format!("\n#set text(size: {size})"),
            }),
        }
        self.doc.commit();
        self.status = format!("figure type set to {size}");
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
        let mut use_saved: Option<String> = None;
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
                    // The user's own, from the library rather than from this
                    // document. Choosing one *brings it in* -- see
                    // `use_saved_theme` -- so the document still carries the
                    // whole theme afterwards.
                    let saved: Vec<String> = self
                        .prefs
                        .of(lilook_core::Kind::Theme)
                        .map(|s| s.name.clone())
                        .filter(|n| !mine.contains(n))
                        .collect();
                    if !saved.is_empty() {
                        ui.separator();
                        for name in saved {
                            if ui
                                .selectable_label(false, &name)
                                .on_hover_text("yours -- adding it writes it into this document")
                                .clicked()
                            {
                                use_saved = Some(name.clone());
                            }
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
            if ui
                .add_enabled(is_mine, egui::Button::new("keep").small())
                .on_hover_text(
                    "Keep this theme in your library, so another document can start \
                     from it. The document keeps its own copy either way.",
                )
                .on_disabled_hover_text("fork a theme first -- lilaq's own are not yours to keep")
                .clicked()
            {
                self.save_theme();
            }
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
        if let Some(name) = use_saved {
            self.use_saved_theme(&name);
            self.mark_dirty();
        }
    }

    /// Everything you have saved, in one place.
    ///
    /// The menus that offer these are attached to a parameter, so without this
    /// there is no way to see a palette without first selecting something that
    /// takes one -- and no way at all to rename one. Collapsed by default: it is
    /// a cupboard, not something to read while drawing.
    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let kinds = [
            (lilook_core::Kind::Cycle, "palettes"),
            (lilook_core::Kind::Colormap, "colour maps"),
            (lilook_core::Kind::Theme, "themes"),
        ];
        let total = self.prefs.saved.len();
        egui::CollapsingHeader::new(format!("your library ({total})"))
            .id_salt("library")
            .show(ui, |ui| {
                if total == 0 {
                    ui.weak("Nothing saved yet. Build a palette or a colour map with");
                    ui.weak("`new…` beside its menu, or keep a theme you forked.");
                }
                let mut rename: Option<(lilook_core::Kind, String, String)> = None;
                let mut forget: Option<(lilook_core::Kind, String)> = None;
                for (kind, label) in kinds {
                    let mine: Vec<lilook_core::Saved> = self.prefs.of(kind).cloned().collect();
                    if mine.is_empty() {
                        continue;
                    }
                    ui.weak(label);
                    for s in mine {
                        ui.horizontal(|ui| {
                            // The same painters the menus use, so a thing looks
                            // here exactly as it does where it is chosen.
                            match kind {
                                lilook_core::Kind::Cycle => lilook_ui::pick::swatches(ui, &s.value),
                                lilook_core::Kind::Colormap => lilook_ui::pick::ramp_of(
                                    ui,
                                    &lilook_ui::pick::cycle_parts(&s.value),
                                ),
                                lilook_core::Kind::Theme => {
                                    ui.weak("show rule");
                                }
                            }
                            let id = ui.id().with(("rename", kind, &s.name));
                            let mut name = ui
                                .data(|d| d.get_temp::<String>(id))
                                .unwrap_or_else(|| s.name.clone());
                            let edit =
                                ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                rename = Some((kind, s.name.clone(), name.clone()));
                                ui.data_mut(|d| d.remove::<String>(id));
                            } else if edit.has_focus() || name != s.name {
                                ui.data_mut(|d| d.insert_temp(id, name));
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text("remove from your library")
                                .clicked()
                            {
                                forget = Some((kind, s.name.clone()));
                            }
                        })
                        .response
                        .on_hover_text(&s.value);
                    }
                }
                if let Some((kind, from, to)) = rename {
                    self.status = match self.prefs.rename(kind, &from, &to) {
                        Ok(()) => {
                            self.prefs_dirty = true;
                            format!("{from} is called {to} now")
                        }
                        Err(why) => why,
                    };
                }
                if let Some((kind, name)) = forget {
                    if self.prefs.remove(kind, &name) {
                        self.prefs_dirty = true;
                        self.status = format!("{name} removed from your library");
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .small_button("export…")
                        .on_hover_text(
                            "Write the whole library out as a file you can keep or send \
                             on. Someone else adds it by dropping it on their window.",
                        )
                        .clicked()
                    {
                        self.requests.library_export = true;
                    }
                    ui.weak("drop a .toml here to add one");
                });
            });
    }

    fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        // A diagnostic with no location is a dead end until something finds its
        // cause. Offered rather than done: locating costs a compile per
        // candidate, and an error the user is still typing into is not worth it.
        let spanless = self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.range.is_none());
        if spanless
            && self.blames.is_empty()
            && ui
                .button("find the cause")
                .on_hover_text(
                    "lilaq reports most errors from inside itself, with no line to \
                     point at. lilook can find it by removing one thing at a time \
                     and recompiling -- a few milliseconds each.",
                )
                .clicked()
        {
            self.request_blame();
        }
        for b in self.blames.clone() {
            ui.horizontal(|ui| {
                ui.weak("caused by");
                if ui.link(&b.label).on_hover_text("select it").clicked() {
                    self.selected = b.node;
                }
            });
        }
        let mut chosen = None;
        for action in self.actions(&self.blames.clone()) {
            if ui
                .button(format!("↻ {}", action.label))
                .on_hover_text(&action.note)
                .clicked()
            {
                chosen = Some(action);
            }
        }
        if let Some(a) = chosen {
            self.apply_action(&a);
            self.blames.clear();
        }

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

/// Does the source pane have keyboard focus?
///
/// A caret nobody is at should not open a popup.
fn has_focus(ui: &egui::Ui, id: egui::Id) -> bool {
    ui.memory(|m| m.has_focus(id))
}
