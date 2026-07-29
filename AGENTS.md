# AGENTS.md

Read this before writing code. Everything here is either an invariant that must
not break or a mistake that was already made once during initial development.

---

## Invariants

**1. Never regenerate the source from a model.**
Every edit is a byte-range replacement on the existing text. The moment you
pretty-print a whole file from an object graph, you have reinvented the
intermediate format this project exists to avoid, and you will destroy the
user's comments, whitespace and hand-written code.

**2. The undo invariant is the acceptance test.**
Any sequence of intents, fully undone, must restore the buffer byte-for-byte.
`crates/lilook-core/tests/core.rs::random_intents_fully_undo` property-tests
this across 39 seeds. If you add an intent, it must pass under that test — add
it to the generator there.

**3. `History` is the only place coalescing policy lives.**
Intents are fine-grained by design: a slider drag emits one intent per frame and
a pan emits two. `History` decides what becomes a single undo step, and it
coalesces **per target**, not per transaction — one gesture routinely rewrites
several parameters, and a single key would collapse none of them. `Document`
passes the intent's key straight through and makes no decision of its own. Do
not add coarse "do-the-whole-thing" methods to the core to make a consumer's
life easier — the CLI, MCP and FFI all get atomicity by opening and committing a
transaction per call, and that is the pattern to follow.

**4. `lilook-ui` depends on `egui`, never on `eframe` and never on the compiler.**
It takes textures and `Scene`s and returns events. This is what lets the whole
interactive surface — inspector *and* canvas, including hit-testing and the drag
arithmetic — run headlessly with no display and no typesetter. It has already
paid for itself: the crate crossed egui 0.29 → 0.35 without a line changing
while `lilook-app` absorbed the entire `eframe::App` break. If you find yourself
wanting `eframe` in `lilook-ui`, the thing you want belongs in `lilook-app`.

**4b. The user's buffer is never probed.**
Probe injection builds a *derived* copy. If a marker ever reaches the buffer the
user saves, invariant 1 has been broken by another route.

**5. lilook knows nothing about any host application.**
No impress, implore, or host-specific crates in any `Cargo.toml`. The subtler
failure is vocabulary: if core types start carrying item IDs, journal hooks or
overlay concepts, you are coupled with no dependency edge to point at. Standing
test — could someone who has never heard of the host use lilook and not notice
anything missing?

**6. A value lilook writes must reparse.**
`Document::resolve` refuses any intent whose value is not a valid Typst
argument. Keep that check in front of every new value-carrying intent rather
than validating in a frontend: the GUI, the CLI, the MCP server and the FFI all
build values as text, and only one of them is easy to watch.

**7. Anything the user did not write is not editable.**
Call sites produced by a loop, closure or spread are indexed and selectable but
flagged `generated`, and must emit no edit events. There is a test asserting
this.

---

## Traps already hit

**Builtin constants and user bindings are syntactically identical.**
`stroke: red` and `stroke: accent` both parse as `SyntaxKind::Ident`. The
`BUILTIN_IDENTS` table in `doc.rs` is what distinguishes "show a colour swatch"
from "read-only, offer jump-to-definition". It is incomplete — extend it rather
than working around it.

**Glob imports are part of the public surface.**
`lq.linspace` reaches users through `#import "math.typ": *`. Skipping globs in
the schema extractor silently lost 13 functions and 31 parameters. Caught only
by a test asserting every indexed call site resolves against the schema. Keep
that test.

**Insert after the last argument, not before the closing paren.**
A call written with a trailing comma and its paren on its own line otherwise
yields `,\n, param: v)`, which does not parse. There is a reason
`InsertNamedArg` looks the way it does.

**The CLI must not panic on a closed pipe.**
`println!` panics when its reader goes away, so `lilook schema lq.plot | head`
used to crash. Use the `outln!` macro in the binaries.

**Probes must sit inside the current axis limits.**
Placed outside, the recovered *scale* stays exactly right but the layout origin
is displaced by thousands of points. `recover_transform` handles this with a
two-pass refinement; do not "simplify" it to one pass — single-pass error on a
300-unit axis was 2.16 units versus 0.007 after refinement.

**`lq.place` relative `0%` is the top-left of the data area.**
Page y grows downward, so the `r0` probe carries `ymax` and `r1` carries `ymin`.
Getting this backwards produces a plausible-looking but inverted transform.

**A gesture must anchor to the state at the press, not integrate the live scene.**
The scene arrives from the compile thread a frame or two behind the pointer.
Accumulating deltas against whatever scene is current feeds that lag back into
the gesture: a pan drifts, a point drag runs away. `Canvas` stores the limits
(or the point) as they were when the button went down and computes absolutely
from the total offset since.

**Probes must be argument-list injections into the diagram itself.**
The series probe re-evaluates the *source text* of a series' positional
arguments. Put it anywhere but the same argument list and that text is evaluated
in the wrong scope, so any figure whose data comes from a local binding silently
loses its points. Generated call sites are skipped for the same reason — their
argument text may mention a loop variable.

**"Editable" must not drift into "lossy".**
A control may reopen a value the core classified as opaque only when a parser
that writes *the same shape back* recognises it. The stroke editor round-trips
`1pt + red`, so it takes it; nothing recognises `red.darken(20%)`, so it keeps
the source editor. The moment a control writes back a shape it did not read,
it is silently rewriting the user's source.

**A set rule is an ordinary call site.**
`#show: lq.set-tick(..)` is in the call index like anything else, so it needs no
second edit path, no second undo story and no special inspector -- an elembic
element's fields are presented as a parameter list and the existing widgets do
the rest. What it *does* need is its scope shown, because a show rule applies to
the end of its enclosing block and users expect it to apply to a figure.

**Call-site ids are positions, not identities.**
They are indices into a document-order walk, so any edit that adds or removes a
call renumbers everything after it. Resolve an id immediately before the edit
that uses it, never across a sequence of structural edits -- pasting a series
inserts the call *first* and carries its bindings afterwards for exactly this
reason. Anchors survive edits; ids do not.

**Coalescing slots must be disjoint.**
Two intents can target nested bytes -- replacing a whole data slot and replacing
one element of the array inside it. Rewriting the outer one moves text the inner
slot believes it owns. An overlapping arrival materialises the group and starts
a fresh one; do not "simplify" that check away.

**Spans do not identify series.**
Do not reach for `typst-syntax` spans to work out what the user clicked.
lilaq constructs its drawing primitives inside its own functions, so spans
resolve into lilaq's source; exported SVG carries no span data at all. Series
identity is geometric — see `compile.rs`. Markers remain relevant only for
non-data furniture (ticks, spines, legend entries).

---

## Portability

`lilook-compile`'s `system` feature is the line between what a browser can have
and what it cannot: reading a project directory, downloading packages, scanning
system fonts, asking the clock for the date, and the compile thread. Everything
else -- the world, the probes, scene recovery, the rasteriser -- compiles for
`wasm32-unknown-unknown` and is exercised natively through `MemoryFiles` by
`crates/lilook-compile/tests/bundle.rs`. Keep it that way: `scripts/check.sh`
checks both targets, and a `std::fs` call in the wrong module fails it.

## Performance envelope

In process, typst 0.15.1, Apple Silicon. These are the numbers the design is
built around; the ratios transfer, the absolutes are machine-specific.

| points | cold | warm (style edit) | warm (data edit) | raster @2x |
| --- | --- | --- | --- | --- |
| 1k | 108 ms | 20 ms | 30 ms | 0.8 ms |
| 5k | 207 ms | 85 ms | 132 ms | 0.9 ms |
| 20k | 670 ms | 363 ms | 526 ms | 1.8 ms |

Injecting the probes into an already-compiled figure costs 3.3 ms at 1k points
and 6.3 ms at 5k, and provably does not change the rendered pixels
(`probes_do_not_perturb_the_render`), so the probe pass and the render pass are
the same compile. If that test ever fails, split them rather than trusting the
scene.

Consequences: decimate at roughly **2k** points in the data path before the
emitter — that is where a data-changing recompile stays under 60 ms — and never
shell out per edit. The subprocess floor was ~570 ms, dominated by startup.

---

## Testing expectations

- New intents get a case in the random-intent undo test, which asserts its own
  coverage: a generator that quietly stops producing an intent fails.
- New inspector controls get a headless `__run_test_ui` test. New *gestures* get
  a synthesised pointer test — `crates/lilook-ui/tests/canvas.rs` drives a real
  `egui::Context` with `RawInput` events, so a drag can be asserted end to end
  without a window.
- Anything that edits source gets an end-to-end check that the result still
  compiles — `crates/lilook-compile/tests/gestures.rs` for the GUI paths,
  `scripts/check.sh` for the CLI. The trailing-comma insertion bug passed the
  round-trip test and was caught only by recompiling the output.
- Visual claims get a screenshot: `cargo run -p lilook-app -- f.typ
  --screenshot out.png`. "It renders correctly" went unverified for a whole
  phase because capture was awkward; it is one flag now.

---

## Style

Comments explain *why*, particularly where a decision looks arbitrary but
encodes a measurement. Several existing comments cite specific numbers; keep
that habit, because it is what stops a future reader from "simplifying" a
two-pass solve or a curation table back into a bug.
