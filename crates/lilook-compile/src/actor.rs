//! A compile thread with latest-wins scheduling.
//!
//! Native only: it owns a `Backend` over the system loader and a thread, and a
//! browser build has neither. The `Backend` itself is portable.
//!
//! A drag emits an intent per frame, and every intent changes the buffer. If
//! requests queued, a two-second drag would leave the compiler a hundred
//! obsolete documents to work through after the pointer stopped. So there is
//! exactly one pending request at a time and a newer one replaces it: the
//! compiler always works on the most recent text, and the UI always shows
//! either the current figure or the last one that compiled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use lilook_core::scene::Scene;
use lilook_core::Document;

use crate::backend::{Backend, Hints, Render};

struct Job {
    generation: u64,
    source: String,
    pixel_per_pt: f32,
}

#[derive(Debug)]
pub struct Frame {
    pub generation: u64,
    pub render: Render,
    /// One per `lq.diagram`, recovered from the same compile that produced the
    /// pixels -- the probes provably do not change the rendering, so there is
    /// no second pass to keep them in step with.
    pub scenes: Vec<Scene>,
    /// Every file this compile read, so the shell can watch the ones the figure
    /// actually depends on rather than guess from the source text. Carried as
    /// paths relative to the root: resolving them to absolute paths needs the
    /// system loader, which is the shell's business and not portable.
    pub data_files: Vec<lilook_core::DataFile>,
}

struct Slot {
    pending: Option<Job>,
    /// Expressions to evaluate. A queue rather than latest-wins, because unlike
    /// a compile every query has a caller waiting for its particular answer.
    queries: Vec<String>,
    /// Figures to export, and in what. Queued like queries rather than
    /// latest-wins: each one has a caller who asked for that particular file.
    exports: Vec<(crate::export::Format, f32)>,
    /// Messages to locate the cause of, with the source to look in.
    blames: Vec<(String, String)>,
    quit: bool,
}

/// The bytes of one `CompileActor::export`.
#[derive(Debug)]
pub struct Exported {
    pub format: crate::export::Format,
    pub bytes: Result<Vec<u8>, String>,
}

/// The answer to one `CompileActor::query`.
#[derive(Debug)]
pub struct Answered {
    /// The expression that was asked, so a caller can tell whose answer this is.
    pub expr: String,
    pub answer: Option<lilook_core::data::Answer>,
    pub diagnostics: Vec<lilook_core::Diagnostic>,
}

pub struct CompileActor {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    out: Receiver<Frame>,
    answers: Receiver<Answered>,
    exports: Receiver<Exported>,
    blames: Receiver<Vec<lilook_core::Blame>>,
    /// True while a job is queued or running, so the UI can say "stale".
    busy: Arc<AtomicBool>,
    next_generation: u64,
    thread: Option<JoinHandle<()>>,
}

impl CompileActor {
    /// `wake` is called from the compile thread whenever a frame is ready; a
    /// GUI passes its repaint request here.
    pub fn spawn(
        root: impl AsRef<std::path::Path>,
        wake: impl Fn() + Send + 'static,
    ) -> CompileActor {
        let slot = Arc::new((
            Mutex::new(Slot {
                pending: None,
                queries: vec![],
                exports: vec![],
                blames: vec![],
                quit: false,
            }),
            Condvar::new(),
        ));
        let (tx, out) = channel();
        let (qtx, answers) = channel();
        let (etx, exports) = channel();
        let (btx, blames) = channel();
        let busy = Arc::new(AtomicBool::new(false));

        let thread = {
            let slot = Arc::clone(&slot);
            let busy = Arc::clone(&busy);
            let root = root.as_ref().to_path_buf();
            std::thread::Builder::new()
                .name("lilook-compile".into())
                .spawn(move || {
                    let mut backend = Backend::new(&root, "");
                    let mut hints = Hints::new();
                    loop {
                        let job = {
                            let (lock, cv) = &*slot;
                            let mut s = lock.lock().unwrap();
                            while s.pending.is_none()
                                && s.queries.is_empty()
                                && s.exports.is_empty()
                                && s.blames.is_empty()
                                && !s.quit
                            {
                                s = cv.wait(s).unwrap();
                            }
                            if s.quit {
                                return;
                            }
                            // Queries first: they are quick, and something in the
                            // UI is waiting on each one. A compile that is about
                            // to be superseded anyway must not hold them up.
                            let queries = std::mem::take(&mut s.queries);
                            let exports = std::mem::take(&mut s.exports);
                            let blames = std::mem::take(&mut s.blames);
                            drop(s);
                            for expr in queries {
                                let (answer, diagnostics) = backend.query(&expr);
                                if qtx
                                    .send(Answered {
                                        expr,
                                        answer,
                                        diagnostics,
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                wake();
                            }
                            // From the document already compiled, so what lands
                            // on disk is what is on screen rather than a second
                            // compile that might differ.
                            for (format, ppi) in exports {
                                let bytes = match backend.document() {
                                    Some(d) => crate::export::export(d, format, ppi),
                                    None => Err("nothing has compiled yet".into()),
                                };
                                if etx.send(Exported { format, bytes }).is_err() {
                                    return;
                                }
                                wake();
                            }
                            for (source, message) in blames {
                                let doc = Document::new(source);
                                let found = crate::blame::locate(&mut backend, &doc, &message);
                                if btx.send(found).is_err() {
                                    return;
                                }
                                wake();
                            }
                            let mut s = lock.lock().unwrap();
                            match s.pending.take() {
                                Some(job) => job,
                                None => continue,
                            }
                        };
                        // Reparsing here rather than sending a `Document` keeps
                        // the channel to plain data. Call-site ids are a pure
                        // function of the text, so they match the UI's exactly.
                        let doc = Document::new(job.source);
                        let (render, scenes) =
                            backend.render_scenes(&doc, job.pixel_per_pt, &mut hints);
                        // After the compile, so the list is that compile's.
                        let data_files = backend.dependencies();
                        let generation = job.generation;
                        // Only clear `busy` once nothing newer is waiting.
                        let idle = slot.0.lock().unwrap().pending.is_none();
                        if idle {
                            busy.store(false, Ordering::Release);
                        }
                        if tx
                            .send(Frame {
                                generation,
                                render,
                                scenes,
                                data_files,
                            })
                            .is_err()
                        {
                            return;
                        }
                        wake();
                    }
                })
                .expect("spawn compile thread")
        };

        CompileActor {
            slot,
            out,
            answers,
            exports,
            blames,
            busy,
            next_generation: 1,
            thread: Some(thread),
        }
    }

    /// Queue a compile, displacing any request that has not started yet.
    pub fn request(&mut self, source: impl Into<String>, pixel_per_pt: f32) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        self.busy.store(true, Ordering::Release);
        let (lock, cv) = &*self.slot;
        lock.lock().unwrap().pending = Some(Job {
            generation,
            source: source.into(),
            pixel_per_pt,
        });
        cv.notify_one();
        generation
    }

    /// Queue an expression for the compiler to evaluate.
    ///
    /// Not latest-wins, unlike `request`: something is waiting for each answer,
    /// so none is dropped.
    pub fn query(&mut self, expr: impl Into<String>) {
        let (lock, cv) = &*self.slot;
        lock.lock().unwrap().queries.push(expr.into());
        cv.notify_one();
    }

    /// Answers that have arrived since this was last called.
    pub fn take_answers(&self) -> Vec<Answered> {
        std::iter::from_fn(|| self.answers.try_recv().ok()).collect()
    }

    /// Ask for the figure as a file.
    pub fn export(&mut self, format: crate::export::Format, ppi: f32) {
        let (lock, cv) = &*self.slot;
        lock.lock().unwrap().exports.push((format, ppi));
        cv.notify_one();
    }

    /// Ask what causes `message` in `source`.
    pub fn blame(&mut self, source: impl Into<String>, message: impl Into<String>) {
        let (lock, cv) = &*self.slot;
        lock.lock()
            .unwrap()
            .blames
            .push((source.into(), message.into()));
        cv.notify_one();
    }

    /// Causes found since this was last called.
    pub fn take_blames(&self) -> Vec<Vec<lilook_core::Blame>> {
        std::iter::from_fn(|| self.blames.try_recv().ok()).collect()
    }

    /// Exports that have finished since this was last called.
    pub fn take_exports(&self) -> Vec<Exported> {
        std::iter::from_fn(|| self.exports.try_recv().ok()).collect()
    }

    /// The newest finished frame, discarding any that were superseded while the
    /// UI was not looking.
    pub fn take_latest(&self) -> Option<Frame> {
        let mut latest = None;
        while let Ok(f) = self.out.try_recv() {
            latest = Some(f);
        }
        latest
    }

    /// Blocks until a frame arrives. For tests and headless use.
    pub fn wait(&self) -> Option<Frame> {
        self.out.recv().ok()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
}

impl Drop for CompileActor {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.slot;
            let mut s = lock.lock().unwrap();
            s.quit = true;
            cv.notify_one();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
