//! A random walk of gestures over the whole example gallery.
//!
//! `random_intents_fully_undo` in `lilook-core` establishes the invariant one
//! level down: any sequence of *intents*, fully undone, returns the buffer
//! byte-for-byte. It runs without a compiler, so it cannot say whether what the
//! intents produced was a document typst would accept -- and "it parses" has been
//! mistaken for "it compiles" here more than once (a trailing comma, `set` on
//! `xlim` writing `()`, `xscale` getting `[]`).
//!
//! So this is the same idea one level up. Gestures, not intents; every example in
//! the gallery, not one fixture; and after each one the figure is recompiled and
//! the diagnostics checked. Two properties, each of which has been violated by
//! real code:
//!
//! 1. **Every intermediate state compiles.** Not just the last one -- a gesture
//!    that writes a value typst rejects leaves the user staring at an error with
//!    no obvious cause.
//! 2. **Undoing the whole walk restores the source byte-for-byte.** Comments,
//!    whitespace and trailing commas included.
//!
//! The gestures are drawn from what each scene actually offers, so a colormesh
//! gets panned and resized but never point-dragged, and a figure whose axes are
//! not numeric is not panned in data space at all. That is the same dispatch the
//! canvas does, which is the point: a walk that only ever produced legal gestures
//! for one shape would not be testing the dispatch.
//!
//! The committed walk is small enough for the gate -- 3 seeds of 6 gestures per
//! example, about 200 in twenty seconds. Widening it is two numbers; 11 seeds of
//! 10 was run at 1,210 gestures and stayed green after the two defects below.
//!
//! It found both of them on the day it was written, which is the argument for it:
//!
//! - **A resized figure could exceed the GPU's maximum texture side**, which is a
//!   panic inside `egui` rather than an error anyone can catch. Six resizes and a
//!   gallery example was 2196 px across. Fixed by capping the raster scale to
//!   what the machine's textures hold, recomputed from every render.
//! - **`SetLimits` could write a negative bound onto a log axis.** The canvas
//!   pans through `AxisMap::shifted` and cannot leave a log axis, so the guard
//!   was in the caller -- but a gesture is the editor's *public* vocabulary and
//!   reaches it from three shells. The check moved to where the document is
//!   written.
//!
//! Note what that means about the walk's own shape: it deliberately does **not**
//! route its pan through `shifted`. Teaching the generator to only produce
//! already-safe inputs would have hidden the second defect entirely.

use lilook_core::render::Severity;
use lilook_editor::CanvasEvent;
use lilook_web::{WebApp, EXAMPLES};

/// xorshift64: reproducible, and a failing seed is a failing test case someone
/// can rerun.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    /// A fraction in `0.15..0.85`, so a dragged point lands inside the figure
    /// rather than at an edge where the axis limits would have to grow.
    fn frac(&mut self) -> f64 {
        0.15 + 0.7 * (self.next() % 1000) as f64 / 1000.0
    }
}

fn run(app: &mut WebApp, n: usize) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    for _ in 0..n {
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.frame(ui));
    }
}

fn app() -> Option<WebApp> {
    let dir = typst_assets_fonts()?;
    let fonts: Vec<Vec<u8>> = lilook_web::WEB_FONTS
        .iter()
        .map(|name| std::fs::read(dir.join(name)).expect(name))
        .collect();
    Some(WebApp::with_fonts(fonts))
}

fn typst_assets_fonts() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let at = text.find("typst-assets")?;
    let key = "\"manifest_path\":\"";
    let from = text[at..].find(key)? + at + key.len();
    let to = from + text[from..].find('"')?;
    let manifest = std::path::Path::new(&text[from..to]);
    Some(manifest.parent()?.join("files").join("fonts"))
}

fn errors(app: &WebApp) -> Vec<String> {
    app.editor()
        .diagnostics()
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

/// One gesture, chosen from what this document's scenes can actually take.
///
/// Returns the events a canvas would emit, `Begin`/`Commit` included, because a
/// gesture is one undo step and the transaction is part of what is under test.
fn gesture(app: &WebApp, rng: &mut Rng) -> Vec<CanvasEvent> {
    let scenes = app.editor().scenes();
    if scenes.is_empty() {
        return vec![];
    }
    let scene = &scenes[rng.pick(scenes.len())];

    // What this scene offers. A pan needs numeric axes; a point drag needs a
    // series whose points are literals in the source, which is what the editor
    // itself checks before drawing a handle.
    let mut choices: Vec<u8> = vec![1]; // resize is always available
    if scene.numeric.0 && scene.numeric.1 {
        choices.push(0);
    }
    let draggable: Vec<(usize, usize)> = scene
        .series
        .iter()
        .filter(|g| {
            app.editor()
                .doc
                .calls()
                .iter()
                .find(|c| c.id == g.node)
                .is_some_and(|c| c.has_literal_points())
        })
        .flat_map(|g| (0..g.points.len()).map(|i| (g.node, i)))
        .collect();
    if !draggable.is_empty() {
        choices.push(2);
    }

    let mut out = vec![CanvasEvent::Begin];
    match choices[rng.pick(choices.len())] {
        // Pan: shift both axes by up to a fifth of their span, which is a drag
        // of a plausible size rather than a jump to nowhere.
        0 => {
            let t = &scene.transform;
            let (dx, dy) = (
                (t.x.max - t.x.min) * (rng.frac() - 0.5) * 0.4,
                (t.y.max - t.y.min) * (rng.frac() - 0.5) * 0.4,
            );
            out.push(CanvasEvent::SetLimits {
                figure: scene.figure,
                x: (t.x.min + dx, t.x.max + dx),
                y: (t.y.min + dy, t.y.max + dy),
            });
        }
        // Resize by the frame. Bounded well away from zero: a figure 2 pt wide
        // is a rendering question, not an editing one.
        1 => out.push(CanvasEvent::SetSize {
            figure: scene.figure,
            width_pt: Some(80.0 + rng.frac() * 300.0),
            height_pt: Some(60.0 + rng.frac() * 200.0),
        }),
        // Drag a point to somewhere inside the axes.
        _ => {
            let (node, index) = draggable[rng.pick(draggable.len())];
            let t = &scene.transform;
            out.push(CanvasEvent::MovePoint {
                node,
                index,
                to: (
                    t.x.min + (t.x.max - t.x.min) * rng.frac(),
                    t.y.min + (t.y.max - t.y.min) * rng.frac(),
                ),
            });
        }
    }
    out.push(CanvasEvent::Commit);
    out
}

#[test]
fn random_gestures_over_every_example_compile_and_fully_undo() {
    let Some(mut app) = app() else { return };

    // Counted rather than assumed: a `gesture` that quietly stopped producing
    // point drags would leave this test passing while testing a third less.
    let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();

    for (i, (name, source)) in EXAMPLES.iter().enumerate() {
        for seed in 1..4u64 {
            app.load(i);
            run(&mut app, 3);
            if !errors(&app).is_empty() {
                // The example itself is broken; `every_example_compiles` owns
                // that failure, and reporting it twice helps nobody.
                continue;
            }
            assert_eq!(app.editor().text(), *source, "{name}: did not load clean");

            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut applied = 0usize;
            for step in 0..6 {
                let events = gesture(&app, &mut rng);
                for e in &events {
                    match e {
                        CanvasEvent::SetLimits { .. } => *kinds.entry("pan").or_default() += 1,
                        CanvasEvent::SetSize { .. } => *kinds.entry("resize").or_default() += 1,
                        CanvasEvent::MovePoint { .. } => *kinds.entry("drag").or_default() += 1,
                        _ => {}
                    }
                }
                if events.len() <= 2 {
                    continue;
                }
                app.editor_mut().handle_canvas(events);
                applied += 1;
                run(&mut app, 3);

                // Property 1: every intermediate state compiles.
                assert!(
                    errors(&app).is_empty(),
                    "{name} seed {seed} step {step}: {:?}\n--- source ---\n{}",
                    errors(&app),
                    app.editor().text()
                );
            }

            // Property 2: undoing the walk restores the source byte-for-byte.
            for _ in 0..applied {
                app.editor_mut().doc.undo();
            }
            assert_eq!(
                app.editor().text(),
                *source,
                "{name} seed {seed}: {applied} gestures did not fully undo"
            );
        }
    }

    for kind in ["pan", "resize", "drag"] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) > 0,
            "the walk never produced a {kind}: {kinds:?}"
        );
    }
}
