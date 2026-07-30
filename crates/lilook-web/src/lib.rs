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
    ("plot grid", include_str!("../examples/07-plot-grid.typ")),
    ("colormesh", include_str!("../examples/08-colormesh.typ")),
    ("thresholds", include_str!("../examples/09-rules.typ")),
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

/// Where a transcoded file lands. The same `.lilook/` the desktop shell uses, so
/// a document written in one shell links in the other.
fn sidecar_name(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    format!(".lilook/{stem}.cbor")
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
        // The bundle is the whole file system here, so this lists the packages'
        // own sources plus anything the user dropped in -- and the panel's filter
        // is what makes the second visible among the first.
        let data_files = self.backend.dependencies();
        self.editor.accept(ctx, render, scenes, data_files);
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
        self.take_deliveries();

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
        for d in requests.adopt {
            self.adopt(d);
        }
        if let Some(expr) = requests.query {
            // In process and in the frame, like the compile: there is no thread
            // to wait for, so the answer is already there when the editor next
            // draws.
            let (answer, diagnostics) = self.backend.query(&expr);
            self.editor.accept_answer(&expr, answer, &diagnostics);
        }
        if let Some((source, ppp)) = requests.compile {
            self.compile(&ctx, source, ppp);
        }
    }

    /// Take a dropped file into the page's file system.
    ///
    /// There is no disk here, so "bringing the file into the project" means
    /// putting its bytes where the compiler's loader will find them. That makes
    /// a browser link real in every way except durability: it lasts as long as
    /// the tab, because there is nowhere else for it to live.
    /// Put a file into this tab's file system, under the project root.
    ///
    /// Public because the page needs it: a drop arrives through egui, but a file
    /// decoded in JavaScript -- an HDF5 read by h5wasm, say -- arrives from
    /// there, and both end up here.
    pub fn insert_file(&mut self, name: &str, bytes: Vec<u8>) {
        self.backend
            .loader_mut()
            .insert(format!("project/{name}"), bytes);
    }

    fn adopt(&mut self, d: lilook_editor::Dropped) {
        let Some(bytes) = d.bytes else {
            self.editor
                .adoption_failed(&d.name, "the drop carried no contents");
            return;
        };

        // A format typst cannot read becomes a CBOR sidecar, exactly as it does
        // on the desktop -- the decoders take `&[u8]` and so port unchanged.
        if let Some(format) = lilook_data::sniff(&bytes, &d.name) {
            if !format.available() {
                // HDF5 is the case this exists for: libhdf5 is C and has no wasm
                // build, so the page reads it in JavaScript instead and calls
                // `insert_columns`. Saying so beats a silent failure.
                if matches!(format, lilook_data::Format::Hdf5) {
                    // Not a dead end here: the page is loading h5wasm and will
                    // call `deliver_columns`. Say that rather than "unavailable",
                    // which would be true of this Rust build and false of lilook.
                    self.editor.status = format!("reading {} with h5wasm...", d.name);
                } else {
                    self.editor
                        .adoption_failed(&d.name, format.unavailable_because());
                }
                return;
            }
            match lilook_data::decode(&bytes, format) {
                Ok(data) if !data.columns.is_empty() => {
                    let n = data.columns.len();
                    let rel = sidecar_name(&d.name);
                    self.insert_file(&rel, data.to_cbor());
                    self.editor.file_adopted(rel.clone());
                    self.editor.status = format!("read {n} column(s) from {} into {rel}", d.name);
                    return;
                }
                Ok(_) => {
                    self.editor
                        .adoption_failed(&d.name, "nothing in it is a column of numbers");
                    return;
                }
                Err(e) => {
                    self.editor.adoption_failed(&d.name, &e.to_string());
                    return;
                }
            }
        }

        let n = bytes.len();
        self.insert_file(&d.name, bytes);
        self.editor.file_adopted(d.name.clone());
        self.editor.status = format!("{} ({n} bytes) is in this tab's file system", d.name);
    }

    /// Columns decoded outside Rust, e.g. an HDF5 file read by h5wasm.
    ///
    /// Turned into the same CBOR sidecar a native transcode produces, so the rest
    /// of the feature -- linking, refreshing, unlocking -- does not know or care
    /// which path the numbers came in by.
    pub fn insert_columns(&mut self, name: &str, names: Vec<String>, values: Vec<Vec<f64>>) {
        let columns: Vec<lilook_data::Column> = names
            .into_iter()
            .zip(values)
            .map(|(name, values)| lilook_data::Column { name, values })
            .collect();
        if columns.is_empty() {
            self.editor
                .adoption_failed(name, "no columns of numbers came back");
            return;
        }
        let n = columns.len();
        let rel = sidecar_name(name);
        let data = lilook_data::Dataset { columns };
        self.insert_file(&rel, data.to_cbor());
        self.editor.file_adopted(rel.clone());
        self.editor.status = format!("read {n} column(s) from {name} into {rel}");
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

// Columns decoded in JavaScript, waiting for the next frame to collect them.
//
// A mailbox rather than a handle, because `eframe::WebRunner` owns the app and
// JavaScript has no way to reach into it. Also the natural shape for the job:
// reading an HDF5 file is asynchronous, and the frame that asked for it is long
// over by the time the answer arrives.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static INBOX: std::cell::RefCell<Vec<Delivered>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// What JavaScript hands back for a file Rust could not read here.
#[cfg(target_arch = "wasm32")]
enum Delivered {
    Columns {
        name: String,
        names: Vec<String>,
        values: Vec<Vec<f64>>,
    },
    Failed {
        name: String,
        why: String,
    },
}

/// Hand lilook the columns of a file the page decoded itself.
///
/// This is how HDF5 works in a browser: libhdf5 is C and has no wasm build, so
/// `index.html` loads h5wasm -- lazily, only once someone actually drops an
/// `.h5` -- reads the datasets, and calls this. What arrives becomes the same
/// CBOR sidecar a native transcode produces, so linking, rereading and unlocking
/// do not know which route the numbers took.
///
/// `values` is an array of `Float64Array`, one per name.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn deliver_columns(name: String, names: js_sys::Array, values: js_sys::Array) {
    use wasm_bindgen::JsCast as _;
    let names: Vec<String> = names.iter().filter_map(|v| v.as_string()).collect();
    let values: Vec<Vec<f64>> = values
        .iter()
        .filter_map(|v| v.dyn_into::<js_sys::Float64Array>().ok())
        .map(|a| a.to_vec())
        .collect();
    INBOX.with_borrow_mut(|q| {
        q.push(Delivered::Columns {
            name,
            names,
            values,
        })
    });
}

/// Report that the page could not decode a file after all -- h5wasm failed to
/// load, or the file was not what it claimed. Better in the panel than in a
/// console nobody has open.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn deliver_error(name: String, why: String) {
    INBOX.with_borrow_mut(|q| q.push(Delivered::Failed { name, why }));
}

impl WebApp {
    /// Collect anything JavaScript decoded since the last frame.
    #[cfg(target_arch = "wasm32")]
    fn take_deliveries(&mut self) {
        let items: Vec<Delivered> = INBOX.with_borrow_mut(std::mem::take);
        for item in items {
            match item {
                Delivered::Columns {
                    name,
                    names,
                    values,
                } => self.insert_columns(&name, names, values),
                Delivered::Failed { name, why } => self.editor.adoption_failed(&name, &why),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn take_deliveries(&mut self) {}
}
