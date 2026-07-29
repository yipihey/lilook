//! The compile thread. These run without the lilaq package -- a plain typst
//! document exercises the scheduling, which is what is under test here.

use lilook_compile::CompileActor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn doc(n: usize) -> String {
    format!("#set page(width: 4cm, height: 2cm)\n#rect(width: {n}mm)")
}

#[test]
fn returns_a_frame_for_the_generation_it_was_asked_for() {
    let woke = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&woke);
    let mut actor = CompileActor::spawn(std::env::temp_dir(), move || {
        w.fetch_add(1, Ordering::Relaxed);
    });

    let g = actor.request(doc(10), 1.0);
    let frame = actor.wait().expect("a frame");
    assert_eq!(frame.generation, g);
    assert_eq!(frame.render.pages.len(), 1);
    assert!(woke.load(Ordering::Relaxed) >= 1, "the UI must be woken");
}

/// The scheduling property: a burst of requests must not queue. Only the last
/// one has to be compiled, and the UI must never be handed a stale frame as if
/// it were current.
#[test]
fn a_burst_of_requests_collapses_to_the_newest() {
    let mut actor = CompileActor::spawn(std::env::temp_dir(), || {});
    let mut last = 0;
    for n in 1..=40 {
        last = actor.request(doc(n), 1.0);
    }
    // Drain until the newest generation shows up; anything else that arrives is
    // a frame that started before the burst finished, never one queued behind.
    let mut seen = vec![];
    loop {
        let f = actor.wait().expect("a frame");
        seen.push(f.generation);
        if f.generation == last {
            break;
        }
    }
    assert!(
        seen.len() < 10,
        "requests queued instead of superseding: {seen:?}"
    );
    assert_eq!(*seen.last().unwrap(), last);
}

/// `take_latest` is the UI's entry point, and it has to hand back the newest
/// frame rather than the oldest queued one: the alternative is a canvas that
/// replays an entire drag after the pointer has stopped.
#[test]
fn take_latest_discards_superseded_frames() {
    use std::time::{Duration, Instant};
    let mut actor = CompileActor::spawn(std::env::temp_dir(), || {});

    // Space the requests out so each one actually compiles and several frames
    // pile up in the channel unread.
    let mut last = 0;
    for n in [5usize, 15, 25] {
        last = actor.request(doc(n), 1.0);
        std::thread::sleep(Duration::from_millis(120));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while actor.busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));

    let f = actor.take_latest().expect("frames are waiting");
    assert_eq!(f.generation, last, "an older frame was handed back");
    assert!(actor.take_latest().is_none(), "the channel must be drained");
}
