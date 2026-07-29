# lilook.rs — finalized plan

Supersedes the earlier phase sketch. Everything below is either settled by
measurement (Phases 0–4, see the findings report) or explicitly named as an open
decision. Nothing here rests on general reputation or on my prior guesses, two of
which the work overturned.

---

## 1. Decisions now settled by evidence

These are ADR-ready. Each states what was measured, so a future reader can see
why it is not merely a preference.

**ADR-0007 — Typst source is the model; no intermediate format.**
`typst-syntax` round-trips byte-identically; surgical named-argument replacement
preserves comments and irregular whitespace and reparses clean; random sequences
of intents, fully undone, restore the buffer byte-for-byte across 39 seeds. The
one rule that makes this hold: never regenerate the file from a model, only
rewrite ranges.

**ADR-0008 — Series identity is geometric, not span-based.**
Exported SVG carries no span attributes, and lilaq constructs its drawing
primitives inside its own functions (`place(curve(..segments))`), so any span on
a rendered curve resolves into lilaq's source rather than the user's call site.
Instead, probes placed via `lq.place` at known data coordinates recover the
data↔page transform to 0.007 units on a 300-unit axis, at ~2% compile overhead.
Hit-testing happens in data space with tolerance in page points.
*This reverses my earlier claim that marker emission was load-bearing.* Markers
are still needed for non-data furniture (ticks, spines, legend entries).

**ADR-0009 — Fine-grained intents with an explicit transaction API.**
The core exposes per-event intents; transactions decide what becomes one undo
step. Validated at four consumers — Rust, CLI, MCP, Python FFI — where a
multi-step drag collapses to a single undo entry and restores exactly. The
coarse wrappers each open and commit one transaction per call.

**ADR-0010 — Schema is generated, curation is separate.**
Mechanical type→widget mapping left 45% of parameters unusable; type-family
coalescing took it to 30%; fifteen hand-curated union signatures took it to 7.8%
of 409. Curation lives in the generator, never in the emitted JSON, so
regeneration cannot clobber it.

**ADR-0011 — UI logic depends on `egui` only; `eframe` is confined to a shell.**
`lilook-ui` runs headlessly under `egui::__run_test_ui` with no display and no
extra dependencies. This is what makes the frontend agent-testable, and it is
also what lets a Swift frontend consume the same `UiEvent` vocabulary instead of
reimplementing interaction policy.

**ADR-0012 — One C ABI for every non-Rust consumer.**
Swift, Python and Julia go through the same header. Intents cross as JSON, so
the ABI does not change when the vocabulary grows.

---

## 2. Two decisions still needed from you

### 2.1 What "mature" means for lilook

You named lilook the first package to bring to maturity for implore, which only
means something with a definition of done. Proposed:

**In scope for 1.0**
- Every lilaq function's literal-valued named arguments editable.
- Element configuration via `lq.set-*` working (see 2.2).
- Sub-100 ms preview on a 5k-point figure.
- Copy/paste of series and of styling between series.
- Round-trip safety property-tested: lilook never corrupts a hand-written file.
- Runs on macOS/Linux desktop **and** in the browser via WASM.

**Explicitly out of scope for 1.0**
- Swift/iOS — Phase 4 exists but is unverified; treat it as post-1.0.
- Non-linear scales beyond log for the probe path.
- Any 3D or volume rendering — that stays with Makie.

### 2.2 Set-rule scoping — the real remaining design question

The schema already carries all 99 fields across 17 elembic elements, but the
document model does not touch set rules, and this is not merely unimplemented
work. `#lq.set-tick(...)` is a *show rule*: it applies from where it appears to
the end of its enclosing scope. An inspector panel that says "this diagram's
ticks" does not map cleanly onto that.

Three options, and they are genuinely different products:

1. **Edit the nearest enclosing set rule.** Honest to Typst's semantics; means
   changing one figure can silently restyle later ones. Needs a clear indication
   of scope in the UI.
2. **Insert a scoped set rule inside the figure's own block.** Predictable per
   figure; adds `#{ ... }` wrapping to the user's source, which is a visible
   change to how their manuscript reads.
3. **Prefer per-call arguments wherever lilaq offers one, and expose set rules
   only as a separate document-level panel.** Least surprising, but leaves some
   element fields reachable only from that panel.

My read is (3), with (2) available explicitly rather than by default — it keeps
the per-figure inspector honest about what it is editing. But this is a product
decision, not a technical one, and it changes what the inspector looks like.

---

## 3. Remaining work, ordered by risk retired per unit effort

**P5 — In-process compile backend.** The largest remaining unknown, and it gates
everything interactive. Currently shelling out at a ~570 ms floor, which is fine
for CLI and MCP and far too slow for drag-rate preview. Needs a `World` impl with
package resolution and font loading, plus comemo memoisation. Requires rustc
≥ 1.92 for current `typst`. *Exit criterion: <100 ms incremental recompile on a
5k-point figure.*

**P6 — Set rules.** Blocked on 2.2. Once decided, the schema work is already done.

**P7 — Copy/paste.** Clipboard payload is the Typst source of the copied subtree,
with a structured MIME alongside `text/plain`. The bulk of the work is
free-variable capture analysis on paste: a copied series may reference a `#let`,
a dataset or an import absent at the destination, and you must decide between
inlining, carrying bindings, and pasting-with-unresolved-marks.

**P8 — WASM.** No blockers known, entirely untested. `lilook-ui` should port
directly. The virtual-FS shim needed here is the same abstraction implore needs
to back figures with its own storage — build it once, deliberately.

**P9 — Swift/iOS.** Post-1.0. Build the XCFramework for `aarch64-apple-ios` plus
the simulator triple and actually run `LilookTests`, which has never executed.

**Continuous — the data path.** Decimation with a hard threshold around 5k points
before the emitter, because the measured envelope is 5k ≈ 1 s and 50 k ≈ 8 s for
lines, worse with marks.

---

## 4. Open risks, with triggers

| risk | trigger to watch | response |
| --- | --- | --- |
| rustc version treadmill — `typst-syntax` 0.15 already needs 1.92 | any typst bump | pin the toolchain in CI; treat typst upgrades as scheduled work |
| lilaq API drift | schema regeneration diff | CI job that **fails** on new or changed parameters, forcing a conscious mapping |
| probe path on `symlog` / datetime scales | first user with one | untested; add to the compile-service test matrix before 1.0 |
| in-process `World` complexity in WASM | P8 | fonts and packages must be bundled; no filesystem |
| generality claim untested | P8 and implore adoption | CLI and MCP already exercise the boundary; treat awkwardness there as a design signal, not plumbing |

---

## 5. The implore adaptation contract

lilook stays standalone. implore consumes it. The seams that make that possible
without a fork are two traits:

- **Workspace/filesystem** — needed anyway for WASM, and what lets implore back
  figures with its own storage.
- **Data resolver** — "where does this dataset come from" must never be
  hardcoded to local paths.

Enforcement is mechanical: separate workspace, zero impress crates in
`Cargo.toml`, CI check. But the leak that actually happens is vocabulary. If
lilook's core types start carrying item IDs, journal hooks or overlay-item
concepts, you are coupled with no Cargo edge to point at. Standing test: could
someone with no knowledge of impress use lilook and never notice something is
missing?

---

## 6. What is already done

Four Rust crates, a Swift package, a Python binding; 2,823 lines; 16 tests green.
Verified: schema extraction, CST round-trip and surgical edits, the undo
invariant under random intent sequences, transaction coalescing at four
consumers, transform recovery and hit-testing against the real compiler,
headless inspector rendering, and that edited output recompiles.

Unverified: anything Swift, visual correctness of the egui app, the in-process
compile backend, set-rule editing, copy/paste, and WASM.
