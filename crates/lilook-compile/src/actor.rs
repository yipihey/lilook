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
}

struct Slot {
    pending: Option<Job>,
    quit: bool,
}

pub struct CompileActor {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    out: Receiver<Frame>,
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
                quit: false,
            }),
            Condvar::new(),
        ));
        let (tx, out) = channel();
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
                            while s.pending.is_none() && !s.quit {
                                s = cv.wait(s).unwrap();
                            }
                            if s.quit {
                                return;
                            }
                            s.pending.take().unwrap()
                        };
                        // Reparsing here rather than sending a `Document` keeps
                        // the channel to plain data. Call-site ids are a pure
                        // function of the text, so they match the UI's exactly.
                        let doc = Document::new(job.source);
                        let (render, scenes) =
                            backend.render_scenes(&doc, job.pixel_per_pt, &mut hints);
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
