//! lilook in the browser.
//!
//! The same editor as the desktop build, in a page. What differs is only what a
//! browser cannot do: there is no file system (packages are baked in by
//! `build.rs`, the document comes from a gallery), and there is no thread to
//! compile on, so the compile happens in the frame. That is affordable because
//! it was measured first -- a lilaq figure of this size recompiles in 3-16 ms
//! inside wasm, which is under a frame at 60 Hz.

use lilook_compile::backend::Hints;
use lilook_compile::{Backend, MemoryFiles};
use lilook_core::{Document, Schema};
use lilook_editor::Editor;

const SCHEMA: &str = include_str!("../../../assets/lilaq-0.6.0.schema.json");

/// The lilaq packages, baked in by `build.rs`.
const PACKAGES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/packages.bin"));

/// The font files the page fetches, in the order `index.html` lists them.
/// Keeping the list here rather than only in the HTML is what lets a test check
/// it is sufficient -- a missing face is invisible, not an error.
pub const WEB_FONTS: &[&str] = &[
    "LibertinusSerif-Regular.otf",
    "LibertinusSerif-Italic.otf",
    "LibertinusSerif-Bold.otf",
    "NewCMMath-Book.otf",
];

/// The gallery. Each is a real lilaq figure, compiled by CI before it ships.
pub const EXAMPLES: &[(&str, &str)] = &[
    (
        "stacked area",
        include_str!("../examples/01-stacked-area.typ"),
    ),
    ("line plot", include_str!("../examples/02-line-plot.typ")),
    ("scatter", include_str!("../examples/03-scatter.typ")),
    ("bar chart", include_str!("../examples/04-bar.typ")),
    ("error bars", include_str!("../examples/05-errorbars.typ")),
    ("log scale", include_str!("../examples/06-log-scale.typ")),
];

/// Decode the bundle: `u32 key_len, key, u32 data_len, data`, repeated.
pub fn bundled_files() -> MemoryFiles {
    let mut files = MemoryFiles::new();
    let mut at = 0usize;
    let u32_at = |b: &[u8], at: usize| -> usize {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
    };
    while at + 4 <= PACKAGES.len() {
        let key_len = u32_at(PACKAGES, at);
        at += 4;
        let key = String::from_utf8_lossy(&PACKAGES[at..at + key_len]).into_owned();
        at += key_len;
        let data_len = u32_at(PACKAGES, at);
        at += 4;
        files.insert(key, PACKAGES[at..at + data_len].to_vec());
        at += data_len;
    }
    files
}

pub struct WebApp {
    editor: Editor,
    backend: Backend<MemoryFiles>,
    hints: Hints,
    example: usize,
    /// Set when the source pane should be the whole window: a phone has no room
    /// for three panels, and neither does a docs page in an iframe.
    narrow: bool,
}

impl Default for WebApp {
    fn default() -> Self {
        Self::new()
    }
}

impl WebApp {
    /// `fonts` are raw font files, fetched by the page. Without at least one
    /// text face typst can still lay a figure out, but every label comes out
    /// empty -- so a caller that has none gets a figure it cannot read.
    pub fn with_fonts(fonts: Vec<Vec<u8>>) -> Self {
        let mut app = Self::new();
        for data in fonts {
            app.backend.add_font_data(data);
        }
        app
    }

    pub fn new() -> Self {
        let schema = Schema::from_json(SCHEMA).expect("bundled schema");
        let editor = Editor::new(EXAMPLES[0].1, schema);
        let files = bundled_files();
        let backend = Backend::with_loader(files, "");
        WebApp {
            editor,
            backend,
            hints: Hints::new(),
            example: 0,
            narrow: false,
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub fn load(&mut self, index: usize) {
        self.example = index;
        self.editor.open(EXAMPLES[index].1);
        self.hints = Hints::new();
    }

    /// Compile in the frame. No thread, no actor: the editor asks for a source,
    /// gets pixels back before the frame ends, and nothing has to reconcile a
    /// result that arrived late.
    fn compile(&mut self, ctx: &egui::Context, source: String, ppp: f32) {
        let doc = Document::new(source);
        let (render, scenes) = self.backend.render_scenes(&doc, ppp, &mut self.hints);
        self.editor.accept(ctx, render, scenes);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut load = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("lilook");
            ui.label("·");
            for (i, (name, _)) in EXAMPLES.iter().enumerate() {
                if ui.selectable_label(self.example == i, *name).clicked() {
                    load = Some(i);
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.editor.layout.tree, "tree");
            ui.toggle_value(&mut self.editor.layout.inspector, "inspector");
            ui.toggle_value(&mut self.editor.layout.source, "source");
            ui.separator();
            if ui
                .button("copy source")
                .on_hover_text("the whole document, ready to paste into a manuscript")
                .clicked()
            {
                ui.ctx().copy_text(self.editor.text().to_string());
                self.editor.status = "source copied to the clipboard".into();
            }
            if ui
                .button("reset")
                .on_hover_text("back to the original example")
                .clicked()
            {
                load = Some(self.example);
            }
        });
        if let Some(i) = load {
            self.load(i);
        }
    }
}

impl eframe::App for WebApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

impl WebApp {
    /// One frame. Split out of the `eframe::App` impl so a native test can
    /// drive it: the browser pane is the only thing that cannot be checked
    /// without a browser, and everything else should not need one.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);

        // A narrow window cannot carry three panels; the figure is the thing
        // worth keeping.
        let narrow = ui.max_rect().width() < 900.0;
        if narrow != self.narrow {
            self.narrow = narrow;
            self.editor.layout.tree = !narrow;
            self.editor.layout.inspector = !narrow;
        }

        egui::containers::Panel::top(egui::Id::new("toolbar")).show(ui, |ui| self.toolbar(ui));

        let requests = self.editor.ui(ui, now, |_ui| {});
        if requests.save {
            self.editor.status =
                "this is the browser build — use “copy source”, or edit the text below".into();
        }
        if let Some((source, ppp)) = requests.compile {
            self.compile(&ctx, source, ppp);
        }
    }
}

/// The browser entry point, called by `index.html` once it has fetched the
/// fonts. Native builds of the workspace compile this crate too -- it shares
/// the workspace lock -- so the part that only exists in a browser is gated
/// rather than the whole crate being excluded.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start(fonts: js_sys::Array) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast as _;
    console_error_panic_hook::set_once();
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    let fonts: Vec<Vec<u8>> = fonts
        .iter()
        .filter_map(|v| v.dyn_into::<js_sys::Uint8Array>().ok())
        .map(|a| a.to_vec())
        .collect();

    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("lilook"))
        .expect("a #lilook canvas")
        .dyn_into::<web_sys::HtmlCanvasElement>()?;

    wasm_bindgen_futures::spawn_local(async move {
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |_cc| Ok(Box::new(WebApp::with_fonts(fonts)))),
            )
            .await
            .expect("failed to start eframe");
    });
    Ok(())
}
