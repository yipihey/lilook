# lilook.rs — Phase 0 findings

Target: **lilaq 0.6.0** (MIT, 68 `.typ` files, ~15k lines, declared compiler `0.13.0`).
Environment: rustc 1.89.0, typst CLI 0.13.1, lilaq's own docs-site tooling.

---

## Gate 1 — Is the API machine-extractable? **PASS, better than assumed**

| metric | value |
| --- | --- |
| public exports in `lilaq.typ` | 42 (35 resolve to a parsed definition) |
| documented parameters | 378 |
| with machine-readable type union | **376 (99.5%)** |
| with prose description | **377 (99.7%)** |
| named (carry a default literal) | 325 (86%) |
| positional (data slots) | 53 |

The earlier worry about patchy doc-comment coverage does not materialise. Coverage is
effectively total.

**lilaq ships its own extractor.** The docs-site repo
(`lilaq-project.github.io/scripts/tidy.py`, 165 lines) is a standalone Python
implementation of the tidy doc-comment parser. It already emits, per parameter:
`name`, `default` (as source text), `description`, and `types` as a *list*. This is
the reference implementation for lilook's schema step — port it, don't rewrite it.

**There are two API surfaces, and both are structured:**

1. **Plot constructors** (`plot`, `scatter`, `bar`, `quiver`, …) — plain `#let`
   functions with tidy doc-comments. Types come from `/// -> a | b | c`.
2. **Model elements** (15 of them: `diagram`, `axis`, `tick`, `label`, `legend`,
   `grid`, `spine`, `title`, `errorbar`, …) — declared via elembic
   `e.element.declare` with **99 settable fields total**. Here the types are
   *expressions*, not doc strings:

   ```typst
   e.field("width", e.types.union(length, relative, auto), default: 6cm),
   e.field("bounds", e.types.union("strict", "relaxed", "data-area"), default: "strict"),
   ```

   That is strictly better input than prose. Note `bounds` — a string-literal union is
   a dropdown with no inference required.

Reconciling the documented signature against the real settable fields: only 6 of 15
elements differ at all, and every difference is structural (`children`, `pad`) or an
internal element (`lbox`, `violin-extremum`). No meaningful drift.

**Union widths** (the widget-mapping problem, quantified):

| alternatives | params |
| --- | --- |
| 1 | 132 |
| 2 | 119 |
| 3 | 61 |
| 4 | 31 |
| 5–8 | 33 |

~35% are single-type and map to a widget mechanically. The pathological tail (5+
alternatives) is 33 parameters — small enough to hand-curate, which is the two-tier
design already planned.

Most common atoms: `auto` (90), `float` (78), `none` (76), `int` (72), `array` (71),
`stroke` (46), `color` (42), `bool` (40), `length` (39). `auto`/`none` appearing in
~44% of typed params means the inspector needs a first-class "unset / inherit"
control, not a nullable widget bolted on.

---

## Gate 3 — Lossless CST round-trip and surgical edit? **PASS**

Using `typst-syntax` 0.13 on a sample containing comments, irregular whitespace, a
`#let` binding used as an argument, and a spread/loop-generated series:

- `parse(src).into_text() == src` → **true**
- Surgical replacement of one named argument's value:
  prefix byte-identical **true**, suffix byte-identical **true**, reparses clean
  **true**, comments preserved **true**, irregular spacing preserved **true**

Argument values classify cleanly into `literal` / `binding-ref` / `computed` /
`callable` / `structured` / `content`, which is the recognized-profile boundary.

**Gotcha worth designing around.** `stroke: red` and `stroke: accent` both parse as
`SyntaxKind::Ident`. A builtin constant and a user `#let` binding are syntactically
indistinguishable. lilook needs a table of builtin identifiers to decide between
"show a colour swatch and edit in place" and "read-only, jump to definition".

Loop-generated calls (`..range(3).map(i => lq.plot(...))`) *are* found by the CST
walk — they are visible and selectable, just not literal-editable. Confirms the
opaque-node design rather than contradicting it.

---

## Gate 2 — Does hit-testing reach the user's call site? **Partial — negative where it counts**

Could not complete: `typst` 0.15 requires rustc ≥ 1.92 and 1.89 was the newest
toolchain installable here, so the in-process `Frame` test is still open.

Two things were established anyway:

1. **SVG export carries no span information at all** — no `data-*` attributes, just
   `<g>`/`<path>`/`<use>`. Inverse search cannot be recovered from exported SVG. It
   must be done Rust-side against laid-out frames.
2. **Spans will not point at the user's call site.** lilaq builds its drawing
   primitives inside its own functions — `place(curve(..segments))` at
   `src/plot/plot.typ:81`. The span on a rendered curve resolves into lilaq's source,
   not into `lq.plot(...)` in the user's document.

**Marker emission is therefore load-bearing, not insurance.** Design the emitter
around stable per-series markers from the first commit.

---

## Bonus — performance envelope (not a planned gate)

Real lilaq 0.6.0 figures, typst CLI 0.13.1:

| case | 1k pts | 5k | 20k | 50k |
| --- | --- | --- | --- | --- |
| line plot (`mark: none`) | 630 ms | 1.0 s | 2.6 s | 8.1 s (1.2 MB SVG) |
| scatter (marks) | 690 ms | 1.3 s | 3.8 s (9.6 MB SVG) | — |

Cold compile including package download: 3.3 s. Warm CLI floor: ~570 ms, dominated by
process startup.

**Implications.** The interactive ceiling is roughly 5k points for lines and 1–2k for
marks. Decimation belongs in the data path *before* Typst sees anything, with a hard
threshold. And the ~570 ms CLI floor rules out shelling out per edit — the in-process
compile service with comemo memoisation is required, not merely preferable.

---

## Prior art spotted

`typst-edit` on crates.io: *"In-place editing for Typst source: parse-plane span
queries and an atomic, non-…"*. Not evaluated. Worth 30 minutes before writing the
edit layer.

---

## Changes to the plan

1. Port lilaq's `tidy.py` rather than writing a doc-comment parser.
2. Build **two** extractors. The elembic one is the easier of the two and yields
   better types; do it first.
3. Add a builtin-identifier table to the core — required for correct editability
   classification.
4. Treat marker emission as a Phase 1 requirement, not a fallback.
5. Put a decimation threshold (~5k) in the data path before the emitter.
6. Gate 2 stays open. Needs rustc ≥ 1.92 plus a `World` impl with package
   resolution and font loading — a half-day, not a spike.

---

# Addendum — Gate 2 resolved, by a different route

The span question turns out to be the wrong question. **Series identity does not need
spans at all.**

## The probe technique

Inject a marker at a *known data coordinate* using lilaq's own `lq.place`, and have it
record where it landed:

```typst
#let probe(fig, x, y) = lq.place(x, y,
  context [#metadata((fig: fig, data: (x, y), pos: here().position()))<lilook-probe>])
```

`typst query doc.typ "<lilook-probe>" --field value` then returns each probe's data
coordinate paired with its page position. Two probes per axis give the exact
data↔page transform, after which hit-testing is arithmetic in data space — against
data lilook already owns.

## Measured

**Linear axes** — three probes on a 6cm×4cm diagram:

| data x | page x | data y | page y |
| --- | --- | --- | --- |
| 0 | 42.23pt | −1 | 112.32pt |
| 5 | 118.16pt | 0 | 61.65pt |
| 10 | 194.08pt | 1 | 10.98pt |

Slope 15.185 pt/unit; the interpolated midpoint predicts 118.155pt against 118.16pt
measured. Exact to output precision.

**Log axes** — `yscale: "log"`, decades 1→1000: 112.31, 78.57, 44.82, 11.07pt.
Spacing 33.74 / 33.75 / 33.75. Affine in log space, as expected. lilook already owns
`yscale`, so it knows which space to invert in.

**Multiple figures in one manuscript** — a tag field in the metadata disambiguates
cleanly; probes from two diagrams in the same document come back correctly separated,
including when the two use different scales and sizes.

**Overhead** — 794 ms vs 777 ms on a 200-point two-figure manuscript. ~2%, i.e. free.

**One constraint.** Probes must sit *inside* the current axis limits. Placed far
outside, the scale stays exactly right (15.19 vs 15.18 pt/unit, within output
rounding) but the layout origin is displaced by thousands of points. Derive probe
coordinates from the limits or the data — both of which lilook holds.

## What this changes

1. **The marker-emission requirement softens.** For *series*, geometry beats identity:
   nearest-point-in-data-space is what a plotting GUI wants anyway, and it gives
   correct tolerance behaviour at any zoom for free. Markers are still needed for
   non-data furniture — which tick, which spine, which legend entry — so the emitter
   design does not go away, it just stops being the critical path.
2. **Phase 1 de-risks considerably.** This works through the CLI today. It needs no
   `Frame` access, no `World` impl, and no `typst` crate — so it works unchanged in
   the WASM build.
3. **Hit-testing moves into data space**, which is strictly better: snapping,
   tolerance in data units, and nearest-series selection all become trivial.

## Still open

- In-process equivalent (introspection through the compiler rather than a `typst
  query` subprocess) is untested.
- Only lilaq 0.6.0, linear and log. `symlog` and datetime scales untested.
- Non-data element identity still needs the marker or span route.

---

# Phase 1 — core crate, first cut

`lilook-core` builds and its test suite is green. No dependency on impress,
implore, egui or Swift; three dependencies total (`typst-syntax`, `serde`,
`serde_json`).

```
lilook/
  tools/extract_schema.py          schema generator (build step)
  assets/lilaq-0.6.0.schema.json   generated, pinned
  crates/lilook-core/
    src/doc.rs        Document: source, CST, call-site index, intent resolution
    src/edit.rs       AppliedEdit, Transaction, History, Anchor
    src/intent.rs     the intent vocabulary
    src/schema.rs     schema types, shared by inspector / CLI / MCP
    src/bin/lilook.rs CLI: inspect, set, add, schema
    tests/core.rs     10 tests
```

## What is implemented

- **Document model.** Typst source is the only state. Every change is a
  byte-range replacement; nothing is regenerated from a model.
- **Fine-grained intents** (`SetNamedArg`, `InsertNamedArg`, `RemoveNode`,
  `ReplaceRange`) with a **transaction layer** above them. Fifty slider events
  coalesce into one undo entry when they share a `CoalesceKey`. The CLI opens
  and commits per command, so a coarse consumer gets atomicity without the core
  exposing a coarse API — which was the ordering trap identified earlier.
- **History as inverse text edits**, so undo composes with edits made anywhere
  else in the buffer.
- **Anchors** that transform through edits with left/right bias, so GUI
  selection survives undo. Spans do not survive; anchors do.
- **Editability classification** with the builtin-identifier table Phase 0
  showed was necessary — `red` resolves to `Builtin`, `accent` to `Binding`.
- **Generated-call detection**: calls under a closure, spread or for-loop are
  indexed and selectable but flagged `generated`, i.e. visible-not-editable.

## Schema generator

Both extraction paths implemented, plus a curation layer. Curation lives in the
generator, separate from the emitted JSON, so regeneration never clobbers it.

| stage | `variant` (needs generic editor) |
| --- | --- |
| mechanical type→widget mapping | 171 / 378 (45%) |
| + type-family coalescing | 112 (30%) |
| + 15 curated union signatures | **32 / 409 (7.8%)** |

Current surface: 48 functions, 17 elements, 409 parameters, 99.5% typed.

## What the tests caught

Three real defects, all found by end-to-end checks rather than unit tests:

1. **Glob imports were missing from the public surface.** `lq.linspace` reaches
   users via `#import "math.typ": *`, which the extractor skipped — 13 functions
   and 31 parameters absent from the schema. Caught by a test asserting that
   every indexed call site resolves against the schema.
2. **Argument insertion produced invalid syntax.** Inserting before the closing
   paren gives `,\n, param: v)` when the call already has a trailing comma. Now
   inserts after the last existing argument.
3. **The CLI panicked on a closed pipe**, so `lilook schema lq.plot | head`
   crashed. An agent-facing CLI cannot panic when its reader goes away.

## Verified end to end

`lilook set` / `lilook add` against a real lilaq figure, output recompiled with
typst 0.13.1: clean.

## Known gaps

- Insertion is not indentation-aware; new arguments land on the last argument's
  line. Valid, ugly.
- `RemoveNode` leaves the separating comma behind.
- Full reparse per edit. Correct, and fast at figure scale; the incremental
  reparser is an optimisation for later.
- No compile service yet — the probe/transform work from Phase 0 is not wired
  into the core.
- Elembic element fields are extracted into the schema but the document model
  does not yet handle `lq.set-*` show rules, which is how those are actually
  configured.

## Next

1. Compile service: in-process or CLI-shelled, plus the probe injection that
   yields the data↔page transform.
2. Hit-testing API in data space on top of it.
3. Copy/paste with Typst source as the clipboard payload, including free-variable
   capture analysis on paste.
4. Set-rule editing for the 17 elembic elements.

---

# Phases 2–4

Workspace: four Rust crates, a Swift package, a Python binding, 2,823 lines.
16 tests green (10 core, 1 compile-service against the real typst CLI, 5 UI).

```
lilook/
  crates/lilook-core   document, intents, history, compile service   no UI deps
  crates/lilook-ffi    C ABI (cdylib + staticlib) + hand-kept header
  crates/lilook-ui     egui-only inspector, headless-testable
  crates/lilook-app    eframe shell -- window, shortcuts, wiring
  bindings/python      ctypes over the C ABI
  swift/               SwiftUI package over the same C ABI
```

## Phase 1 completion — compile service

The probe technique from the Gate 2 addendum is implemented and tested against
the real typst CLI. Two passes: unit-separated probes for an approximate range,
then probes at 10%/90% of it for a well-conditioned solve.

Measured against a figure with declared `xlim: (-3, 7)`, `ylim: (100, 400)`:

| | x error | y error |
| --- | --- | --- |
| one pass | 0.0006 | 2.16 |
| two passes | 0.0003 | 0.007 |

Single-pass error scales with how small the probe separation is relative to the
data range — the y axis spans 300 units, so unit probes sat 0.38 pt apart and
the 0.01 pt output precision dominated. The refinement pass removes it.

`hit_test` then works in data space with tolerance in page points, so selection
behaves identically at any zoom.

## Phase 2 — CLI, MCP, bindings

- **CLI**: `inspect`, `set`, `add`, `schema`.
- **MCP server**: newline-delimited JSON-RPC over stdio. `initialize`,
  `tools/list`, `tools/call`. Tool descriptions are generated from the schema,
  so the agent and the inspector share one vocabulary. Verified end to end:
  inspect, set, a deliberate bad parameter returning a structured error, and the
  result recompiled clean by typst.
- **C ABI + Python**: intents cross as JSON, so the ABI does not change when the
  vocabulary grows. Verified that a three-step drag through Python coalesces
  into **one** undo step and that undoing restores the source byte-for-byte —
  the transaction contract survives the FFI boundary.

Both wrappers open and commit one transaction per call, which is the coarse
consumer the fine-grained core was designed to accommodate.

## Phase 3 — egui

Split deliberately:

- `lilook-ui` depends on **`egui` only, never `eframe`**, so the whole inspector
  runs headlessly under `egui::__run_test_ui` with no display and no extra
  dependencies. This is the concrete form of "easy for the agent to inspect and
  test."
- `lilook-app` is the eframe shell: window, undo/redo/save shortcuts, and the
  single place `UiEvent`s become transactions.

The inspector is schema-driven: controls come from the generated widget field,
sentinels (`auto`/`none`) are first-class buttons rather than a nullable hack,
bound identifiers are read-only with a jump-to-definition affordance, and
generated call sites render read-only and are asserted to emit no edit events.

`lilook-app` builds and runs under Xvfb. I could not capture a screenshot —
the capture kept hanging the shell — so "it renders correctly" is **not**
verified, only that it builds, starts and stays up.

## Phase 4 — Swift

Written, **not compiled**: there is no Swift toolchain in this container, so
everything below is unverified beyond review.

- `LilookDocument` wraps the C ABI with correct ownership — every returned
  string is copied out and released, the handle is freed in `deinit`.
- Call sites decode straight from the FFI's JSON into `Decodable` structs.
- `FigureView` is viewer-first: a call-site list, a narrow editing surface over
  arguments that carry a real widget, everything else read-only. This follows
  the earlier conclusion that exposing lilaq's full parameter surface on a phone
  is a poor experience regardless of toolkit.
- `LilookTests` mirrors the Rust and Python suites, including the
  drag-is-one-undo-step assertion.

For iOS the cdylib becomes an XCFramework built for `aarch64-apple-ios` and the
simulator triple; that build is not set up.

## Honest status

**Verified here:** schema extraction, CST round-trip and surgical edits, the
undo invariant under random intent sequences, transaction coalescing (in Rust,
through the CLI, through MCP, through Python FFI), transform recovery and
hit-testing against the real compiler, headless inspector rendering, and that
edited output recompiles.

**Not verified:** anything Swift, the visual correctness of the egui app, the
in-process compile backend (still shelling out at ~570 ms per invocation),
elembic `lq.set-*` show-rule editing, copy/paste, and WASM.

## Next, in order

1. In-process compile backend with comemo — required before drag-rate preview.
2. `lq.set-*` show-rule editing for the 17 elembic elements. The schema already
   carries their 99 fields; the document model does not yet touch set rules.
3. Copy/paste with Typst source as the clipboard payload, including
   free-variable capture analysis on paste.
4. Indentation-aware insertion, and comma cleanup on `RemoveNode`.
5. WASM target for `lilook-ui` — no blockers known, untested.
6. Swift: build the XCFramework and actually run `LilookTests`.

---

# Phase 5 measurements — the in-process backend, resolved

Environment: Apple Silicon, rustc **1.97.1**, typst CLI **0.15.1**, lilaq
**0.6.0** (still the latest published version — 0.6.1 and 0.7.0 do not exist).
The rustc ≥ 1.92 blocker that stopped Gate 2 is gone. These numbers come from a
throwaway spike, not from committed code; the plan they support is
`docs/plan-1.0.md`.

## Gate 5 — Does an in-process `World` reach drag rate? **PASS**

A `World` over `typst-kit` 0.15 (`FileStore` + a `FileLoader` that serves the
main buffer from memory and delegates packages to `SystemFiles`, embedded fonts
via `typst_kit::fonts::embedded`), calling `typst::compile::<PagedDocument>` and
`FileStore::reset()` between edits, on `lq.plot` of a sine over *n* points:

| points | cold | warm (style edit) | warm (data edit) | `typst_render::render` @2× |
| --- | --- | --- | --- | --- |
| 1k | 108 ms | 20 ms | 30 ms | 0.8 ms |
| 5k | 207 ms | 85 ms | 132 ms | 0.9 ms |
| 20k | 670 ms | 363 ms | 526 ms | 1.8 ms |
| 50k | 1.58 s | 866 ms | 1.20 s | 2.9 ms |

Against the ~570 ms subprocess floor this is a 28× improvement on the edit path
at 1k points, and it moves the interactive ceiling from "roughly 5k" to "5k
comfortably". Raster export is not on the critical path at all.

The same figures through the 0.15 CLI, for comparison: 180 ms / 310 ms /
770 ms / 1.68 s — i.e. this machine is roughly 4× the one Phase 0 was measured
on, so the *ratios* in the Phase 0 table are what transfers, not the absolutes.

**Decimation threshold should be ~2k, not ~5k.** 2k is where a data-changing
recompile stays under 60 ms, which is the budget that makes a drag feel
attached to the cursor rather than trailing it.

## The two-pass refinement is a CLI artifact

`typst query` prints positions as rounded strings — `57.41pt`. The in-process
introspector returns `57.41296287964004pt`. Since the whole reason for the
second refinement pass was that 0.01 pt output precision dominated a 300-unit
axis (2.16 units of error single-pass), the refinement is unnecessary
in-process. Keep it for `CliCompiler`, and keep the comment saying why —
deleting it while the CLI path still exists would reintroduce the bug the
comment is guarding.

## Evaluated series data is recoverable, and nearly free

The open question this settles: hit-testing "against data lilook already owns"
is only true for literal arrays, and real figures are written
`lq.plot(x, x.map(t => calc.sin(t)))`. Injecting
`#metadata((id: .., x: x, y: y))<lilook-series>` into an already-compiled
figure and reading it back through the introspector:

| points | probe compile + query | marshal to `Vec<f64>` |
| --- | --- | --- |
| 1k | 3.3 ms | 0.002 ms |
| 5k | 6.3 ms | 0.007 ms |

comemo makes the second evaluation of `lq.linspace(...)` and `.map(...)` all
but free, and `Value::Array` → `f64` is microseconds. So lilook can hold the
evaluated points of *every* series, however the user computed them. This is
what makes selection and direct manipulation work on real documents; it also
means the probe carries the call-site id, so pixel → byte range is exact rather
than inferred.

## Dependency bumps, measured on a scratch copy

- **`typst-syntax` 0.13 → 0.15**: `SyntaxNode::into_text` is gone; slicing the
  source by `node.range()` replaces it at the two call sites in `doc.rs`. All
  16 tests pass. Worth doing regardless, so the workspace does not link two
  parsers once `typst` 0.15 is a dependency.
- **`egui` 0.29 → 0.35**: `lilook-ui` compiles **unchanged** and all 5 headless
  tests pass, `__run_test_ui` included. Only `lilook-app` breaks: `eframe::App`
  is now `fn ui(&mut self, ui: &mut Ui, frame)` instead of `update(ctx, frame)`,
  and `SidePanel`/`TopBottomPanel` are unified into
  `egui::containers::Panel::left(id)` / `::bottom(id)`. About 30 lines.

A UI crate crossing six egui releases untouched while the shell absorbs the
whole break is ADR-0011 doing exactly what it was chosen for; that belongs in
the ADR as evidence rather than as an assertion.

`egui_kittest` 0.35 exists (AccessKit-driven, with snapshot support) and is the
natural extension of the headless-testing property to the whole app surface.

---

# Implementation — M0 to M5

What the plan in `docs/plan-1.0.md` proposed, and what happened when it was
built. 62 tests green; `scripts/check.sh` is the gate (fmt, clippy, tests, and
recompiling edited output).

## The two-pass probe design collapsed to one compile

The plan hedged: inject probes, then check whether they change the rendering,
and fall back to a separate clean pass if they do. They do not. Rendering the
same figure with and without the injected markers produces **byte-identical
pixmaps** (`probes_do_not_perturb_the_render`), so the scene and the picture
come from a single compile. `metadata` has no size, and the corner and series
probes use relative coordinates, which cannot widen the data range.

The second hedge did pay off. Probes at fixed data coordinates *can* land
outside the axis limits when the limits are automatic and far from the origin —
the case the earlier findings recorded as displacing the layout origin by
thousands of points. The backend detects it (`probes_were_in_range`), re-places
the probes from the limits the first pass recovered, and compiles again. After
that the recovered limits are cached per figure, so the steady state is one
compile: `auto_limits_far_from_the_origin_still_resolve` asserts both passes
agree.

## Coalescing had to become per target, not per transaction

Predicted from the design and confirmed by writing the pan: `History` coalesced
only against `tx.edits.last()`, so `xlim` and `ylim` interleaving one per frame
collapsed nothing. Forty frames of a two-parameter drag recorded 80 edits.

The fix is a slot per target inside the open transaction, materialised into a
valid edit chain on commit. Three things made it subtle enough to be worth
recording:

1. Coalescing rewrites an edit that is already recorded, so every later edit in
   the chain has to shift by the length delta.
2. A new slot's position has to be expressed in the coordinate space *before*
   any slot in the group changed length, or the chain replays wrong.
3. An intent with no key (an insertion, say) is a hard boundary: the slots
   before it must be materialised first, or the chain comes out unordered. The
   first frame of a pan on a figure with automatic limits *inserts* `xlim` and
   `ylim`, so this is the common path, not an edge case.

`a_two_parameter_drag_stays_two_edits` asserts the edit count rather than the
undo depth, because the bug was invisible to every test that only checked that
undo worked.

## Dependency bumps cost what was measured

`typst-syntax` 0.13 → 0.15: two call sites. `egui` 0.29 → 0.35: `lilook-ui`
unchanged, `lilook-app` rewritten for `App::ui` and `Panel::left`. That a UI
crate crossed six releases untouched while the shell absorbed the whole break is
ADR-0011 earning its keep, and it is now stated in the ADR as evidence.

## Visual correctness is verified, and cheaply

`--screenshot PATH` draws until the first compile lands, writes a PNG of the
window and exits. The previous phase left "it renders correctly" unverified
because capture kept hanging the shell; through eframe's own
`ViewportCommand::Screenshot` it is about forty lines and needs no display
server tricks. `--select N` sets the selection, so a scripted screenshot can
show a state that otherwise needs a pointer.

## What M5 did not do

Dragging the data-area edge to resize a diagram is not implemented. `width` and
`height` are already drag-editable in the inspector, and the data area's edge is
not the diagram's edge — the axis labels live outside it — so the gesture needs
a target that does not exist yet. Everything else in M5 is in: data pan, data
zoom, point drag on literal arrays, delete, plus the intents underneath
(`SetArrayElement`, `SetPositionalArg`, `RemoveNamedArg`), comma cleanup on
removal, and indentation-aware insertion.

---

# Implementation — M6 and M7

## Validation belongs in the core, not in each frontend

Every consumer builds argument values as text: a widget formats a number, an
agent pastes a string through MCP, a colour picker emits `rgb("#4c72b0")`. One
unbalanced paren leaves the user's manuscript broken in a way that is hard to
attribute to lilook. `Document::resolve` now refuses any value-carrying intent
whose value would not reparse, so the check happens once for the GUI, the CLI,
the MCP server and the FFI together.

The check parses the value *in an argument list* — `__lilook_check(<value>)` —
rather than as free-standing code. Two things fell out of that:

- `1 2` is two expressions as code and an error in an argument list, which is
  the reading lilook wants: a value that silently became a second argument
  would be a corruption the round-trip test could not see.
- `let x = 1` **is** a valid argument value in Typst — it evaluates to `none`.
  Verified against the compiler rather than assumed. The check is syntactic on
  purpose; lilaq's type rules stay lilaq's, and typst's diagnostic for
  `stroke: 3` already points at the right argument.

## The editability classification was too strict in two places

Both found by looking at a real figure in the running app rather than by
reasoning about the model.

**`rgb("#4c72b0")` was read-only.** It is a `FuncCall`, so it classified as
opaque — but a call to a *builtin constructor* is a value literal in everything
but syntax. `classify` now consults the same `BUILTIN_IDENTS` table that
distinguishes `red` from `accent`, so `rgb(..)` and `luma(..)` are editable
while `calc.sin(x)` and `lq.linspace(..)` stay opaque.

**`stroke: 1.5pt + red` was read-only**, and that is the commonest stroke in
every lilaq document. It parses as a binary operation, which the core is right
to call opaque — a general expression editor would have to rewrite a program.
The fix is in the frontend and is narrow: a control may reopen an opaque value
only when a parser that writes *the same shape back* recognises it. The stroke
editor round-trips `paint + thickness`, so it takes that value; nothing
recognises `my-style` or `red.darken(20%)`, so they keep the source editor and
the jump-to-definition. That rule is what keeps "editable" from drifting into
"lossy".

## Widget coverage

`widget_control` maps all 23 widget kinds the schema emits, and
`every_widget_kind_in_the_schema_has_a_control` fails if a regenerated schema
grows a new one. Eight kinds get a specialised control (colour picker, stroke
editor, mark and scale pickers, alignment pair, content, toggle, numeric drag
with its unit); six deliberately get the validating source editor, because
`array`, `data`, `dictionary`, `structured`, `variant` and `opaque` have no
small form. The source editor is the honest floor, not a gap: the value is
editable, and one that would not reparse is refused with the reason shown.

A second refinement pass adapts the control to the value: 32 `variant`
parameters admit several shapes, and `xlabel: [Time]` is content whatever the
type union says.

## Typing needed a transaction the user never opens

A drag brackets itself with a press and a release. Typing in a text box and
dragging inside a colour picker do not, so every keystroke was its own undo
step. The shell now opens a transaction on the first such edit and commits it
0.4 s after the last one, with a `request_repaint_after` so the commit does not
wait for the next input event. Combined with per-target coalescing, typing a
word is one undo step and one edit.

## Set rules: option 3, and a smaller change than expected

The decision was to keep set rules out of the figure inspector and give them a
document-level panel. Implementing it turned out to need almost no new
machinery, because **a set rule is an ordinary call site**: `#show:
lq.set-tick(..)` is already in the call index, so it is edited by the same
intents, coalesces the same way and undoes the same way. What was actually
needed:

- `Document::set_rules()`, which pairs each rule with the region it governs —
  to the end of the enclosing block, or to the end of the file. That scope is
  the part of Typst's semantics users get wrong, so the panel prints it.
- `Schema::element_as_function`, which presents an elembic element's fields as
  a parameter list. The inspector then renders a set rule with no special case
  at all. Element fields carry no `kind`, so `ParamSchema::kind` defaults to
  `named`.

Adding a rule inserts it after the lilaq import, where the alias is in scope,
and only at document level. The scoped variant stays something the user writes
in their own source: wrapping a figure in `#{ .. }` changes how their
manuscript reads, and doing that as a side effect of touching an inspector is
exactly what option 3 exists to avoid.

## Still not done

- **Decimation.** The plan carried a ~2k threshold forward from Phase 0. It is
  not implemented, and it is no longer obviously desirable: the derived-buffer
  machinery *could* substitute decimated arrays for the preview, but then the
  picture on screen would not be the picture that compiles, which is a strange
  property for a WYSIWYG editor. At the measured envelope (5k points, 85 ms per
  style edit) nothing forces the question yet. Decide it with a real figure
  that hurts, not in advance.
- **The resize gesture**, for the reason recorded in the M0–M5 section.
- **Copy/paste (M8), WASM (M9)**, and the Swift package, which has still never
  been compiled.

---

# Implementation — M8, M9, M10

## Copy/paste is a capture-analysis problem, as the plan said

The clipboard payload is the Typst source of the copied call, so a copy is
useful in any editor. The work is on paste. A copied series routinely reads
`#let xs = lq.linspace(..)` defined elsewhere, and pasting it where that binding
does not exist produces a document that will not compile.

`Document::free_identifiers(range)` answers "what does this fragment need from
outside itself", excluding what is bound *inside* it (closure parameters, a
local `#let`), field names (`plot` in `lq.plot`), argument names (`stroke:`) and
builtins. `binding_of(name)` finds the `#let` that defines it. On paste lilook
**carries the definitions** rather than inlining values: inlining would turn a
two-line figure into a wall of numbers and would drop the relationship the user
wrote. What it cannot resolve it names in the status line, so the failure is
legible before the compiler restates it.

**One ordering fact worth knowing before writing any structural edit:** call-site
ids are indices into a document-order walk, so inserting a binding above a
figure renumbers the figure. The paste inserts the call *first*, then the
bindings. This cost a failing test to discover and is now stated in AGENTS.md.

## The random-intent test earned its keep again

Adding `InsertPositionalArg` to the generator turned seed 6 red. The cause was
not the new intent: coalescing slots assume targets are **disjoint**, and
`SetPositionalArg { index: 0 }` and `SetArrayElement { arg: 0, element: 1 }` are
different targets over nested bytes. Rewriting the outer one moved text the
inner slot believed it owned, and undo replayed the chain into the wrong
positions. Overlapping arrivals now materialise the group and start a fresh one.

A property test that only asserted "undo works" would have found this too, but
only as a mangled buffer. What made it debuggable was the trace of intents per
seed; the fix has its own named regression test, because a seed is a poor
explanation of a bug.

## WASM: the compiler ports, the loader was the whole question

Measured, not assumed:

| target | result |
| --- | --- |
| `typst`, `typst-layout`, `typst-render`, `typst-syntax` on `wasm32-unknown-unknown` | compile clean |
| `lilook-core`, `lilook-ui` on wasm32 | compile clean, unchanged |
| `lilook-compile` on wasm32 without its new `system` feature | compiles clean |

So the browser build needs a bundle and a shell, not a different architecture.
What is feature-gated as `system` is exactly what a browser cannot have: reading
a project directory, downloading packages, scanning system fonts, and asking the
clock what day it is. The compile actor is native too -- it owns a thread.

`LilookWorld<L>` and `Backend<L>` are now generic over a `FileLoader`, with
`MemoryFiles` as the portable implementation. Three tests compile a real lilaq
figure and recover its scenes through a bundle built from the package cache,
with **no filesystem access during the compile** and only embedded fonts. lilaq
0.6.0 and its three dependencies come to ~180 files.

This is also the implore seam `docs/plan.md` §5 promises: the abstraction WASM
forces is the one a host application needs, built once rather than twice.

What remains for M9 is a web shell (eframe supports it), a committed or
generated package bundle, and actually running it in a browser -- none of it
blocked, none of it verified here.

## Polish that turned out to be structural

**The source pane is editable.** Typing produces a whole new buffer, but
replacing the whole document per keystroke would discard every anchor and make
each character a document-sized undo entry. `minimal_replacement(old, new)`
trims the common prefix and suffix (backing off to character boundaries) and
recovers the edit the user actually made -- almost always a few bytes. With the
idle transaction, typing a word is one undo step and the anchors survive.

**External edits.** lilook is not the only thing that touches a manuscript. The
file's mtime is checked once a second: unchanged buffer, reload silently;
unsaved changes, ask. A reload starts a fresh `Document`, because an undo
history recorded against text that no longer exists would write bytes from a
dead file back into the live one.

---

# The browser build

The goal, stated by the user: reproduce lilaq's documentation examples so each
one is editable in the page, by pointer or by text.

## Measure first: typst in wasm

Before building anything, a throwaway spike compiled the stacked area chart in a
browser and reported timings. **Warm recompiles: 3-16 ms.** That is not "usable
with care" -- it is faster than the desktop build's 20 ms on the same figure,
because these figures are small and comemo does the rest. It decided the design:
compile synchronously in the frame, no web worker, no scheduling.

Two things did have to be fixed to get there:

- **`std::time::Instant::now()` panics on `wasm32-unknown-unknown`** -- "time not
  implemented on this platform". The backend times every compile, so the first
  render died. `web-time` is a drop-in replacement and is already in the tree
  via eframe.
- **The wasm is 50 MB raw, 18 MB gzipped**, almost all of it typst's embedded
  fonts. Fine for a demo, worth trimming before it goes on a docs page.

## What the shells actually share

Building the second frontend is what showed how much of the first one was
shell-shaped. `lilook-app` was 1150 lines; the parts that were genuinely about
*windows* -- a file path, a compile thread, a screenshot flag, watching the disk
-- came to about 200. The rest moved into `lilook-editor`, which depends on
`egui` and `lilook-core` alone, never on a typesetter: the shell hands it a
compiled frame and asks it for the next source to compile.

That boundary is what makes the browser build small. `lilook-web` is a gallery,
a synchronous compile, and a bundle decoder.

The render data types (`Image`, `Page`, `Render`, `Diagnostic`) moved from
`lilook-compile` into `lilook-core` for the same reason: the editor has to name
what it draws without linking a compiler.

## Testing a browser app without a browser

`lilook-web`'s tests run natively. They drive `WebApp::frame` through a real
`egui::Context`, compile every gallery example against the bundled packages, and
assert on figures rather than pixels: every example compiles and yields a scene;
the stacked area chart is one diagram whose areas are generated (they come out
of a fold and a map, so lilook shows them and refuses to pretend they are
editable); an edit rescales the axis and undoes exactly.

What that leaves for a browser to prove is only that the page paints, which the
spike already showed.

## What the stacked area chart teaches

It is the honest hard case. The three areas are produced by
`.windows(2).map(..)` inside a helper function, so lilook flags them
`generated`: visible, selectable, not structurally editable. The *diagram* is
the user's own call, so its size, labels, limits and styling are all editable by
pointer -- and the areas are editable as text, in the same window, against the
same live figure. That is exactly why the answer to "GUI or text" is "both": the
GUI is honest about what it can safely rewrite, and the text pane covers the
rest without a mode switch.

## Where the browser bundle's weight actually is

Measured, because every guess about this was wrong.

| | raw | gzipped |
| --- | --- | --- |
| first build (ordinary release, fonts embedded) | 50.9 MB | 19.5 MB |
| `wasm-release` profile + `wasm-opt -Oz` | 27.0 MB | 10.5 MB |
| four fonts fetched separately instead of 9.6 MB embedded | +2.3 MB | +1.6 MB |
| **what a browser downloads now** | | **12.1 MB** |

The `wasm-release` profile is `opt-level = "s"`, thin LTO, `panic = "abort"`,
stripped. Fat LTO was tried and abandoned: it ran for over twenty minutes
without finishing and `wasm-opt -Oz` does the same work afterwards. `wasm-opt`
needs `-all`, because wasm-bindgen emits reference types and bulk memory that
binaryen rejects by default -- and the failure is a validation error, so a build
script that does not check it silently ships the unoptimised module.

**Fonts were 60% of the download**, not of the binary: 9.6 MB raw compresses to
6.3 MB, while code compresses four-fold. A lilaq figure asks for four faces --
Libertinus Serif regular, italic and bold, and NewCM Math -- which come to
1.6 MB gzipped, fetched alongside the module and cached separately.

Getting that list wrong fails *silently*: the figure lays out identically and
every label comes out blank, because a missing family is a warning typst prints
and then carries on from. `the_four_fetched_fonts_are_enough_to_draw_a_labelled_figure`
compiles with exactly those four and asserts nothing complains.

## The remaining weight, measured properly

**A correction.** The section that used to be here said the remaining 11 MB was
`icu_segmenter_data` and blamed `typst-layout`'s default features. That was
inferred from the size of the crate directory, and it was wrong. A two-line
probe -- one crate depending on `icu_segmenter`, built for wasm32 with and
without the dictionary features -- comes out at **0.39 MB either way**: the
linker already drops the 10.3 MB of dictionary data, because `typst-layout`
calls `LineSegmenter::new_lstm` and nothing reaches the dictionary path. Only
the 0.8 MB LSTM model is linked. Inferring binary contents from crate sizes is
not measurement.

What is actually there, from `twiggy` on an unstripped build (20 MB of code,
12 MB of data) together with `cargo tree`:

- **`wasmi`, a WebAssembly interpreter**, because typst supports `plugin()`. We
  ship a wasm interpreter inside a wasm binary for a feature no figure uses.
- **`hayro`, `hayro-interpret`, `hayro-postscript`, `hayro-ccitt`,
  `hayro-syntax`** -- a PDF *and* PostScript interpreter, plus fax decoding, for
  rendering PDFs embedded in a document. Pulled unconditionally by
  `typst-render`.
- **`vello_cpu` and `vello_common`, linked twice**: 0.0.8 for hayro and 0.0.9
  for egui. The top of the `twiggy` list is two hash-variants of the same blend
  functions. Both are pinned at `0.0.x`, where every release is
  semver-breaking, so cargo cannot unify them and neither can we.
- **`resvg`/`usvg`**, for SVGs embedded in a document.
- ICU LSTM data and `hypher`'s hyphenation dictionaries, which typst does use.

Three more levers were checked and rejected. Dropping eframe's `default_fonts`
changed the binary by 0.07 MB -- egui 0.35 no longer carries the emoji fonts it
used to. The ICU dictionary was already gone, as above. And `opt-level = "z"`
saves 1.5 MB raw over `"s"` (33.07 -> 31.56), which is perhaps 0.4 MB gzipped,
for the 20-30% runtime penalty `"z"` usually carries -- a bad trade for a demo
whose whole claim is that editing feels immediate.

So the remaining cuts are not available from this side of the dependency wall.
They need feature flags in `typst-render` (PDF and SVG image support) or
`typst-library` (plugins), or a fork of them. That is a well-formed upstream
request rather than a local task, and these numbers are what it should carry.

## Swift, compiled at last

The package had never been built: there was no toolchain in the container where
it was written, so "written, not compiled" stood for months. There is one here
now, and the verdict is mild -- it was wrong in exactly one way, uniformly.

`LilookDoc` is an opaque struct in the header, so Swift imports every
`LilookDoc *` as an `OpaquePointer`. The wrapper had been written as though it
imported as a typed pointer, and wrapped its handle in
`UnsafeMutablePointer(handle)` or `UnsafePointer(handle)` at every call site.
Every one of those is a type error; none of them is a design error. Removing the
conversions was the whole fix, and then all three tests passed first time --
including `testDragCoalescesIntoOneUndoStep`, which asserts the transaction
contract from the far side of the C ABI.

`scripts/swift.sh --ios` also builds `Lilook.xcframework` with both slices
(`ios-arm64`, `ios-arm64-simulator`) from the two Rust iOS targets. That was
listed as the blocker for iOS; it turned out to be about fifteen lines of
`xcodebuild`. The FFI static library is 20.6 MB per slice unstripped, which is
the document model and nothing else -- no typesetter, because `lilook-ffi`
depends on `lilook-core` alone.

What is still untested is the *SwiftUI* part: `FigureView` compiles, and nobody
has ever looked at it on a device. Compiling is not seeing.

## Reading data at compile time costs nothing measurable

The Veusz-style linked-dataset design rested on a performance claim, so it was
measured before anything was built. `crates/lilook-compile/examples/measure-data.rs`
reproduces it.

The claim was that `csv()` returns *strings*, so a linked dataset spends one
interpreted `float()` call per cell, while `cbor()` returns native floats with no
per-cell interpretation -- and that the gap would be wide enough to force a
transcoded sidecar for text formats too. The prediction was "fine at 1k, marginal
at 10k, unusable at 100k".

The prediction was wrong. Warm recompiles, best of five, release build:

| rows | csv+float | csv inlined | cbor | literal array |
| --- | --- | --- | --- | --- |
| 1k | 21.7 ms | 21.6 ms | 20.2 ms | 20.1 ms |
| 10k | 161.9 ms | 182.3 ms | 155.1 ms | 161.5 ms |
| 100k | 1.8 s | 2.2 s | 1.9 s | 2.0 s |

Every shape is within noise of every other at every size. **How the data reaches
the document does not measurably affect compile time**; what costs is lilaq
drawing N points, which it does identically whatever produced them. At 100k the
literal array carries 2.8 MB of source and still compiles in the same time as a
286-byte document that reads a file.

Three consequences:

- There is no row count above which lilook must prefer a sidecar to a live CSV
  link. Text formats can be linked directly at any size lilaq can plot, and the
  sidecar's justification is *capability* -- typst cannot read HDF5, npz or FITS
  at all -- plus slicing, so a 2 GB file need not be loaded and hashed to plot two
  columns of it. Not speed.
- The probe's second evaluation really is nearly free, which `probe.rs` already
  claimed at 1k and which now holds even for the worst case: an expression inlined
  into the series slot rather than bound to a name, where the conversion genuinely
  happens twice. That costs ~10% at 10k (182 vs 165 ms), not 2x.
- The wall is point count and nothing else. ~160 ms warm at 10k and ~1.9 s at 100k
  are the numbers decimation has to answer, and they are unrelated to linking.

One methodological note, because the first run of this measurement produced
garbage: comemo's cache is **process-global**, and `Backend::render` evicts lazily
(`comemo::evict(20)`). Six cases in one process therefore let each inherit the
previous ones' work, which made the fourth case look like the fastest way to read
a file -- csv-inlined-no-probe at 1.0 s against 2.4 s for a shape doing strictly
less work. A fresh `Backend` is not a fresh cache; `comemo::evict(0)` per case is.

## Linked datasets: what the compiler already knew

Veusz-style linked datasets turned out to need much less new machinery than
expected, because two things already existed and one measurement removed a third.

**typst's file store already tracks dependencies.** `FileStore::dependencies()`
returns every file a compile read, *including failed loads*, and `reset()` --
already called before every compile -- already marks them stale. So a figure that
reads a CSV already follows that file correctly; `tests/dependencies.rs` asserts
that changing the file changes `scenes[0].series[0].points` while `doc.text()` and
`history_depth()` are both unchanged. Nothing had to be built for that. What was
missing was only a *trigger* and a way to say so in the UI.

The list needs one filter to be usable: a compile of a lilaq figure reads 20-odd
package `.typ` files, so the panel shows project files that are not `.typ`. That
is `DataFile::is_data`, and the test asserts the unfiltered list really is
dominated by the package, so the filter is the point rather than a detail.

**The series probe already recovers linked data.** It works by re-evaluating a
slot's own source text, and it does not care whether that text is a literal array
or `run.map(r => float(r.t))`. A linked series appears in the tree with its point
count, hit-tests, and inspects with no new code at all.

**A query needs its own file id, and that is the whole reason it is not a probe.**
"What columns are in `run.csv`?" cannot go through `probe.rs`: a probe injects into
a diagram's argument list, and linking a file to a document with no diagram yet is
the first-run case. Compiling a throwaway `#metadata(csv("f.csv").at(0))` document
under its own `FileId` costs ~9 ms on top of a warm recompile (38.7 ms against
29.8), where reusing `main`'s slot would have rewritten the document's cached
source and put the next edit back in the 151 ms cold band. There is a test that
asserts which band it lands in.

Two things worth writing down for anyone extending this:

- **`Editor::ui` clears `requests` at the top of every frame**, so anything that
  wants to ask the shell for something from *outside* a frame has to hold it in
  editor state and emit it during `ui`, the way `dirty` feeds `want_compile`. A
  link started programmatically silently did nothing until `queued_query` existed.
- **A typst path cannot escape the project root.** Verified in
  `typst-syntax/src/path.rs`: `test("../world.txt", Err(PathError::Escapes))`.
  Since lilook roots at the `.typ` file's parent, `/scratch/run.h5` and
  `../data/run.csv` are inexpressible in any document plain `typst` can compile.
  This is the one Veusz behaviour that cannot be mirrored, so a drop from outside
  the root offers to copy the file in, and says that it did.

## The formats typst cannot read, and what that costs

Four of the five formats Veusz reads are not typst's: HDF5, npz, FITS, and Veusz's
descriptor ASCII (which typst *could* parse with `read` + `split` + `float`, at the
cost of a five-line interpreted parser living in the user's manuscript). Those four
decode in `lilook-data` and become a **CBOR sidecar** the document links.

CBOR rather than CSV for one reason worth recording: typst reads it back as native
`f64`, so the values that reach the figure are bit-exact -- `tests/dependencies.rs`
asserts `channel("x") == t` on the nose. A CSV sidecar would round-trip through
decimal text and through `float()` per cell.

**The whole crate is pure and portable, and that was a design choice with teeth:**
every decoder takes `&[u8]` rather than a path. So `lilook-data` builds for
`wasm32-unknown-unknown` with no feature gate, and npz, FITS and descriptor ASCII
work identically in the browser. `scripts/check.sh` checks each format's feature
on its own, so one decoder cannot come to depend on another being compiled in.

Three hand-rolled decoders instead of dependencies, and the reasoning was the same
each time -- the format is small, and the crate that reads it brings a world:

- **npy/npz**: `ndarray-npy` pulls `ndarray` plus `zip` to read a Python dict
  literal and a block of little-endian doubles. The reader is ~250 lines; the zip
  container is another ~150 using `flate2`, which was already in the tree with its
  pure-Rust `miniz_oxide` backend.
- **FITS**: there is no mature pure-Rust reader -- `fitsio` wraps cfitsio, which is
  C and cannot target wasm. The format is kind to this: 2880-byte blocks of
  80-column cards, then big-endian data the cards fully describe. `BSCALE`/`BZERO`
  had to be applied, not skipped: unsigned 16-bit data is conventionally stored
  signed with `BZERO = 32768`, so ignoring it halves every value and wraps the top
  half negative.
- **CBOR**: forty lines for a map of arrays of doubles, which is the one shape
  needed. The test asserts the encoding byte for byte and checks all five
  length-head widths, because a head that lies about its length is unreadable and
  the file is not human-inspectable.

One bug worth naming, because it is the kind that hides: in FITS, `NAXIS = 0` means
*no data*, and the product of no dimensions is 1. A primary header therefore
claimed one byte of data, which pushed every subsequent HDU one block out of
alignment -- so a `BINTABLE` after a primary header read as nothing at all.

## HDF5: verified, at the cost of vendoring

HDF5 is the exception to `lilook-data`'s rule. libhdf5 is C, its API is built
around a *path* rather than bytes, and there is no wasm build. So `hdf5.rs` takes a
path, is `cfg`'d off for wasm32, and sits behind an off-by-default feature.

The system HDF5 here is **2.1.1, and `hdf5-metno-sys` 0.10.1 rejects it outright**
("Invalid H5_VERSION"). Rather than ship a feature that only builds on machines
with a 1.x installed -- the "written, not compiled" trap the Swift package fell into
-- the feature uses `static`, which vendors and builds libhdf5 1.14. Three minutes
once, cached after, and the test writes a file *with libhdf5* and reads it back with
lilook's reader, so the walk, the type dispatch and the shape handling are checked
against a real file rather than against themselves.

In the browser the answer is different and better than "unsupported": the page
loads **h5wasm** on the `drop` event for an `.h5`, reads the datasets in
JavaScript, and hands back names and `Float64Array`s through `deliver_columns`.
Rust turns those into the same CBOR sidecar a native transcode produces, so
linking, rereading and unlocking cannot tell which route the numbers came by. It
is lazy by construction -- the `import()` is inside the drop handler -- so the cold
start is unchanged and only someone who opens an HDF5 file pays the ~1 MB.

**Not verified**: the h5wasm path has never run in a browser. The Rust side is
tested and the loader parses, but nobody has dropped an `.h5` on the page. That is
the same "compiling is not seeing" gap the Swift `FigureView` has, and it should be
said rather than assumed away.

## Refresh is not an edit, and that is the load-bearing property

The design's one non-obvious payoff. Because a linked file is read *by the
document*, rereading it is a recompile:
`a_changed_linked_file_is_reported_and_reread_without_touching_the_document`
asserts that after a reread the points changed while `doc.text()` **and**
`history_depth()` are byte-identical. Nothing to coalesce, nothing for undo to know
about, no interaction with the idle-transaction machinery at all.

Had the values been embedded instead, every refresh would have been a 20,000-number
transaction landing in whatever the user was typing. That is what made the sidecar
worth a generated file.

The watcher offers rather than acts, following `check_disk`'s precedent for the
manuscript: mtime *and* size, and a change must hold across two consecutive polls
before it counts, which keeps a file caught mid-write out of the figure. It still
cannot see a sub-second rewrite or a writer that preserves both (`rsync -t`), which
is a second reason not to act unasked. "Follow" is opt-in and never fires while a
gesture or an idle transaction is open.

## Provenance read from the source, not stored anywhere

The plan considered recording where a slot's data came from in a comment, and that
was the right thing to reject: it is state only lilook can validate, in the one
place the compiler cannot see.

The replacement turned out to need no storage at all. The slot says
`run.map(r => float(r.t))`, `run` is a binding, and the binding says
`csv("run.csv")` -- so provenance is *derived* every frame from the document, one
hop, in `file_behind`. It cannot go stale, cannot lie after a copy into another
project, and there is no lilook-only syntax anywhere in the `.typ`.

`read_path` deliberately refuses to answer for a computed path: `csv("runs/" + name)`
yields `None` rather than `"runs/"`. The file is still tracked -- the compiler
reports what it read -- but a confident wrong answer about where data came from is
worse than no answer. The first version of that function got this wrong, stopping at
the closing quote without checking the argument ended there.

Unlocking then had to remove the binding it orphaned. Without that the document
would keep reading a file nothing plots, so the Data panel would go on listing it
and the figure would look linked when it was not.

## `Editor::ui` clears `requests` every frame

Worth writing down for anyone extending the editor: `ui` starts with
`self.requests = Requests::default()`, so anything that wants to ask the shell for
something from *outside* a frame must hold it in editor state and emit it during
`ui`, the way `dirty` feeds `want_compile`. A link started programmatically
silently did nothing until `queued_query` existed -- and it was silent, not broken:
the flow just sat in its "asking" state forever.

## The deploy shipped a module no browser could load

The linked-datasets commit deployed green -- CI success, Pages success, HTTP 200,
11 MB over the wire -- and the site would not start:

```
CompileError: WebAssembly.Module doesn't parse at byte 153:
20th Type is non-Func, non-Struct, and non-Array 0
```

Two independent parsers agree, at the same byte. `wasm-opt` version 130: "Bad type
form 0 (at 0:153)". V8, via node: "unknown type form: 0 @+152". Parsing the type
section by hand shows why -- type #20 is `(i32, i32, structref) -> ()`, a **WasmGC
type**, and the byte after it is `0x00`, which is not a valid type form for anyone.

Where it entered, established by validating each stage:

| stage | size | V8 |
| --- | --- | --- |
| `wasm-bindgen` output | 32.6 MB | compiles |
| `wasm-opt -all -Oz`, binaryen 130 (local) | 28.4 MB | compiles |
| `wasm-opt -all -Oz`, Ubuntu's binaryen (CI) | 28.6 MB | **rejected** |

So rustc and wasm-bindgen were fine, and a *newer* binaryen is fine. The
apt-packaged one wrote a GC `structref` into the type section of a module it could
still read back itself -- and `wasm-opt` succeeded, exit code 0, no warning.

Three things had to be wrong at once for this to reach a user:

1. **`-all`.** It tells binaryen every proposal is permitted, which is what let it
   reach for a GC type at all. The features are now listed explicitly. Measured,
   that costs **10 KB gzipped out of 11 MB** -- 11.06 vs 11.07 -- so the entire
   class of failure goes away for nothing.
2. **`2>/dev/null` on the wasm-opt invocation.** Whatever binaryen might have said,
   nobody could have seen it. Removed.
3. **Nothing validated the artefact.** This is the real defect. `scripts/web.sh`
   now runs `new WebAssembly.Module(bytes)` under node -- V8, the same parser the
   browser uses, not a second opinion from the tool that just wrote the file -- on
   the optimised module, falling back to the unoptimised one if it fails, and then
   on whatever is about to be deployed, fatally. A build that cannot start is worse
   than a build that fails.

Binaryen is now pinned to a release tarball rather than taken from the runner
image, so its version is part of the build.

**The verification mistake was mine and worth naming.** I reported the deploy
healthy on the strength of an HTTP 200 and a transfer size. That checks the file is
*served*, not that it *runs* -- the same "compiling is not seeing" error recorded
two sections earlier about the Swift `FigureView`, made within the hour. The gate
that now exists is the one that should have existed before the claim: load the
module in an engine, in the build, every time.

## Two frames of state on a one-frame object

"Add argument" never worked. Picking a parameter from the combo did nothing: it
snapped straight back to "add argument…" and the "add" button never appeared.

The chosen parameter was a field on `Inspector`, commented "kept across frames".
It was not kept across anything -- the shell builds a fresh `Inspector` every
frame:

```rust
let mut insp = Inspector::new(f).with_context(context);
```

So the pick was stored and then dropped, microseconds later, between the click
that made it and the frame that would have acted on it. Any state a widget needs
across frames cannot live on an object that does not survive one.

**Why seven inspector tests missed it.** Every one of them holds the inspector in
a `RefCell` across `__run_test_ui` calls:

```rust
let insp = RefCell::new(Inspector::new(f));
egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
egui::__run_test_ui(|ui| insp.borrow_mut().ui(ui, call));
```

That is the opposite of what the app does, so the tests preserved exactly the
state the app threw away. A test harness that is more generous than production
cannot fail on a bug that production has -- and this one had been green through
the whole of M6, which is when the feature was written.

The fix puts the choice in egui's own per-widget store, keyed by
`add_argument_choice_id(call.id)`. Derived from the call site alone, deliberately:
not from the inspector, and not via `make_persistent_id`, which mixes in the
enclosing `Ui`'s hash -- and the panel the inspector draws into is not guaranteed
to hash the same across frames as the tree above it grows and shrinks.

The new test asserts the seam with a *fresh inspector each frame*, mirroring the
shell. It deliberately stops short of synthesising the click: egui's widget-rect
records are private, and a test that fakes its way to a position is a test that
will drift. The click path was verified in a browser instead -- pick `title`, the
"add" button appears, pressing it writes `title: none,` into the call, one undo
step, recompiled in 20 ms, and the combo clears so it cannot fire twice.

## The type is in the schema, so the user should never type the syntax

Adding `title` landed in a raw source box holding `none`, so a title had to be
written `"Flux"` or `[Flux]` by hand -- while `xlabel` needed neither, *purely*
because its current value happened to be `[day]`. `refine` decided the control
from the text alone, and no parser recognises `none`, so every typed control fell
through to the source editor. That is 140 of lilaq's 409 parameters: they accept
`none`/`auto` and most default to one.

The schema already knew better. It records `types` and, per parameter, the
`sentinels` it accepts -- 64 with `auto`, 50 with `none`, 26 with both. So the
control is now chosen by `control_of(param, editability, text)`, the one place that
sees both the schema and the value, and an unset value lets the *schema* decide:

- **words** (`content` in the types) → a plain text field, `none` as its hint. One
  `Control::Text` replaced `Control::Content` and covers strings too, so
  `title: "Flux"` reopens as text and is written back **as a string**: the shape the
  user wrote survives, which is the same-shape-back rule the inspector already had.
- **named variants** (`enum`, `mark`, `scale`) → the sentinels join the menu, so
  `auto`, `log` and `o` are entries to pick rather than spellings to remember.
- **everything else** → a new `Control::Unset`, which shows the sentinel and offers
  `set` to start from a value of the right type. Not a control seeded at zero: a
  slider showing `0` for `auto` would claim a value the document does not have.

### Three bugs, and only the compiler found them

The interesting part. A schema-wide test asserts all 409 parameters avoid the raw
editor at each of their sentinels, and it passed while the feature was still broken
in three places -- because it checked `check_expr`, and reparsing is not compiling:

- `set` on `xlim` wrote `()`. Parses; lilaq refuses it: "Limit arrays must contain
  exactly two items". **This one reached a live page before it was caught.**
- `xscale` got `[]`, because `takes_text` counted `str` as free text. In this schema
  a `str` *without* a `content` beside it is always a named variant -- `"log"`,
  `"o"` -- and an earlier scan of the schema had said exactly that, which I then
  did not act on. lilaq: "expected auto, string or dictionary, found content".
- `aspect-ratio` got `0`, the "neutral" number. "cannot divide by zero". Zero is
  neutral only for an offset; `1` is valid wherever a positive number is wanted.

So `seed` returns `Option` now: `None` where lilook knows the *shape* but not the
*contents*, and then the row shows a source editor with the shape as placeholder
text -- `(0, 10)` for a limit pair. Nothing invalid is ever written, and the user is
prompted rather than left to guess.

The test that catches this class lives in `lilook-compile`, where there is a
compiler: `seeded_arguments_compile` builds a document per seeded argument and
compiles it. This is the lesson `scripts/check.sh` was built on -- "the
trailing-comma insertion bug passed the round-trip test and was caught only by
recompiling the output" -- rediscovered on a new surface. A round trip is not the
gate. The compiler is.

## Panning a log axis: one symptom, three bugs

Reported as "panning a log-log plot gives *value must be strictly positive*". The
pan was the least of it.

**1. The recovered transform was linear, always.** `AxisMap` was `origin + data *
scale`, fitted from two probe points. On a log axis that is the *chord* between
them: every value in between maps to the wrong place -- hit-testing as much as
panning -- and extrapolating past them gave `y.min = -0.40` on a logarithmic axis,
before any gesture at all. That negative number is what a pan then wrote into
`ylim`.

So `AxisMap` now carries an `AxisScale`, and maps through it. Which one an axis is
is **recovered, not parsed**: a third marker, `dm`, goes at the *data* midpoint of
the probe pair. If the axis is linear in data, `dm` lands exactly halfway between
the other two on the page; if it does not, the axis bends, and the only bend lilaq
offers is logarithmic. That keeps the ADR-0008 discipline -- geometry is measured,
not inferred from source -- and it works whether the scale came from the call, a
set rule, or an `lq.axis` handed to `xaxis:`.

The test needs no probe of its own: on an axis spanning `min..max`, the middle of
the data area in page terms is the *arithmetic* mean of the limits if the axis is
linear and the *geometric* mean if it is logarithmic.

**2. The pan itself was additive.** Now the shift happens in the axis's own space,
so a log pan multiplies rather than steps. That is the guard the report asked for,
and it is worth being precise about why it is better than a clamp: a ratio can
approach zero without ever reaching it, so there is no boundary to special-case
and no drag that "sticks" at a limit. `nudged` does the same for dragging a point.
Linear axes are untouched, including their right to go negative.

**3. Two formatters were writing data through a geometry rounder.** Both were the
P1 defect class, in places P1 did not reach:

- `Editor`'s `SetLimits` used `num()`, six decimal places, so a limit of `3e-9`
  became `0`. New `gesture_num` keeps six *significant figures* instead: tidy for
  an ordinary pan (`10.1235`), lossless for a small one.
- `probe.rs`'s own `fmt` did the same to probe *coordinates*, and the comment
  justifying it -- that typst has no exponent literal in argument position -- was
  simply untrue (`lq.place(9e-17, ..)` compiles; checked). So a probe on a log
  axis became `lq.place(0, ..)` and lilaq refused the figure. More quietly, the
  number written into the document then disagreed with the `f64` `solve` fits
  against, so the transform was wrong by the rounding on *any* figure whose data
  lives near zero.

**And a fourth, found by pushing the test further than the report.** `Bounds::padded`
used an absolute `f64::EPSILON * 8.0` to decide an axis was degenerate, so any axis
narrower than ~1e-15 -- ordinary after panning deep into a log plot -- was replaced
by `(-1, 1)`. A probe then went to data −1 on a log axis. The test is relative now,
and padding keeps the sign of what it pads.

The lesson repeated from the seeded-arguments work: each of these produced a
document that *parsed*. Only compiling it found them, and only a drag far larger
than the figure found the fourth.

## A flaky test, and why the product order stays

CI went red on a commit that changed nothing but which files git tracks --
`actor.rs`, "the UI must be woken". Every prior commit had passed, which is what
identified it: a latent race the runner finally lost, not a regression.

The compile thread **sends the frame and then wakes**:

```rust
if tx.send(Frame { .. }).is_err() { return; }
wake();
```

`actor.wait()` returns the instant the send lands, so the test asserted the wake
counter while the wake was still a few instructions away.

The tempting fix is to swap the two lines, and it would be wrong: waking first
lets a UI repaint, find no frame waiting, and go back to sleep until something
else disturbs it. The ordering is load-bearing. So the *test* waits for the wake
with a deadline instead, and says why in the place someone would otherwise
"tidy" it.

Worth stating plainly because it cuts the other way from the last several
findings: those were cases where a passing test hid a real bug. This one is a
failing test hiding nothing -- and the answer is not to relax the assertion but
to fix the assumption it was making about ordering.

## Plot grids worked; a series in a `#let` did not

lilaq builds plot grids out of Typst's own `grid`, with `lq.layout` aligning the
axes across cells. Measured before assuming anything, the complex example from
lilaq's tutorial already worked: four diagrams recovered, each cell's frame
correct including the `colspan: 3` and both `rowspan: 2` cells, per-cell
hit-testing in each cell's own data space. Nesting inside `grid.cell(..)` costs
nothing, because `figures()` finds diagrams by name and the probes are injected
into the diagram's own argument list wherever that happens to sit.

One thing was missing, and it was not a plot type. The tutorial writes

```typst
#let mesh = lq.contour(..)
grid.cell(rowspan: 2, lq.diagram(.., mesh)),
grid.cell(rowspan: 2, lq.colorbar(mesh)),
```

-- by necessity, since the diagram and the colorbar share one plot object.
`figures()` found a diagram's series by *nesting*, so that diagram came out with
`series: []`: nothing to select, nothing to inspect, no data recovered.

`series_named_by` now resolves a diagram's positional arguments one hop through
their bindings. One hop only: `#let a = b` chains and series built inside
functions are not followed, because a wrong answer about which figure draws what
is worse than no answer. The contour's data then recovers through the probe with
no further change -- its `lq.linspace(0, 1)` arguments are self-contained, so
re-evaluating them in the diagram's argument list gives the same arrays.

### Two staleness checks earned their keep

Adding the example failed twice before it passed, both times with a missing-file
error naming the exact path:

- **komet was not in the bundle.** lilaq imports it lazily for colour maps, so no
  example had needed it until one used a contour. The package list in
  `build.rs`, `bundle.rs` and `fetch-packages.sh` is kept honest exactly this way.
- **komet computes its colour maps in a typst plugin**, and `build.rs`'s own
  extension filter was `["typ", "toml"]`, so `src/komet.wasm` was left out. Now
  `["typ", "toml", "wasm"]`. Bundle 752 KiB to 934 KiB.

That second one is a nice confirmation of something recorded earlier for a
different reason: `wasmi` is linked into the browser build unconditionally, so
typst plugins run there at no marginal cost -- and now one demonstrably does.

### Known artefact, for the mesh work

A contour's x and y are grid *axes*, not a list of points, but `XY_SERIES` treats
slots 0 and 1 as paired coordinates -- so the canvas draws 50 markers down the
diagonal of the contour cell. Harmless (they cannot be dragged: `lq.linspace(..)`
is not a literal array) but misleading, and it predates this change; the binding
fix merely made it visible. The mesh-shaped series need a hit region that is the
area they cover rather than a synthesised diagonal.

## A colormesh is a grid, and lilook was reading it as a diagonal

`XY_SERIES` meant one thing -- "slots 0 and 1 are x and y" -- and that is simply
false for a third of its members. `colormesh(xs, ys, z)` over 60x40 was read as
**40 paired points down the diagonal**: zipped, so truncated to the shorter axis,
drawn as draggable markers corresponding to nothing in the figure, with both axes
reporting the wrong length and `z` never recovered at all.

`SeriesShape` now distinguishes them, and the shape travels with the data in the
probe's metadata, because how to read `x` and `y` depends on it:

- **`Points`** -- `plot`, `scatter`, `bar`, `stem`, `quiver`, and now
  `fill-between`, whose second surface arrives as the `y2` channel. Parallel
  arrays, one point per index.
- **`Mesh`** -- `colormesh`, `contour`, `mesh`. Axes of independent length, so
  `points` is empty, `grid` carries `(columns, rows)`, and the axes are stored
  whole as channels rather than zipped against each other.

A mesh is then picked by **the area it covers** (`Scene::hit_mesh`), which is what
it is on the page: a field with no vertex to aim at. Vertices and segments are
tried first, so a scatter drawn over a colormesh is still pickable. `hit_mesh`
also returns the nearest grid cell, row-major, which is what a value readout will
need.

`has_literal_points` is false for a mesh whatever its axes were written as: there
is no point to move even when both are literal arrays.

### The wording belongs in the core

The tree kept saying "0 pts" for a 60x40 field after all of the above landed,
because the edit meant to change it silently failed to match -- a `str.replace`
with no assertion, after `cargo fmt` had reflowed the lines. Every test still
passed, since none of them looked at the label.

So `SeriesGeom::summary()` decides the wording now, in the core, where a test can
assert it without driving a UI: `assert_eq!(geom.summary(), "60×40 grid")`. Two
frontends cannot describe the same series differently, and the next silent
no-op fails a test instead of shipping.

### Two drive-by fixes

- **`scripts/web.sh` could not do a debug build on macOS.** `set -u` plus
  `"${FLAGS[@]}"` on an empty array is an unbound-variable error in bash 3.2,
  which is what macOS ships, so `scripts/web.sh` with no arguments died before
  compiling anything. Only the `release` path had ever been used.
- **A blank page that was not a bug.** Rebuilding under the same filenames left
  the browser holding a cached `lilook_web.js` against a fresh `.wasm`, which is a
  `LinkError` and a blank canvas -- and it read exactly like the earlier
  genuinely-broken deploy. Serving on a new port made it render immediately.
  Worth knowing before diagnosing the next blank page: check for a LinkError
  before suspecting the code.

## Rules: one line per argument, so one argument per drag

`hlines(1.5, 2.5)` is two lines in one call, and each coordinate is a whole
*positional argument* rather than an element of an array. That makes the edit
`SetPositionalArg`, not the `SetArrayElement` a point drag uses -- writing an
array element into `hlines` would be rewriting an array that is not there.

`SeriesShape::Rules(Axis)` carries which axis the coordinates live on. The probe
gathers every positional slot into one array and sends it on that axis, leaving
the other empty: a rule spans the frame, so its other coordinate does not exist.
`Scene::hit_rule` therefore measures only the distance *across* the line, which is
also why `hit` cannot find one -- there is no vertex to be near.

Two details worth keeping:

- **`hit_rule` returns the argument index**, not a point index, because that is
  what the edit needs. The test asserts the *second* argument moves when the
  second line is grabbed and the first stays put.
- **`editable_series` requires every coordinate in the call to be literal.** The
  canvas gets one flag per call, so a partly-computed `hlines(1, threshold)` would
  otherwise offer a drag it could honour for one line and not the other.

A rule is grabbable without being selected first, unlike a point. That is
deliberate: a point drag needs the selection to disambiguate overlapping series,
whereas a line spanning the frame is unambiguous about what was grabbed.

`SeriesGeom` now carries its `shape` rather than having `grid` stand in for it.
Three shapes was the point at which "mesh or not" stopped being enough, and a
consumer holding only a `Scene` -- the canvas, a host frontend -- now reads the
geometry the same way the probe wrote it.

## Distributions: one dataset per argument, positioned by a named argument

`boxplot(a, b, c)` is three datasets in one call, each its own positional
argument, and their positions come from a named `x:` -- which defaults to `auto`,
meaning `1..n`. So without resolving `auto` the positions are unknown in the
*commonest* case and there is nothing to hit-test against.

They are resolved in typst, inside the injected metadata:

```typst
((p) => if type(p) == array { p } else if p == auto { range(1, n+1) } else { (p,) })(<x>)
```

which is the same trick the linked-dataset work used for column discovery: ask the
compiler rather than reimplement its defaults. Each dataset comes back as its own
channel, so the inspector reports how many values went into each box, and a linked
file can later be checked against those lengths.

`hit_distribution` picks by nearest position, but only when the pointer is within
the *range of values that went into that box* -- lilook does not recompute the
quartiles, so the region it claims is the data's own extent rather than an
assertion about where the whiskers ended up. Adjacent categories split the gap
between them, so two boxes never claim the same pixel.

Hit-test precedence across all four shapes is now by how precisely the user aimed:
a vertex or segment, then a rule, then a distribution or a mesh area. A scatter
drawn over a colormesh, or a marker sitting on a threshold, still wins.

### The same bug, caught before shipping this time

The inspector offered "materialise" on every dataset of a boxplot. `points` is
empty for a mesh, a rule *and* a distribution, so it would have written `()` into
the slot and broken the figure -- exactly the defect that put `xlim: ()` on a live
page earlier today. The offer is now gated on the series being paired-point shaped
*and* having points, and a test walks the three shaped examples asserting none of
them has points to embed.

Worth noting what caught it: not a test, but looking at the screen after the
feature "worked". The three shapes had been added one at a time, and the filter
had been written for the first of them.

### Not done: dragging a box's position

Moving a box along its axis means rewriting one element of the *named* `x:`
argument, and no intent addresses an element of a named array -- `SetArrayElement`
indexes positional slots. Adding one means a new `Intent` variant, which means the
`random_intents_fully_undo` generator, and for the default `x: auto` there is no
array to edit at all until the positions are materialised first. Recovery,
selection and the readout are in; the drag is a separate piece of work and is not
pretended at.

## Annotations: the same handles, three different edits

`place`, `rect` and `ellipse` keep their coordinates as two *scalar arguments*;
`line(start, end)` and `path(..vertices)` keep each vertex as an `(x, y)` array.
Both fill `points`, so `Scene::hit` finds their handles with no new case -- what
differs is the **edit**:

| shape | coordinates live in | drag writes |
| --- | --- | --- |
| `Points` | parallel arrays | `SetArrayElement { arg: 0/1, element: i }` |
| `Anchor` | two arguments | `SetPositionalArg { index: 0/1 }` |
| `Vertices` | an array per slot | `SetArrayElement { arg: i, element: 0/1 }` |

All three are two intents under one coalesce pair, so a drag is one undo step. No
new `Intent` variant was needed: the existing vocabulary already addressed all
three places, which is a good sign about the vocabulary.

### The bug, for the third time

I made the probe report Anchor and Vertices as `"points"` to get hit-testing for
free -- and that lost exactly the distinction the editor needed. The inspector
then offered "materialise" on the ellipse's `x`, which would have written
`(7.85,)` where a scalar belongs. Third outing for the same defect in one day:
`xlim: ()`, then boxplot datasets, now an anchor.

The shape travels with the geometry now, and `points` is still populated, so
hit-testing stays free *and* the editor can tell what it is looking at. The
lesson is not "add another filter" -- it is that a type which lies to make one
thing easy will be believed by everything else.

Caught by looking at the screen, again. Two of the three were.

### `-1` is not a literal, which was true everywhere

`lq.ellipse(7.85, -1, ..)` refused to be draggable, and the reason had nothing to
do with annotations: `-1` parses as a *unary negation* of `1`, and `classify` had
no arm for `Unary`, so it fell through to `Opaque`. **Every negative number
argument in every lilaq call** was therefore treated as an expression lilook
must not touch -- `margin: -2pt`, `aspect-ratio: -1`, any coordinate below zero
-- and got a raw source editor instead of a number control.

`split_numeric` had always read the sign. The classifier simply never let it.
One arm, and a test over `-1`, `-2.5`, `+3`, `-1pt` and `-y` -- the last staying
opaque, because a negated *expression* is still a program.

## The completeness sweep, and the crash it found

Measured rather than recalled: lilaq 0.6.0 has 48 functions, 17 elements and 409
parameters, cross-referenced against lilook's own tables.

**No lilaq function is unknown to lilook.** Every one is a recognised series, a
structural call (`diagram`, `layout`, `grid`, `axis`, `colorbar`, ...), a helper
that returns data (`linspace`, `percentile`, ...) or an element. Set-rule coverage
is complete *by construction* -- the "add style" menu is built from
`schema.elements` at runtime, so all 17 are offered and a lilaq release that adds
one gets it for free.

Only two real data gaps: `bar`'s `width` and `base`, both one value per bar, now
recovered as channels.

### And then: lilook crashed on a document that compiles

`datetime` axes. lilaq plots them; lilook assumes numbers everywhere. Three
distinct failures, in increasing seriousness:

1. The series recovers **no points**, so it is invisible to the canvas.
2. A pan would write `xlim: (0, 100)`, which *compiles* -- silently swapping a
   calendar axis for a numeric one. Compiling is not the same as being right.
3. **`typst_render` panicked**, on a figure that `typst compile` handles fine.

The third is the one that mattered. lilook's *scale probes* are placed at data
coordinates -- `0.1`, `0.5`, `0.9` -- and lilaq maps those onto a calendar, so an
`auto`-sized page grows without bound. `typst_render::render` allocates its pixmap
with `Pixmap::new(w, h).unwrap()`, which returns `None` when the allocation is too
large, and the panic is inside a call no caller can catch. In the desktop build
that kills the compile thread; in the browser, where the profile is
`panic = "abort"`, it kills the app.

Three fixes, in order of what each buys:

- **Never call the renderer on an implausible page.** 64 megapixels is far beyond
  any figure and far below what fails to allocate. This is the guard that turns a
  crash into a degraded view, and it protects against any future cause, not just
  this one.
- **Retry without the scale probes.** They are the only markers placed at data
  coordinates. Without them the layout is the user's own and the figure draws
  normally; what is lost is a transform that was never recoverable for that axis.
- **Say so.** `Scene::numeric` records that the axis is not usable in data space,
  and the canvas declines the data pan, falling back to panning the view. The
  figure still moves under the pointer; it just does not rewrite the document.

The corners are relative (`0%`/`100%`), so they are safe on any axis and still give
the frame -- which means a datetime figure can be selected and resized even though
its data cannot be touched.

**Not done:** reading datetimes as data and writing them back. That is a third
axis property alongside linear/log -- the axis's *data type* -- and it needs a way
to write `datetime(year: .., month: ..)` from a drag. Scoped, not started, and the
gestures that would produce nonsense are declined in the meantime.

## Recovering a colormesh's field costs 7%, because comemo has already paid for it

2026-07-30. A mesh's `z` is usually a *function* -- `(x, y) => calc.exp(..)` -- so
recovering the field means evaluating it over the whole grid through typst's
interpreter. At 300x200 that is 60,000 calls, and the fear was that a
value-under-the-cursor readout would be affordable only up to some grid size,
needing a cap and a story about what happens past it.

Measured before building anything, warm, with `comemo::evict(0)` between cases:

| grid | cells | without `z` | with `z` |
| --- | --- | --- | --- |
| 20x15 | 300 | 10.2 ms | 8.8 ms |
| 60x40 | 2,400 | 13.9 ms | 14.5 ms |
| 120x90 | 10,800 | 35.1 ms | 37.7 ms |
| 300x200 | 60,000 | 155.4 ms | 167.0 ms |

**About 7%, flat, and inside the noise at the sizes anyone actually plots.** The
reason is the same one that makes the whole probe technique work: lilaq has
already called that closure for every cell to draw the figure, and comemo memoises
it, so the probe's evaluation is a cache lookup rather than a second computation.
The 300x200 row is dominated by lilaq drawing 60,000 quads, not by the field.

So recovery is unconditional -- no cap, no size-dependent behaviour, no explaining
to the user why the readout stopped working. The same measurement says the *first*
attempt at this would have been wrong in the other direction too: a flat 7% is not
worth an opt-in toggle either.

Two conventions were pinned by test rather than by reading lilaq's source. The
field comes back as rows over y and columns over x, flattened row-major so
`hit_mesh`'s single index still names one cell; and an explicitly-written 2-D
array is passed through unchanged in that same shape. The array test uses a
deliberately non-square grid -- 5 columns by 3 rows -- because a transposed
convention there does not read the wrong number, it fails to compile, which is a
much better failure than a plausible one.

## A random walk of gestures found two defects on its first run

2026-07-30. `random_intents_fully_undo` verifies, without a compiler, that any
sequence of *intents* fully undone returns the buffer byte-for-byte. What it
cannot check is whether the result was a document typst accepts -- and "it parses"
has been mistaken for "it compiles" here repeatedly.

`crates/lilook-web/tests/walk.rs` is the same idea one level up: random
**gestures**, over every example in the gallery, recompiled after each one. Two
properties, both previously violated by real code -- every intermediate state
compiles, and undoing the walk restores the source byte-for-byte.

It found two things immediately, neither of which any existing test could have:

**A resized figure could exceed the GPU's maximum texture side.** Six resizes and
a gallery example was 2196 px across, against a limit of 2048; `egui` panics on
that rather than returning an error. The limit is real and low on mobile GPUs, and
dragging a diagram bigger is an ordinary thing to do. The raster scale is now
capped to what this machine's textures hold, recomputed from every render so it
relaxes again when the figure shrinks, and an over-large render keeps the last
good pixels instead of uploading.

**`SetLimits` could write a negative bound onto a log axis** -- the very failure
the log-pan work fixed a week earlier. That fix was correct but in the *caller*:
the canvas pans through `AxisMap::shifted`, which cannot leave a log axis. A
gesture is the editor's public vocabulary and arrives from three shells, so the
guard belonged where the document is written. It does now, and a log axis keeps
the limits it had rather than being handed an invented positive number.

The methodological point is the second one. The obvious repair after the first
failure was to teach the generator to pan through `shifted` like the canvas does
-- which would have made the test pass and hidden the defect completely. A
property test that only produces already-safe inputs is testing its own generator.

## The layering is clean except for one crate, and it is measurable

2026-07-31. Audited how much of lilook is portable Rust versus how much is tied
to egui, since the answer decides how cheap a Swift frontend and a useful MCP
server are.

| crate | lines | needs egui |
| --- | --- | --- |
| lilook-core | 8,140 | no |
| lilook-compile | 3,644 | no |
| lilook-data | 3,490 | no |
| lilook-ffi | 332 | no |
| lilook-ui | 4,650 | **yes** |
| lilook-editor | 4,560 | **yes** |
| lilook-app / lilook-web | 2,038 | yes (shells) |

58% is GUI-free, and the core/compile/data split holds up: the document model,
the probe technique, every decoder and every emitter are portable today.

**The exception is `lilook-editor`, and it is not marginal. 49 of its 62 methods
never mention egui.** Every link operation, `unlock`, all four theme operations,
paste, duplicate, delete, and `handle_canvas` -- the entire gesture vocabulary --
are pure document logic sitting behind a GUI dependency. Same shape one level
down: 18 of `inspector.rs`'s 22 functions are pure schema policy (`control_of`,
`seed`, `takes_text`, `sentinel_of`, `widget_control`, `shape_hint`), which is
exactly what an MCP server needs to tell an agent that a parameter takes a
length, defaults to `1cm`, and treats `none` as unset.

The cost is already visible rather than hypothetical. The MCP server exposes four
tools -- inspect, schema, set_arg, add_arg -- and the FFI exposes ten raw
document primitives. Neither can link a data file, unlock a dataset, switch a
theme or perform a gesture. Not because those are GUI concerns, but because they
live in a GUI crate.

**The remedy is a move, not a rewrite,** and the struct already shows the seam.
`Editor` holds five render fields (`canvas`, `textures`, `pages`,
`max_pixel_per_pt`, `rendered_at`) and twenty document fields (`doc`, `schema`,
`selected`, `link`, `data_files`, `scenes`, `clipboard`, ...). Split it into a
GUI-free `Session` carrying the document state and the 49 methods, plus an
`Editor` that owns a `Session` and the panels. Move the 18 policy functions to
core beside the schema. Then Swift and MCP get the vocabulary the GUI already
has, and "the editor never touches the compiler" gains a sibling: "the panels
never touch the document."

**Done**, the same day. `lilook-editor` went from 4,560 lines and 62 methods to
1,974 and 18; `Session` in `lilook-core` holds 48 methods of which **none**
mention egui. The GUI-free share of the codebase went from 58% to 69%.

Three things the move turned up that the audit had not predicted:

- **`CanvasEvent` and `UiEvent` had to move too.** They are the gesture
  vocabulary -- plain data about call sites and data coordinates -- and they were
  sitting in the egui crate purely because that is where the widgets that emit
  them live. A drag is a drag whether a mouse or an agent asked for it.
- **`SlotSource` and `split_numeric` were likewise misfiled**: provenance and
  value parsing, neither of them a widget.
- **`Deref` earns its keep here.** `Editor` derefs to `Session`, so every shell
  and every test kept working unchanged. The one place it does not work is
  `self.canvas.ui(.., &self.scenes)`, where the deref borrows the whole editor
  and collides with the mutable canvas borrow; naming `self.session.scenes`
  keeps the two disjoint. That is the only site in the codebase.

The receipt was the compiler's: after the move, `lilook-editor` no longer needed
to import `Document` at all.

## One MCP tool per lilaq function costs 18,000 tokens a turn; five tools cost 500

2026-07-31. Measured before building, because the shape of the MCP server
depended on the answer. Tool definitions are re-sent on every request, so the
standing cost is what matters, not the one-off.

| surface | tokens | paid |
| --- | --- | --- |
| whole schema as one blob | 44,550 | — |
| one tool per function (48) | 17,980 | **every request** |
| terse capability index | 810 | once, on demand |
| `describe` for one function | 47 | only when used |

So discovery is hierarchical: five fixed tools (`doc`, `capabilities`,
`describe`, `edit`, `render`), an index that names functions without their
parameters, and expansion only for what the agent will actually use. An agent
spends ~1,000 tokens to learn what it needs against ~18,000 per turn for the flat
surface, and gets *more* detail on the functions it uses rather than less.

Nothing in the server enumerates lilaq. `capabilities` and `describe` are
projections of the generated schema, and `edit` is a projection of `Session` --
the same operations the GUI performs, which is what the Session extraction was
for. A new lilaq function needs no change here.

`render` reports a scene readback -- what was drawn, point counts, axis ranges and
scales -- not just diagnostics, so an agent can check its work without pixels.
`edit` recompiles and includes that readback in its own reply, because an edit
that does not compile is the thing an agent most needs to hear about and hearing
it a round trip later is a wasted turn.

Two things driving it turned up that review had not:

- **Node ids renumber within a batch.** They are positions in a document-order
  walk, so an op that inserts text above a node (a theme, a style rule) moves it.
  The same hazard `commit_link` already documents. The tool description now says
  to order such ops last, and every reply lists the ids as they are afterwards.
- **`schoolbook` is incompatible with a log axis** -- it sets `position: 0`, and
  lilaq answers "value must be strictly positive". Not a lilook bug, and the
  server reports it verbatim, which is exactly what an agent needs.

## The axis guard needed an invariant, not a better fit test

2026-07-31. The first attempt at guarding the scale recovery checked whether the
log fit reproduced the third probe. It was committed unverified, because no lilaq
fixture could be made to demonstrate it firing. Driving the four cases directly
showed why:

| scale | recovered as | y range |
| --- | --- | --- |
| linear | Linear | 0.82 … 4.18 |
| log | Log | 0.66 … 1513 |
| custom `v/(1+abs v)` | **Linear** | **0.846 … −1.749** |

The custom scale passed the bend test -- the three probes did look straight in
page space -- and produced **a maximum below its minimum**. Nothing failed and
nothing was reported; every pan and drag on that figure would have written
nonsense into the document.

What catches it is the cheapest thing available and needs no knowledge of which
scales lilaq has: *an axis whose recovered maximum is below its minimum is not a
transform.* The figure then degrades to frame-only -- the path a datetime axis
already takes -- so it still draws, selects and resizes while the gestures fall
back to moving the view.

The lesson is about the shape of the check rather than the bug. Testing the fit
asks "is this one of the two things I know?", which is a coin flip when the answer
is neither. Testing an invariant asks "is this result possible at all?", which is
answerable without enumerating the alternatives.

## Export: PDF, SVG and PNG, from the document already on screen

2026-07-31. The release blocker. All three come from `Backend::document()` -- the
same compiled document the canvas is showing -- so what lands on disk cannot be a
second, subtly different compile, and an export costs only the encode.

PDF and SVG go through typst's own exporters, which is most of the argument for
building on typst. PNG stays a forty-line encoder writing stored deflate blocks:
bigger on disk, readable everywhere, no dependency.

Reaching all four frontends took one shape each. The desktop compiles on another
thread, so export is a round trip through the actor beside `query`. The browser
has no file system, so "write a file" is a blob URL and a synthetic click -- a
blob rather than a data URL because a 300 ppi PNG is megabytes and some browsers
refuse a data URL that size outright. The MCP server picks the format from the
extension and defaults to PDF, on the grounds that a bare name usually means a
paper.

## Diagnostic ranges are unusable, in two different ways

2026-07-31. Measured before planning the language-server capabilities, because
code actions were going to be built on them.

Six representative errors, through the path the app actually uses:

| what the user wrote | range reported | where it points |
| --- | --- | --- |
| `bogus: 1` — unknown argument | none | — |
| `width: "wide"` — wrong type | none | — |
| `xlim: ()` | none | — |
| `yscale: "log", ylim: (-1, 10)` | none | — |
| `lq.plot(nope, ..)` — undefined name | `781..785` | past the end of a 200-byte file |
| unclosed delimiter | `772..773` | past the end |

**Most lilaq errors carry no location.** lilaq validates through `elembic`,
inside the package, so `span.id()` is the package's file rather than the user's
and `world.rs:193` correctly declines to report a range. Four of the six most
common failures have no span to follow -- including every entry in the quick-fix
catalogue.

**The two that do have spans point into the derived buffer.** Probes are injected
into each diagram's argument list, so every offset after the first diagram is
shifted by the length of what was inserted. Harmless while diagnostics are only
displayed as text; a live defect the moment anything tries to highlight one.

The design consequence is the useful part. Code actions cannot be driven off
diagnostic ranges and **should not be**: they are driven off *(message,
document)*. lilook knows without any span which call has `xlim` set to `()`, and
which diagram pairs a log scale with a non-positive limit. That is more robust
than span-following -- it survives lilaq moving its assertions -- and it uses the
thing lilook has that a text-only language server does not: a parsed document and
a recovered scene beside the error.

## An error with no location can be found by removing things: ~4 ms a variant

2026-07-31. The obvious consequence of the previous finding was that lilook would
have to guess where a spanless error came from, by matching its message. It does
not have to guess. It can ask the compiler.

Remove one argument from a scratch copy, compile, and see whether the error
survives; the removal that clears it names the culprit. Delta debugging, on
documents small enough that it is simply affordable:

| | |
| --- | --- |
| first compile (failing) | 66 ms |
| one variant, warm | **4.0 ms** |
| 9 variants | 36 ms |

Cheaper than the first compile, because a failing compile stops early and comemo
has already cached everything that did not change.

Two properties make this better than matching on message text:

- **It produces the byte range the diagnostic was missing**, in the user's own
  buffer. `value must be strictly positive` -- which arrives with no span at all
  -- comes back as ``caused by `yscale: "log"` (bytes 119..124)`` and ``caused by
  `ylim: (-1, 100)` (bytes 132..141)``.
- **It is evidence, not a guess.** The claim is falsifiable and was falsified: the
  document without this argument does not have this error. It survives lilaq
  rewording anything, because it never reads the message except to compare it
  with itself.

Two things the tests caught rather than review. An error that needs *two* causes
-- a log scale and a negative limit -- is cleared by removing either, so both are
reported; naming one would be arbitrary. And `locate` must first confirm the
error is present at all, or an error nobody had is "cleared" by every removal and
the whole document is indicted.

## tinymist-query needs a patched typst, so lilook cannot have it

2026-07-31. Investigated as the way to give lilook's source pane the typst-general
completion and hover it lacks. The intended trade was bundle size against
capability; the actual answer was neither.

`tinymist-query 0.15.4-rc1` resolves `typst 0.15.1` from crates.io and then fails
to compile against it:

```
error[E0432]: unresolved import `typst::foundations::FuncInner`
error[E0599]: no method named `inner` found for struct `typst::foundations::Func`
```

`FuncInner` does not exist in published typst. tinymist builds against its own
patched typst, so linking `tinymist-query` would mean replacing lilook's typst
with that fork -- and the probe, the render, the export and every pixel-identical
guarantee would then depend on a compiler lilook does not control. That is not a
size trade-off, it is a foundation trade-off, and the answer is no.

The size number was going to be the argument and is now only a footnote: 379
crates, against 321 for the whole of `lilook-web`, including tokio, protobuf and
dashmap.

**What was done instead.** A hand-kept table of the twenty-six typst expressions
people actually write inside a figure -- `calc.*`, `range`, `float`, `rgb`,
`luma`, `gradient.linear` -- offered where an expression goes: in a data slot, and
beside a numeric value. Each entry is asserted to parse, so the table cannot rot
quietly.

This is the second time this question has come out the same way. `typstyle-core`
was worth linking because it is a small library with a narrow contract;
`tinymist-query` is not because it dictates the compiler. **Borrow a library, not
a foundation.**

## Two panes offered the same list and disagreed about what picking one meant

2026-08-01. The source pane's completion popup wrote a whole argument on one
click: `interpolation: "smooth"`, name and value together. The inspector's "add
argument" combo, over the same parameters, took three acts — choose a name from
the menu, press `add`, then find the control and set the value — and the value it
started from was chosen by different code. Same list, same job, two behaviours,
because there were two implementations.

There is one now, in two pieces.

**`policy::argument_offers` is the list.** Every argument a call is missing,
offered once with a safe value and again per choice where it has a small fixed
set. `Session::completions` maps it into `Completion`s for the text pane; the
inspector maps it into `UiEvent::Insert`. Making it one function moved two
decisions out of the frontends and settled them:

- **Positional parameters are not offered.** The popup used to list them, and
  `lq.plot(x: xs)` is "unexpected argument: x" — an offer that cannot work.
- **The value falls back to the documented default, sentinel or not.** The seed
  policy skips `auto` and `none` deliberately, which is right for `set` on a
  parameter the user is looking at and wrong for adding one blind: `xscale` has
  no seed, and the shape placeholder for a scale is `none`, which parses and does
  not compile. `ArgumentOffer::written` prefers the default it was given.

**`lilook_ui::pick` is the popup.** One `Area`, one filter, one row. Three things
the shared version had to fix that neither copy had right:

- **A row is one click target, picture included.** `selectable_label` senses only
  the text it painted, so the colour ramp beside a colormap's name — the widest
  thing in the row and the only part that says what the map looks like — was
  inert. So were the cycle swatches. `pick::click_row` interacts with the whole
  row and paints the hover highlight under it, and labels inside a row are made
  non-selectable, because draggable text in the middle of a row is a hole in it.
- **Focus is not enough to keep a popup open.** egui surrenders a text field's
  focus the moment the pointer clicks anywhere else — the popup included — and a
  click is a press in one frame and a release in the next. A popup gated on focus
  alone disappears *between the two*, so every row in it is unclickable. It is
  gated on focus **or** the pointer being inside the popup drawn last pass. The
  source pane needed the same fix from the other end: egui only reports
  `cursor_range` for a focused field, so the caret is now remembered across that
  frame. **This was not a regression to guard against — it was the state of the
  source pane.** Clicking a completion did nothing at all, in a popup whose
  offers were tested through `Session` rather than through the pointer — the list
  was right, and nothing in it could be taken.
- **A pane that shows its own copy of the text has to be told.** With the click
  landing, the next defect was visible: the source pane reads the document only
  while it does *not* have focus, so an accepted completion reached the document
  and the pane went on showing the half-typed word that asked for it. The copy
  and the caret are carried across the accept, which is also what lets typing
  continue where it left off.
- **A popup is a fixed width.** An `Area` sized to its longest label leaves the
  rest of every shorter row hanging outside the frame: painted as part of the
  row, hit-tested as background. Found by a click that landed on the row's own
  rectangle and did nothing.

Tested by driving real pointer input at a chosen part of a row — `pick::popup`
derives each row's id from its own, so a test can aim at the ramp rather than the
name. Aiming needs a settled layout: an `Area` that has never been laid out does
not know its height and is placed again once it does, so the first frame's rect
is a row out.

## A recompile gate on what "add argument" writes found two values that never compiled

2026-08-01. `seeded_arguments_compile` exists because `set` once wrote `()` into
`xlim` and reached a browser: reparsing is not the gate, the compiler is. Adding
an argument is the *other* value-writing path and had no such gate, so it got
one — `offered_arguments_compile`, over every name `argument_offers` offers for
`lq.diagram` and `lq.plot`. Thirty-three values, and two of them did not compile.

**A documented default is written in lilaq's scope, not the user's.**
`diagram`'s cycle defaults to `petroff10`, which is a binding inside the package.
Copied into a figure it is "unknown variable: petroff10" — syntactically a
perfect identifier, semantically nothing, which is precisely the gap `check_expr`
cannot close. `resolves_anywhere` closes it, over the same `BUILTIN_IDENTS` table
that tells a colour swatch from a jump-to-definition: it is the same question
asked at the other end. Any default that names something only its own module can
see is now declined, and the typed fallback stands in.

**`cycle` takes an array, and lilaq exports no name for one.** With the default
declined, the seed wrote `"petroff10"` — and lilaq answered "expected array,
found string". The three named palettes in `CYCLES` had never been settable: the
picker offered them, and every one of them produced a figure that would not
build. They carry lilaq's own colours now, read out of `src/style/map.typ` in
0.6.0, so the offer means what its name says and the swatch row parses the same
array it writes. lilook no longer keeps a second table of those colours.

Both were reachable from the inspector's cycle menu *and* the source pane's
completion popup, and neither was reachable by any test: `cycle` has no sentinel,
and `seeded_arguments_compile` only walks parameters that have one. The lesson is
not about cycles. **A gate that skips silently when its fixture is unavailable
reports green from an empty run** — every compile-backed test here returns early
when the lilaq package cannot be fetched, which is exactly the state a sandboxed
agent works in, and it is indistinguishable from a pass in the output.

## A caption beside a click target is a promise, and this one was a lie

2026-08-01, reported from the deployed page. `lq.colormesh`'s `interpolation` sat
in the add-argument list as

```
interpolation    "pixelated"|"smooth"
```

and clicking the word `smooth` wrote `pixelated`. Every time.

Three separate mistakes stacked into one bad minute for the person using it.

**The note was the types, and the types of that parameter are its values.**
`p.types.join("|")` is honest for `length|relative|auto` and a trap for an enum,
where the type *is* the set of literals. Beside a row that writes one fixed
thing, it reads as a menu. It was fine while only the name was clickable —
making the whole row a target is what turned a caption into a control. The rule
now: **a note must not name a value the click will not write.** The note is the
widget kind (`enum`, `length`, `scale`); the types moved to the hover.

**The values were there, three rows down, and the list was cut before them.**
The offers ran name, value, value, name, value…, capped at twelve rows — and on
a colormesh the cut fell exactly between `interpolation` and its `pixelated` and
`smooth`. The only row you could see was the one that could not write what it
advertised. A cap by *count* hides; the popup now bounds its *height* and
scrolls, so nothing is offered-but-unreachable.

**And the list was three times longer than the thing it listed.** One row per
value made a seven-argument call into a twenty-row menu. So the values moved
onto their parameter's own line, each its own target: `interpolation  enum
pixelated  smooth` — one row per argument, one click for either meaning of it.
The bill for that: `argument_offers` returns one offer per parameter carrying
`choices`, and `pick::popup` answers with a row *and* which value on it.

The egui detail that makes it work is worth keeping. **Whoever registers last
wins the hit test**, so a row that claims its rectangle after laying out its
contents swallows every widget inside it. `click_row` now allocates and claims
the row first, then builds the contents in a child `Ui` — the chips are
registered later, so the pointer reaches them, and the row still answers for
everything that is not one.

Neither the source pane nor the inspector got to keep its own version of any of
this: both read `argument_offers` and both draw `pick::popup`, so `xscale scale
linear log symlog datetime` is one line in the text pane too.

## A completion is a text edit, and a text edit does not know it is in a list

2026-08-01, reported. Accepting `xscale: "log"` between `width: 6cm,` and the
series on the next line produced a figure typst refused: "expected comma (byte
168)". The completion replaces the word being typed -- that is what makes it a
completion rather than a regeneration -- and a byte-range replacement knows
nothing about the argument list it lands in.

`fit_argument` supplies the separators, and the shape of it is the interesting
part. **The comma before belongs against the argument it follows, not at the
start of the line the caret is on.** So the replaced range is widened backwards
to the end of the previous token and the whitespace between is reproduced:

```
  width: 6cm          width: 6cm,
  xsc          ->       xscale: "log"
```

rather than `width: 6cm\n  , xscale: "log"`, which is what appending to the
value would have written.

Three things are deliberately left alone. A completion that is not `name: value`
-- a value inside an argument, `calc.sqrt(` in a data slot -- is not in a list
and gets nothing. A name whose value the user has still to type ends in a colon,
and `xlabel: ,` is worse than the miss it fixes. And a trailing comma before `)`
is not added: the argument-insertion path already learned that lesson once, in
the trap `InsertNamedArg` is written around.

The inspector never had this bug, because `InsertNamedArg` computes its own
separator from the call site. Two paths write arguments and only one of them
knew where it was.

## A library of your own, and the rule that keeps it safe

2026-08-01. Palettes, colour maps and themes you have saved, offered beside
lilaq's. The interesting part is not the editor, it is the constraint the whole
feature is built inside.

**A library is a shelf of offers. It is never an input to rendering.** Choosing a
saved palette writes its colours into the document, exactly as choosing a
built-in one does — so the document still says everything about how the figure
looks, and a co-author who has never seen the library gets the same picture.
Delete `prefs.toml` and no figure changes. The moment a figure needed the library
to draw, every other property this tool is built on would go with it: a document
you can paste into a manuscript, a compile that means the same thing everywhere,
a `git diff` that shows what changed.

That rule decides the design. `SavePref` is not an edit and opens no transaction.
The renderer never sees `Prefs`. And the value is checked with `check_expr` on
the way *in*, because a library that can hold something unwritable is a library
that breaks a figure later, at a moment nobody connects with having saved it.

**One table, three kinds.** A palette, a colour map and a theme are the same
thing — a name bound to a typst expression, offered wherever that kind of
expression is chosen — so they share `Saved { kind, name, value }` and one file.
Three tables would be three formats to migrate the first time one of them grew a
field. The cycle editor is the first editor over that table; the other two slot
in without a format change.

**Where it lives is the shell's business.** `lilook-core` has no file system,
which is exactly what lets the same editor run in a page: the desktop keeps the
library in the platform's config directory, the browser in `localStorage`, and
both read and write the same TOML. The core hands over a `Prefs` and a dirty
flag, the way it hands over an `Extraction` for a figure written to disk.

Two things this caught while it was being built, both by doing it for real rather
than by reasoning about it. A derived `Default` stamped a fresh library
`version = 0` — a format that never existed, which a later lilook would have had
to guess the meaning of. And `web_sys` off wasm is a panic rather than a stub, so
the browser's storage is behind `cfg(target_arch = "wasm32")` with a native
fallback: this crate's own tests run natively, and they start from an empty
library rather than from a panic.

## The other two kinds, and the one honest thing a colour map editor must say

2026-08-01. The library grew its remaining two editors, and each turned out to
be a different shape of the same problem.

**A colour map is the palette editor with a different preview.** Both are an
array of colours; lilaq reads one as the series to draw with and the other as
stops to interpolate between. So there is one editor, and it differs in what it
paints under the row -- blocks for a palette, an interpolated ramp for a map --
and in which argument the result goes to.

The one thing it must not do is pretend. Opening `new…` while the figure uses
`color.map.viridis` starts from **lilook's five-stop preview of viridis, not
viridis**: the real map is 256 entries inside typst and lilook has never had
them, which is exactly why `colormap_stops` says "approximate on purpose" at the
top. Five stops is a fine place to start a map of your own and a lie if it were
handed back as viridis, so it opens already named as yours and the hover says
where the colours came from. (Reading the true stops is possible -- the probe
machinery evaluates expressions in the user's own scope -- but that is a compile,
and it belongs to `lilook-compile` rather than to a menu.)

**A theme is a function, so what gets stored is the closure.** `#let mine = it
=> {..}` cannot be checked by `check_expr` and cannot be pasted into another
document under a name that may already be taken there. So the library keeps the
`it => {..}` and `use_saved_theme` writes the `#let` itself, choosing a free
name -- which means the receiving document ends up carrying the whole theme, and
that is the point: it travels with the file to someone who has no library at
all.

Only a theme of the user's own can be kept. lilaq's are functions in a package,
and storing a copy would go stale the moment lilaq revised one -- the same reason
`fork_theme` composes rather than copies, decided once and now decided again for
the same reason.

## Storage's second obligation: never be the one who destroys the file

2026-08-01. Adding a library was the first half. The second half is that a tool
which keeps a file has to be trusted with it, and lilook was not.

`load` reported an unreadable `prefs.toml` and started empty. The session then
carried on, and the next `SavePref` wrote the empty-plus-one library straight
over the user's file. A stray keystroke in a config file cost everything in it,
one save later. The case that had been *explicitly* guarded was worse: `from_toml`
refuses a library written by a newer lilook -- and then the older binary
overwrote it anyway.

The lesson generalises past this bug. **Reporting is not a remedy when the user's
next ordinary action does the damage.** The report is read by a person; the
overwrite is done by the program, sooner. So the unreadable text is now carried
back out of the load as `rescue`, and neither shell will write until it has been
put somewhere safe -- numbered, and never over an earlier rescue, because that
copy may be the one that matters.

Three more rules came out of building sharing on top of it:

**A file has to say it is a library.** Every field had a default, so any TOML at
all parsed as an empty library and a dropped `Cargo.toml` was answered with
"nothing in that library". Found by writing the test for the refusal and
watching it not refuse.

**An import keeps both sides.** A name clash renames the *incoming* thing rather
than replacing what is here. Someone else's `warm` is not yours, and an import
that quietly overwrote it would lose a palette the same way the truncation did.

**An import is a save path.** `merge` pushed entries straight in, skipping the
`check_expr` that `save` applies -- so a library file could carry something
lilook would then offer in a menu and write into a figure typst cannot compile.
The check belongs at every door into the library, not at the one that happened to
be built first.

And a note on evidence: while chasing a preview that looked wrong in a
screenshot, the *capture* turned out to be rotating colour channels. The previews
are now gated on egui's own shapes, which are what the app actually asks for.
A picture of the program is not the program.

## Asking the compiler, and the difference between reparsing and drawing

2026-08-01. The library is finished by putting values to the one thing that can
actually judge them.

**`check_expr` says a value reparses. That is a different claim from "this can be
drawn with", and the gap is exactly the size of an import.** A value lilook built
itself is made of colours the user picked and is applied to the document as it is
saved, so the ordinary compile already answers for it. A value out of someone
else's file has never been near a compiler, and `(1, 2, 3)` and
`their_document_accent` both reparse perfectly and draw nothing.

So an imported palette or colour map is now staged rather than admitted: put to
the compiler one at a time through the query slot that already exists, and let
into the library only when it comes back clean. Refused ones are named. The test
is deliberately a **blacklist** -- a non-empty list of things that are not
numbers or words -- because wrongly refusing a value someone really uses is worse
than letting an odd one through: the odd one fails visibly at the first figure
that draws it, and the refusal fails invisibly forever.

**A theme is admitted without asking, and that is not a shortcut.** Typst
resolves what is inside a closure when it is *called*, so evaluating a theme's
definition proves nothing about its body. The document that uses it answers for
it, at the compile that follows. Checking it would be theatre.

**And the colormap editor's last dishonesty is gone.** `new…` on
`color.map.viridis` seeded from lilook's five-stop preview sketch, which is not
viridis. It now asks the compiler and opens when the answer arrives. The
surprise: typst's own maps are **nine** stops, not the 256 this file previously
assumed -- so the answer is the map itself, exactly, with nothing thinned out,
and it is small enough to edit by hand. The preview strips keep the compiled-in
sketch, because a menu draws its ramps before anything has been asked.

The shape of that fix is worth keeping: the honest way out of an approximation is
usually to ask whoever owns the real answer, and the reason not to is usually an
assumption about cost that nobody has checked.

## A short shelf, and why the maps are not all named

2026-08-01. Curated palettes and colour maps, offered beside typst's and lilaq's.

**Kept short on purpose.** A menu of forty maps is a decision the reader has to
make forty times, and the fortieth is not better than viridis. What was added
fills gaps rather than lengthening the list: two more colourblind-safe palettes
(Tol muted for many series, Tol high-contrast for three), and four maps typst
does not carry -- cividis, and three diverging maps with a neutral middle, which
is the shape an anomaly or a difference wants and the one typst's own set is
thinnest on.

**Two kinds of entry in one table, deliberately.** A map typst names stays a
name: `color.map.viridis` is the map, and spelling out its nine stops here would
be a copy that drifts the moment typst revises one. A map typst does not have is
written out as colours, because there is nothing to name it by. So `COLORMAPS`
carries an *expression* per entry, the same shape `CYCLES` has always had, and
the menu decides how to draw a row by whether it can read colours out of it.

That difference has a consequence worth stating: the curated maps are lilook's
own data, so a mistyped stop is lilook's bug and nobody else's. They are gated
twice -- every entry renders a mesh through real typst, and the diverging one is
asserted to actually be blue at one end, near-white in the middle and red at the
other, from the shapes the menu paints.

The attribution belongs in the hover, where the person choosing can see it:
ColorBrewer for the diverging three, Okabe--Ito and Paul Tol for the palettes.

**On evidence, again.** The screenshot pipeline in this session mangles
gradients -- it rotates colour channels, which is how a correct preview came to
look wrong twice. Colour is now checked against egui's shapes, and pictures are
kept for questions about *layout*, which they answer honestly.
