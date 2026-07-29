//! The editor: everything between a `Document` and a pair of eyes.
//!
//! Panels, selection, gestures, intents and transactions live here, over `egui`
//! and `lilook-ui` alone. It never compiles anything and never touches a file
//! system: the shell hands it a compiled frame and asks it for the next source
//! to compile. That is what lets the desktop window and the browser page be the
//! same editor rather than two that drift.

use lilook_core::render::{Diagnostic, Render, Severity};
use lilook_core::scene::Scene;
use lilook_core::{Document, Intent, Schema};
use lilook_ui::{Canvas, CanvasEvent, CanvasInput, Inspector, PageTexture, UiEvent};

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
    scenes: Vec<Scene>,
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
            scenes: vec![],
            diagnostics: vec![],
            rendered_at: 1.0,
            dirty: true,
            explicit_tx: false,
            idle_tx: None,
            busy: false,
            timing: String::new(),
            clipboard: None,
            requests: Requests::default(),
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
            },
        );
        let layout = self.layout;
        *self = Editor::new(text, schema);
        self.layout = layout;
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
    pub fn accept(&mut self, ctx: &egui::Context, render: Render, scenes: Vec<Scene>) {
        self.diagnostics = render.diagnostics.clone();
        self.timing = format!(
            "compile {:.0} ms · raster {:.1} ms",
            render.compile_time.as_secs_f64() * 1000.0,
            render.render_time.as_secs_f64() * 1000.0,
        );
        if render.failed() {
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
        let want = (ctx.pixels_per_point() * self.canvas.zoom()).min(MAX_PIXEL_PER_PT);
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
        self.tick_idle(&ctx, now);

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
                                    let context = lilook_ui::Context {
                                        recovered_points: self
                                            .scenes
                                            .iter()
                                            .flat_map(|s| &s.series)
                                            .find(|s| s.node == call.id)
                                            .map(|s| s.points.len()),
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
                                format!("{:.4}, {:.4}   [#{}]", hit.data.0, hit.data.1, hit.node)
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
                UiEvent::Materialize { node, index } => self.materialize(node, index),
                UiEvent::GoToBinding { name, .. } => {
                    self.status = match self.doc.text().find(&format!("#let {name}")) {
                        Some(at) => format!("`{name}` bound at byte {at}"),
                        None => format!("`{name}` is not bound in this file"),
                    };
                }
            }
        }
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
    fn materialize(&mut self, node: usize, index: usize) {
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
        let value = lilook_ui::value::array_source(values.into_iter());
        self.doc.begin("materialise");
        self.apply(Intent::SetPositionalArg { node, index, value });
        self.doc.commit();
    }

    fn handle_canvas(&mut self, events: Vec<CanvasEvent>) {
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
                    for (param, (lo, hi)) in [("xlim", x), ("ylim", y)] {
                        let value = format!("({}, {})", num(lo), num(hi));
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
                CanvasEvent::MovePoint { node, index, to } => {
                    // x and y are separate array elements, so a point drag is
                    // two intents with two coalesce keys -- and one undo step.
                    for (arg, v) in [(0usize, to.0), (1, to.1)] {
                        let intent = Intent::SetArrayElement {
                            node,
                            arg,
                            element: index,
                            value: num(v),
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
            .filter(|c| !c.generated && c.has_literal_points())
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
                        let points = self
                            .scenes
                            .iter()
                            .flat_map(|sc| &sc.series)
                            .find(|g| g.node == s)
                            .map(|g| g.points.len());
                        let label = match (call.generated, points) {
                            (true, _) => format!("      {}  (generated)", call.callee),
                            (false, Some(n)) => format!("      {}  · {n} pts", call.callee),
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
            });

        ui.separator();
        let (undo, redo) = self.doc.history_depth();
        ui.label(format!("undo {undo} · redo {redo}"));
        if !self.timing.is_empty() {
            ui.label(&self.timing);
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

        let lq = self
            .doc
            .calls()
            .iter()
            .find_map(|c| c.module())
            .unwrap_or("lq")
            .to_string();
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
