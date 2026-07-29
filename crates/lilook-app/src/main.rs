//! The desktop shell. Deliberately thin: a window, a file, a compile thread,
//! and a screenshot flag. Everything you can see and click lives in
//! `lilook-editor`, which the browser build runs unchanged.

use std::path::PathBuf;

use lilook_compile::CompileActor;
use lilook_core::Schema;
use lilook_editor::Editor;

const SCHEMA: &str = include_str!("../../../assets/lilaq-0.6.0.schema.json");

struct App {
    editor: Editor,
    path: Option<PathBuf>,
    actor: CompileActor,
    /// Current window title, so it is only pushed when it changes.
    title: String,
    /// What was last written to (or read from) disk, so unsaved changes are a
    /// fact rather than a guess.
    saved_text: String,
    /// Modification time of the file when lilook last read or wrote it.
    saved_at: Option<std::time::SystemTime>,
    /// The file changed on disk under us and the user has to choose.
    conflict: bool,
    /// Frame time of the last staleness check.
    checked_at: f64,
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
        App {
            editor: Editor::new(
                text.clone(),
                Schema::from_json(SCHEMA).expect("bundled schema"),
            ),
            path: path.clone(),
            actor,
            title: String::new(),
            saved_text: text,
            saved_at: mtime(path.as_ref()),
            conflict: false,
            checked_at: 0.0,
            screenshot: screenshot.map(|path| Screenshot {
                path,
                countdown: 0,
                asked: false,
            }),
        }
    }

    /// Feed the compile thread and take back whatever it finished.
    fn pump(&mut self, ctx: &egui::Context, request: Option<(String, f32)>) {
        if let Some((source, ppp)) = request {
            self.actor.request(source, ppp);
        }
        self.editor.set_busy(self.actor.busy());
        if let Some(frame) = self.actor.take_latest() {
            self.editor.accept(ctx, frame.render, frame.scenes);
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

    fn unsaved(&self) -> bool {
        self.editor.text() != self.saved_text
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

/// The file's modification time, if it exists.
fn mtime(path: Option<&PathBuf>) -> Option<std::time::SystemTime> {
    std::fs::metadata(path?).ok()?.modified().ok()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.check_disk(now);
        self.retitle(&ctx);

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
        self.pump(&ctx, requests.compile);
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
