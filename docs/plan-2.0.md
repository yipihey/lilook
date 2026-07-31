# lilook 2.0 — a language server for figures, in egui first

Supersedes nothing. `docs/plan.md` §1 (the settled ADRs) and §5 (standalone)
still hold unchanged; `docs/plan-1.0.md` is complete and shipped. This plan adds
one thing to what exists, and it is a *framing* rather than an architecture:

> lilook already is a language server for figures. It syncs a document
> incrementally, publishes diagnostics, answers "go to definition", and serves an
> outline — over JSON-RPC, to four frontends. What it does not yet do is the half
> of a language server that *helps you write*: tell you what can go here, show
> you what a value resolved to, and offer to fix what broke.

The goal is a specific product: **a source pane that teaches lilaq while you use
it**, so that the usual misery of learning a plotting library — reading docs in
another window to find the argument you want, and staring at an error that names
no location — does not happen in lilook.

Nothing here requires a refactor. The Session extraction (`a84f576`) already put
the seam where this needs it: every capability below is *computed in the core as
data* and *rendered by a frontend*. egui is the first renderer; Swift and MCP get
each capability for free as it lands.

---

## 1. What measurement says

Two things were measured before this plan was written, because both could have
invalidated it. One did change the design.

### Diagnostic ranges are unusable, in two different ways

`world.rs:193` resolves a diagnostic's span to a byte range only when the span
belongs to the main file. Six representative errors, through the path the app
actually uses (`render_scenes`, so with probes injected):

| what the user wrote | range reported | where it points |
| --- | --- | --- |
| `bogus: 1` — unknown argument | **none** | — |
| `width: "wide"` — wrong type | **none** | — |
| `xlim: ()` | **none** | — |
| `yscale: "log", ylim: (-1, 10)` | **none** | — |
| `lq.plot(nope, ..)` — undefined name | `781..785` | **past the end of a 200-byte file** |
| unclosed delimiter | `772..773` | **past the end** |

**Finding A: most lilaq errors carry no location at all.** lilaq validates
through `elembic`, inside the package, so `span.id()` is the package's file and
not the user's. Four of the six most common failures — including every one in the
quick-fix catalogue this plan is built around — have no span to follow.

**Finding B: the two that do have spans point into the *derived* buffer.** The
probe injects `lq.place(..)` markers into the diagram's argument list, so every
offset after the first diagram is shifted by the length of what was inserted. A
200-byte file reports an error at byte 781. This is a live defect: any UI that
tried to highlight it today would slice out of bounds.

**What that changes.** Code actions cannot be driven off diagnostic ranges, and
should not be. They are driven off **(message, document)** instead: lilook knows,
without any span, which call has `xlim` set to `()`, and which diagram has a log
scale with a non-positive limit. That is more robust than span-following anyway —
it survives lilaq moving its assertions — and it plays to the thing lilook has
that a text-only server does not: a parsed document *and* a recovered scene.

Finding B still gets fixed, because two error classes do carry spans and a wrong
byte range is worse than none. `Injection` already records every insertion, so
the mapping back is a subtraction.

### egui can already do the pane

`TextEdit::layouter` takes `FnMut(&Ui, &dyn TextBuffer, f32) -> Arc<Galley>`, and
`TextEditOutput` returns the `galley`. So per-range colouring needs no new widget,
and a completion popup is an `Area` placed from the galley's cursor rectangle.

This is the measurement that shrank the plan. The "real editor pane" was the long
pole; it is now a spike (M3) rather than a milestone, and if the spike fails the
plan degrades to hints-in-the-margin rather than collapsing.

---

## 2. The principle

**A capability is a pure function of `(document, schema, scene)` returning data.
It never renders, and it never edits.**

Already true of everything in `Session`; stating it is what keeps it true as the
surface grows. The consequences are the reason to bother:

- Swift gets each capability by calling a function, not by reimplementing it.
- The MCP server gets each capability as a tool. `lilook_actions` falls straight
  out of M4, which means **agents get quick-fixes too** — an outcome nobody
  planned and worth protecting.
- Each capability is testable without a display, like everything else here.

The corollary, equally binding: **capabilities are advisory**. A completion list
is a suggestion, an inlay hint is a readout, a code action is an offer. None of
them may edit the buffer on their own. The user's document changes when the user
says so.

---

## 3. What is being added

Two axes of addressing, where today there is one.

lilook is **node-addressed**: `selected: usize` is a call-site id, and every
existing operation takes one. A language server is **position-addressed**: every
question is asked at a cursor offset. Both must exist, and the second is new:

```rust
/// What is at this byte offset: the call it is inside, the argument, the slot.
pub fn at(&self, offset: usize) -> Cursor
```

`Document::calls()` already carries ranges, so this is a lookup rather than a
parse. Designing it once, up front, is why it is M2 and not a detail of M4.

Everything else is a function over `Cursor`, the schema, and the scene:

| capability | signature (all on `Session`) | needs |
| --- | --- | --- |
| inlay hints | `hints() -> Vec<Hint>` | scene + argument ranges |
| code actions | `actions() -> Vec<Action>` | diagnostics + document |
| completion | `completions(Cursor) -> Vec<Completion>` | schema + policy |
| signature help | `signature(Cursor) -> Option<Signature>` | schema |

`Hint`, `Action`, `Completion` and `Signature` are plain data in `lilook-core`,
serde-derived, like `Scene` and `Diagnostic` before them.

---

## 4. Milestones

Ordered so that each one is useful alone, and so the riskiest unknown is proved
before anything depends on it.

### M1 — Diagnostic ranges tell the truth

Map a span through `Injection` back to the user's buffer, and drop the range
rather than report a wrong one when the mapping is ambiguous. Both retry paths
(`inject_with(.., false)`) map too.

*Exit:* the six-case table in §1 re-run as a test — the two spanned errors point
at the bytes the user actually wrote, and no range ever exceeds `doc.text().len()`.
A property test over the gesture corpus asserts the same invariant after arbitrary
edits.

### M2 — `Session::at(offset)`

The position axis. `Cursor { call, argument, slot, in_string }` — enough for
completion to know whether it is offering a parameter name, a value, or nothing.

The source pane starts reporting its cursor, which it does not today.

*Exit:* a table test over a fixture with nested calls; every byte offset in the
file resolves to the call a human would name, including inside nested
`lq.diagram(lq.plot(..))` and inside string literals.

### M3 — Layouter spike: semantic colour

The one unknown left. Build a `LayoutJob` from `Document::classify()` and prove
egui renders it inside a live `TextEdit` without breaking editing, selection or
undo.

Deliberately the *cheapest* consumer of the layouter, so that if it fails the
plan learns that here rather than in M5.

*Exit:* the source pane colours literals, bindings and generated regions
differently, typing still works, and a headless test asserts the `LayoutJob`'s
sections match the classification. Plus one delight, if it is free: each
`lq.plot(..)` tinted with the colour it actually draws, from the cycle.

### M4 — Inlay hints

The capability that exists *only* because of the probe, and the best one-screen
argument that lilook is not a text editor with a linter:

```typst
#lq.diagram(xlim: auto,   ⟨0.82 … 4.18⟩
```

Sources: `auto` limits resolved by the scene, a linked slot's row count, and a
`z` field's grid shape. Rendered in the margin if inline insertion proves hard —
the value is in the number, not its placement.

*Exit:* a fixture with `xlim: auto` reports a hint whose text matches the
recovered transform to six figures; hints move correctly after an edit; zero
hints on a figure that has not compiled.

### M5 — Code actions

Driven by `(message, document)`, per §1. The starting catalogue, every entry of
which this project has already hit:

| diagnostic | offer |
| --- | --- |
| `value must be strictly positive` + a log axis | switch that axis to linear · clamp the limit to the positive data |
| `Limit arrays must contain exactly two items` | fill from the data range |
| `unknown named` on an element | the nearest parameter by edit distance |
| a linked file that is missing | relink · unlock and embed |
| `schoolbook` with a log axis | drop the theme · drop the log scale |

Each action is a label plus an `Intent`, so applying one is an ordinary undoable
edit and nothing new enters the history.

*Exit:* each row reproduced as a fixture; the offered action applied; the figure
compiles afterwards; and the whole thing undoes byte-for-byte. Plus
`lilook_actions` as an MCP tool in the same commit, because it costs nothing once
the function exists.

### M6 — Completion and signature help

At a `Cursor`: parameter names for the call, values for the parameter, and the
policy's safe seed as the preselected entry. Signature help is the same data
rendered as one line.

The schema already knows all of it; this is the frontend finally asking.

*Exit:* inside `lq.plot(` the list is `plot`'s parameters and nothing else; after
`map:` it is the thirteen colormaps; after `yscale:` the scale names; and every
value offered is asserted to compile, exactly as the colormap and cycle tables
already are.

---

## 5. Decisions taken

**LSP the shape, not LSP the protocol.** lilook already speaks JSON-RPC through
MCP. Adopting the LSP spec would buy nothing and cost line/character positions in
UTF-16 code units.

**No dependency on `impress-helix`.** It is Helix-*style* modal editing over a
`HelixTextEngine` trait — editing semantics, no rendering, which is the half not
needed — and `plan.md` §5 makes standalone-ness mechanically enforced. The
*boundary* it draws is right and worth copying if modal editing is ever wanted;
that is a v2.1 preference, not a v2 dependency.

**Capabilities never edit.** §2. A code action is an offer.

**egui first, generalise on demand.** Each capability returns data, so a second
renderer costs a renderer. Writing an abstraction for a second frontend that does
not exist yet would be guessing.

---

## 6. Risks

**The layouter may fight `TextEdit`.** M3 exists to find out early. Fallback:
hints and diagnostics in the margin, colour deferred. The plan survives.

**Completion inside a live compile loop may feel slow.** Completion must not
require a compile — it is schema plus parse, both already incremental. If it ever
waits on the compiler, that is the bug.

**`(message, document)` matching is string matching against another project's
error text.** lilaq will reword something eventually. Mitigation: every rule is a
fixture that compiles the failing document, so a reworded message shows up as a
failing test rather than a quietly missing action.

**Scope.** Six milestones is a release, not an afternoon. M1 and M2 are days; M3
is a spike; M4–M6 are days each. If it has to stop early, it should stop after
M4: hints alone justify the framing.

---

## 7. What this is not

- **Not a text editor.** No multi-file, no workspace symbols, no formatting, no
  modal editing. The source pane serves the figure.
- **Not data analysis.** Fitting, binning and histograms stay upstream — see the
  standing scope decision. lilook makes figures excellent; it does not massage
  data.
- **Not a rewrite.** Every milestone is an addition to `Session` and a renderer
  in `lilook-ui`. If a milestone starts wanting to move something, that is a
  signal to stop and re-read this section.
