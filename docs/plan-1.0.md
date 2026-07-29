# lilook 1.0 — implementation plan for the egui frontend

Supersedes `docs/plan.md` §3 (the ordered backlog). §1 of that document — the
settled ADRs — still holds, with two amendments recorded below. What is *new*
here is that the largest open unknown, P5, was measured rather than estimated:
see §1 and the appended "Phase 5 measurements" section of `docs/findings.md`.

The goal of this plan is a specific product: **an egui application in which a
lilaq figure is rendered, clicked, dragged and edited directly, and every
gesture lands as a surgical edit in the user's `.typ` file.** Today
`lilook-app` renders no figure at all — it is a call-site list, an argument
inspector and a read-only source dump. That gap is the whole plan.

---

## 1. What measurement changed since `plan.md` was written

All numbers below are from this machine (Apple Silicon, rustc 1.97.1, typst
0.15.1, lilaq 0.6.0 — still the latest published version). The code that
produced them is kept at `spikes/inprocess-world/`, outside the workspace.

**The rustc blocker is gone, and the in-process backend works.** A `World`
built on `typst-kit` 0.15 (`FileStore` + `FileLoader`, embedded fonts,
`SystemPackages`) compiling a real lilaq figure, with the main buffer served
from memory and `FileStore::reset()` between edits:

| points | cold compile | warm recompile (style edit) | warm recompile (data edit) | render @2× |
| --- | --- | --- | --- | --- |
| 1k | 108 ms | 20 ms | 30 ms | 0.8 ms |
| 5k | 207 ms | 85 ms | 132 ms | 0.9 ms |
| 20k | 670 ms | 363 ms | 526 ms | 1.8 ms |
| 50k | 1.58 s | 866 ms | 1.20 s | 2.9 ms |

The 1.0 target of "sub-100 ms preview on a 5k-point figure" is met for style
edits at 5k (85 ms) and comfortably met at 1–2k for everything. The
`plan.md` decimation threshold of ~5k should be **~2k**, which is where a
data-changing edit stays under 60 ms.

**Probe precision improves by two orders of magnitude in-process.** The CLI
returns positions as rounded strings (`57.41pt`); the introspector returns
`57.41296287964004pt`. The two-pass refinement in `recover_transform` exists
solely because 0.01 pt output rounding dominated a 300-unit axis (2.16 units of
error in one pass). In-process that rounding does not exist, so the second pass
is unnecessary — **but keep the two-pass path for `CliCompiler`**, and keep the
comment explaining why, because the CLI is still the WASM-less fallback and the
MCP server's backend.

**Evaluated series data is recoverable, and nearly free.** Injecting
`#metadata((node: N, x: <x-expr>, y: <y-expr>))<lilook-series>` into an
already-compiled figure and reading it back through the introspector:

| points | probe compile + query | marshal to `Vec<(f64,f64)>` |
| --- | --- | --- |
| 1k | 3.3 ms | 0.002 ms |
| 5k | 6.3 ms | 0.007 ms |

comemo makes the second evaluation of `lq.linspace(...)` and `x.map(...)`
essentially free. This is the load-bearing new fact: **lilook can own the data
of any series, including series whose data is computed**, which is what makes
hit-testing and direct manipulation work on realistic documents rather than
only on literal arrays.

**Dependency bumps, measured on a scratch copy of the workspace:**

- `typst-syntax` 0.13 → 0.15: two call sites (`SyntaxNode::into_text` is gone;
  slice the source text by `node.range()` instead). All 16 tests pass.
- `egui` 0.29 → 0.35: `lilook-ui` compiles and all 5 headless tests pass
  **unchanged**, including `__run_test_ui`. Only `lilook-app` breaks:
  `eframe::App` is now `fn ui(&mut self, ui: &mut Ui, frame)` rather than
  `update(ctx, frame)`, and `SidePanel`/`TopBottomPanel` are unified into
  `egui::containers::Panel::left(id)` / `::bottom(id)`. ~30 lines of shell.

That `lilook-ui` survived a six-version egui bump untouched is the ADR-0011
split paying for itself; it is worth saying so in the ADR.

### Amendments to the settled ADRs

**ADR-0008 amended.** Series identity is still geometric, but the geometry now
comes from an injected series probe rather than from data lilook parsed out of
the source. The call-site id travels *in* the probe, so the mapping from a
clicked pixel to a byte range is exact rather than inferred.

**ADR-0013 (new) — the in-process backend lives in its own crate.** `typst`
pulls ~280 transitive crates. `lilook-core` stays at its three dependencies so
the CLI, the MCP server and the FFI keep their build times and so a
document-only consumer never compiles a typesetter. The `Compiler` trait stays
in core; `lilook-compile` implements it.

---

## 2. Target architecture

```
lilook-core      document, intents, history, schema, transform math   typst-syntax, serde
lilook-compile   World, compile actor, probe injection, scene, raster typst 0.15, typst-kit
lilook-ui        egui-only: inspector, canvas, viewport, gestures     egui
lilook-app       eframe shell: window, files, threads, textures       eframe
lilook-ffi       C ABI, unchanged
```

`lilook-ui` still never depends on `eframe`, and now also never depends on
`lilook-compile`: it receives a `Scene` and an already-uploaded texture id, and
returns `UiEvent`s. That is what keeps the canvas — including its hit-testing
and drag arithmetic — testable under `__run_test_ui` with no display and no
compiler.

### Data flow, one frame

```
  user gesture ──► UiEvent ──► Intent ──► Document (byte-range edit)
                                             │
                                             ├──► source text ──► CompileActor (own thread)
                                             │                         │
                                             │                    derived buffer
                                             │                  (preamble + probes)
                                             │                         │
                                             │                    typst::compile
                                             │                         │
                                             └──◄── Frame { pixmap, Scene, diagnostics }
```

The actor is latest-wins: a request supersedes any queued predecessor, so a
drag at 60 Hz never builds a backlog. The UI keeps the last good frame and
dims it while a newer one is in flight, so a compile error never blanks the
canvas.

### The Scene

One `Scene` per `lq.diagram` call site, produced by the probe pass:

```rust
struct Scene {
    figure: usize,                 // diagram call-site id
    page_rect: Rect,               // where the data area sits on the page, pt
    transform: Transform,          // data <-> page, from the 4 transform probes
    series: Vec<SeriesGeom>,       // { node: usize, points: Vec<(f64, f64)> }
    furniture: Vec<Marker>,        // ticks, legend, title, spines -> call site
    diagnostics: Vec<Diagnostic>,
}
```

`Transform`, `AxisMap` and `hit_test` already exist in `lilook-core` and are
pure arithmetic; they stay there and gain a `Viewport` sibling that composes
data → page pt → texture px → screen rect, so the chain lives in one tested
place instead of being open-coded in the canvas.

### Probe injection

Injection happens on a **derived buffer**, never on the user's text: the same
`AppliedEdit` machinery applied to a scratch clone. Two kinds:

1. **Transform probes** — four `lq.place` calls appended to the diagram's
   argument list, as today (`d0`, `d1` inside the current limits, `r0`/`r1` at
   0%/100%). The existing constraint stands: probes outside the limits displace
   the layout origin.
2. **Series probes** — for each non-`generated` series call inside that
   diagram, one extra argument to the *same diagram call*:
   `lq.place(0, 0, [#metadata((node: N, x: <x-src>, y: <y-src>))<lilook-series>])`,
   where `<x-src>` and `<y-src>` are the verbatim source text of the series'
   positional arguments.

Injecting into the diagram's own argument list is what makes this scope-correct
by construction: the argument expressions are evaluated in exactly the scope
they were written in, so a series whose data is a local `#let` or a closure
capture still resolves. Generated call sites are skipped, which they must be —
their argument text may reference a loop variable.

**Risk, with a test attached.** The findings note that placed items can
influence layout. `metadata` has no size, but `lq.place` may still enter
lilaq's autoscaling. Acceptance test: render the figure with and without the
injected probes and assert the pixmaps are byte-identical. If that fails, fall
back to two passes — a clean render pass for the texture and a probe pass for
the scene — which the measured 3–6 ms probe cost makes affordable.

---

## 3. The interaction model

This section is the actual product design; the milestones below just sequence
it.

**Selection precedence** on a click: point handle → series curve (nearest in
data space, tolerance in page points, as `hit_test` already does) → furniture
marker → diagram background. Selection is a call-site id, held as an `Anchor`
so it survives undo, and it drives the inspector — one selection model for the
canvas, the tree and the inspector.

**Gestures, and the edits they produce:**

| gesture | edit |
| --- | --- |
| drag on empty plot area | pan: `SetNamedArg` on `xlim` and `ylim` each frame |
| scroll / pinch | zoom: same two arguments, scaled about the cursor |
| drag a data point | `SetArrayElement` on the series' positional array |
| drag the data-area edge | `width` / `height` on the diagram |
| drag the legend | `position` on `lq.legend` |
| click a curve, then edit in the inspector | `SetNamedArg`, as today |
| <kbd>Delete</kbd> on a selected series | `RemoveNode` |
| <kbd>Alt</kbd>-drag a series | duplicate (P7 copy/paste) |
| double-click a tick label | edit the corresponding `lq.set-tick` field (P6) |

Pan and zoom are the highest-value gestures and the cheapest: they need only
`SetNamedArg`, plus `InsertNamedArg` when `xlim`/`ylim` are absent, both of
which exist.

**Dragging a point is where editability bites.** It is possible only when the
positional argument is a literal array (or a `#let` binding that resolves to
one — worth following, it is the common idiom). When the data is computed,
the handles render hollow with a tooltip, and the inspector offers an explicit
**Materialize to literal array** action that writes the evaluated data — which
lilook now holds, from the series probe — into the source. That is a large text
edit, but it is a deliberate user action with a visible result, not the
model-regeneration that invariant 1 forbids. It must be one undo step.

**A core defect this design exposes.** A pan sets two parameters per frame.
`History::record` coalesces only against `tx.edits.last()`, and `Document::apply`
compares against a single open `CoalesceKey`, so interleaved `xlim`/`ylim`
intents will not coalesce: a two-second pan appends ~120 edits to one
transaction. Fix in M5: store the key alongside each edit in the open
transaction and coalesce against the most recent edit *with the same key*. The
random-intent undo property test must keep passing, and gains a case that
interleaves two parameters on one node.

---

## 4. Milestones

Each has an exit criterion that is a test or a measurement, in the house style.
**M0–M10 are done, browser build included** — see the "Implementation" sections
of `docs/findings.md` for what the building of them changed, including several
things the plan had wrong. M9 grew a shell of its own: `lilook-web`, a gallery
of lilaq's documentation examples edited in the page.

**M0 — Foundation.** Bump `typst-syntax` to 0.15 (2 call sites), `egui`/`eframe`
to 0.35, rewrite the app shell for `App::ui` and `Panel::left`. Add CI: fmt,
clippy, `cargo test`, and the end-to-end "edited output still compiles under
typst" check that `AGENTS.md` requires. Adopt `egui_kittest` 0.35 alongside
`__run_test_ui`, which gives snapshot tests of the whole app surface.
*Exit: 16 tests green on bumped deps; CI runs them.*

**M1 — `lilook-compile`.** The `World` (validated by the spike), the compile
actor with latest-wins scheduling, and `PagedDocument` → `egui::ColorImage`.
Widen the `Compiler` trait beyond `query(&str, &str) -> String`, which is a
JSON-string shape that only suits the subprocess.
*Exit: a test compiles a lilaq figure in-process with no typst binary present,
and asserts <150 ms cold / <40 ms warm at 1k points.*

**M2 — Canvas.** Render the page into a texture, pan/zoom the *view* (distinct
from the data zoom in M5), fit-to-figure, device-pixel-ratio aware
`pixel_per_pt`. Diagnostics panel showing typst errors with their source spans.
*Exit: `cargo run -p lilook-app -- figure.typ` shows the figure; a kittest
snapshot covers it. This retires the "visual correctness unverified" line in
the README.*

**M3 — Scene recovery.** Probe injection on the derived buffer, transform
solve, series extraction, multi-figure documents.
*Exit: transform error < 0.01 data units against declared `xlim`/`ylim`; series
points recovered for a `linspace`/`map` series; probes-do-not-perturb-the-render
test passes (or the two-pass fallback is in place and documented).*

**M4 — Selection.** Click-to-select across canvas, tree and inspector; hover
readout of data coordinates; selection survives undo via anchors.
*Exit: a headless test that computes the page position of a known data point,
synthesises a click there, and asserts the resulting selection is the right
call-site id.*

**M5 — Direct manipulation.** Pan, zoom, point drag, resize, delete. Core work:
per-key coalescing (§3), `SetArrayElement` and `SetPositionalArg` intents,
comma cleanup on `RemoveNode`, indentation-aware insertion.
*Exit: every gesture is exactly one undo step; the random-intent property test
covers the new intents and still restores byte-for-byte across its 39 seeds;
the result recompiles.*

**M6 — Inspector 1.0.** Parameter grouping, a real colour picker on the swatch,
a stroke editor (`paint` + `thickness` + `dash`), a mark picker, add/remove
argument, docs from the schema on hover, unset-to-default, and the materialize
action. The 32 `variant` parameters get a validating source editor rather than
a silent text box.
*Exit: every `widget` value in the schema maps to a control; a test enumerates
the schema and fails on any unmapped widget kind.*

**M7 — Set rules.** Blocked on the §5 decision. Schema work is already done —
99 fields across 17 elembic elements.

**M8 — Copy/paste and an add-series palette.** Clipboard payload is Typst
source plus a structured MIME; the work is free-variable capture analysis on
paste. Adding a series is the same machinery in reverse.

**M9 — WASM.** `FileLoader` over a bundled package snapshot — lilaq 0.6.0 and
its three dependencies are 210 KiB total (lilaq 101, elembic 70, zero 24,
tiptoe 15) — plus embedded fonts. eframe 0.35 supports web. That `FileLoader`
is exactly the workspace seam `plan.md` §5 promises implore.

**M10 — Polish.** External-edit reconciliation (the file changed under us),
recent files, preferences, packaging, and the first real user document.

M0–M5 is the product: a figure you can see, click and drag. M6–M10 is 1.0.

---

## 5. Decisions taken

`plan.md` §2.2 asked how `lq.set-*` show rules should be scoped. **Option 3**
was chosen and is implemented: per-call arguments wherever lilaq offers one,
set rules only from a separate document-level panel, and the scoped variant
(option 2) left as something the user writes in the source rather than
something an inspector does to their manuscript behind their back. The panel
names the scope of every rule it lists — "rest of file" or "enclosing block" —
because that is the part Typst's semantics make surprising.

Two smaller ones, both defaults I will take unless told otherwise:

- **Decimation threshold 2k, not 5k** (§1), applied in the data path before the
  emitter, with the full data retained for hit-testing.
- **The source pane becomes editable**, with typing applied as `ReplaceRange`
  intents. Direct text editing is not model regeneration, and a figure editor
  whose source pane is read-only is a strange object.

---

## 6. Testing

The existing expectations from `AGENTS.md` all still apply. What the frontend
adds:

- **Canvas logic is pure and tested without a display.** Viewport composition,
  hit precedence and gesture → intent mapping are functions over a `Scene`;
  `__run_test_ui` covers the widget layer and `egui_kittest` snapshots the app.
- **Pixel-level regression on the render path**, which is how the
  probes-don't-perturb-layout property is enforced.
- **The undo invariant extends to gestures**, not just intents: a synthesised
  drag must produce one transaction that restores the buffer byte-for-byte.
- **A performance test with the measured numbers as thresholds**, so a
  regression in the compile path fails CI rather than being noticed in the
  hand.

---

## 7. Risks

| risk | trigger | response |
| --- | --- | --- |
| probes perturb lilaq's autoscaling | M3 pixmap test fails | two-pass compile; 3–6 ms measured cost makes it affordable |
| series-probe argument text is not valid in the injection scope | any figure built by a helper function | already excluded via `generated`; widen that flag to cover calls inside `#let` function bodies |
| typst 0.16 lands mid-build | dependabot | typst upgrades are scheduled work, per the existing risk table; the workspace now pins one typst version across `typst-syntax` and `typst` |
| egui churn | next release | ADR-0011 measured: `lilook-ui` crossed six versions untouched. The shell absorbs it |
| WASM package bundling drifts from the pinned lilaq | schema regeneration | the CI job that fails on schema diffs should also fail on a bundle/schema version mismatch |
| 50k-point figures | first user with one | decimate at 2k; the full data stays in the `Scene` for hit-testing |
