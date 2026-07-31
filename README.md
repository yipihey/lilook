# lilook.rs

A GUI editor for [lilaq](https://lilaq.org) figures in [Typst](https://typst.app)
manuscripts.

**The Typst source is the document model.** There is no `.vsz`-style intermediate
format. Every GUI action becomes a surgical byte-range replacement on the user's
`.typ` file, and undo is a text-edit history rather than a widget-tree history.
This is the load-bearing decision; read `docs/plan.md` §1 before changing
anything that touches it.

lilook is deliberately standalone. It knows nothing about the impress suite,
implore, or any host application — see `docs/plan.md` §5.

---

## What it is

**A language server for figures.** lilook syncs a document incrementally,
publishes diagnostics, answers go-to-definition and serves an outline — over
JSON-RPC, to four frontends — and it does the half that helps you write: what can
go here, what a value resolved to, and an offer to fix what broke.

Two things follow from that framing, and they are the reason the codebase looks
the way it does.

**Every capability is a pure function of `(document, schema, scene)` returning
data.** It never renders and it never edits. So a desktop window, a browser tab,
a SwiftUI view and an MCP server for agents all get each capability from one
place — and when lilaq adds a function, no frontend changes.

**It has two surfaces, not one.** A language server maps position to meaning.
lilook maps position ⟷ meaning ⟷ *geometry*: clicking a curve selects a byte
range, and editing a byte range moves a curve. That round trip is what the probe
technique (ADR-0008) exists for, and it is the part no language server has.

## Status

Editing: click, pan, zoom, drag points, resize by the frame — every gesture a
surgical edit and one undo step. Every lilaq function and style element has a
control, including pickers for colormaps and colourblind-safe palettes. Themes
switch, fork and rename. Data links live to CSV, JSON, HDF5, npz, FITS and
Veusz's descriptor ASCII, refreshing when the file changes. Export to PDF, SVG
and PNG.

Language-server capabilities: semantic colour, inlay hints for what `auto`
resolved to, completion and signature help, and quick fixes. Errors that name no
location — most of lilaq's, since it validates inside its own package — are
located by recompiling variants at ~4 ms each, which produces the byte range the
diagnostic was missing.

229 tests. `scripts/check.sh` is the gate.

| crate | what it is | GUI |
| --- | --- | --- |
| `lilook-core` | document, intents, history, scene, schema, policy, session, capabilities | — |
| `lilook-compile` | in-process typst: World, probes, actor, export, blame | — |
| `lilook-data` | npz, FITS, HDF5, Veusz ASCII, CBOR sidecars | — |
| `lilook-ffi` | C ABI for Swift / Python / Julia | — |
| `lilook-ui` | egui inspector, canvas, highlighting | egui |
| `lilook-editor` | panels and layout | egui |
| `lilook-app` / `lilook-web` | desktop and browser shells | egui |
| `lilook-compile/bin/lilook-mcp` | MCP server: five tools, hierarchical discovery | — |

About 70% of the code has no GUI dependency. `docs/plan-2.0.md` has the
architecture; `docs/plan.md` §1 has the settled ADRs.

Read `docs/findings.md` for what was actually measured versus assumed. Several
early assumptions were overturned by measurement; the document records both.

---

## Build and test

Requires a Rust toolchain (1.92+, for `typst` 0.15). Typst versions are pinned
once in the workspace manifest so `typst-syntax` cannot drift from the parser
`typst` links.

```sh
scripts/check.sh               # the full gate: fmt, clippy, tests, recompile
cargo test                     # everything
cargo test -p lilook-core      # document model, history, scene maths
cargo test -p lilook-compile   # in-process typst, probes, gestures end to end
cargo test -p lilook-ui        # inspector and canvas, headless, no display
cargo run  -p lilook-app -- figure.typ
```

Nothing needs a `typst` binary any more: the backend compiles in process. Tests
that need the real lilaq package skip themselves if it is neither cached nor
downloadable.

### Looking at it without a display

```sh
cargo run -p lilook-app -- figure.typ --select 2 --screenshot out.png
```

Draws until the first compile lands, writes a PNG of the window and exits. This
exists because "it renders correctly" was unverifiable for a whole phase.

### For an agent

```sh
cargo run -p lilook-compile --bin lilook-mcp -- /path/to/project
```

An MCP server over stdio: five tools, not one per lilaq function. That shape was
measured — a tool per function costs ~18,000 tokens on *every* request, because
tool definitions are re-sent each turn; a terse capability index costs 810 once
and describing a single function 47. So discovery is hierarchical, and an agent
spends about a thousand tokens learning what it needs instead of eighteen.

It reports what actually happened, not just that something failed:

```
ERROR: unknown named field 'widht'
  caused by `widht: 3cm` on #0 (bytes 118..121)
  fix: rename `widht` to `width` — the nearest parameter this call takes
```

### In the browser

```sh
scripts/web.sh release      # build the wasm bundle and serve it
```

It prints a LAN address, so a phone on the same network can open it. The server
gzips (`scripts/serve.py`), because uncompressed the module is 27 MB and
compressed it is 10.5 -- and a phone notices the difference. Verified working on
iOS Safari: touch, pinch and drag all reach the same editor the desktop uses.

Opens a gallery of lilaq's own documentation examples -- the stacked area chart
among them -- each editable by pointer *and* by text, with typst compiling in
the page. Measured there: 3-16 ms per recompile on these figures.

The first load is about 12 MB gzipped, because the page carries a whole
typesetter: 10.5 MB of wasm and 1.6 MB of fonts, cached separately. After that
nothing goes over the network, and nothing is uploaded -- a figure edited in the
browser never leaves it. `docs/findings.md` records where the weight is: a
PDF interpreter, a WebAssembly interpreter and two copies of a rasteriser, all
arriving unconditionally with typst, none of which a figure uses.

### Keys

`⌘Z` / `⇧⌘Z` undo and redo · `⌘S` save · `⌘0` fit · `⌘C` copy the selection
with the bindings it needs · `⌘V` paste into the selected figure · `⌘D`
duplicate · `⌫` delete. Drag inside a diagram to pan the data, scroll to zoom
it, drag a point of a selected literal series to move it, and drag the right or
bottom edge of the axis frame to resize the figure -- `width` and `height` *are*
the data area's dimensions, so the frame follows the pointer exactly.

### Swift and iOS

```sh
scripts/swift.sh            # build the package and run its tests
scripts/swift.sh --ios      # also build target/ios/Lilook.xcframework
```

The package wraps the same C ABI as the Python binding. It had never been
compiled before -- there was no toolchain where it was written -- and what
review had missed was uniform: every `LilookDoc *` imports as an
`OpaquePointer`, so the pointer conversions around the handle were all wrong.

### CLI

```sh
cargo run --bin lilook -- inspect figure.typ
cargo run --bin lilook -- schema lq.plot
cargo run --bin lilook -- set figure.typ 2 stroke "blue.darken(20%)"
```

### MCP server

Newline-delimited JSON-RPC over stdio; tool descriptions are generated from the
same schema the inspector consumes.

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run --bin lilook-mcp
```

### Python binding

```sh
cargo build
LILOOK_LIB=target/debug/liblilook_ffi.so python3 -c "
import sys; sys.path.insert(0,'bindings/python'); import lilook
d = lilook.Document(open('figure.typ').read())
print([(c['node'], c['callee']) for c in d.calls])"
```

---

## Regenerating the schema

`assets/lilaq-0.6.0.schema.json` is generated from a pinned lilaq checkout and
committed. Do not hand-edit it.

```sh
scripts/bootstrap.sh          # clones lilaq + its docs-site, regenerates
```

The generator reuses **lilaq's own** tidy doc-comment parser from the docs-site
repo rather than reimplementing it. Curation of wide type unions lives in
`tools/extract_schema.py`, deliberately outside the emitted JSON, so
regeneration never clobbers it.

---

## Where to start

1. `docs/plan-1.0.md` — the current plan: architecture, the interaction model,
   milestones with their exit criteria, and what is still open.
2. `docs/findings.md` — what was measured, in order. Schema coverage, the
   performance envelope, transform accuracy, and the in-process numbers.
3. `AGENTS.md` — invariants that must not be broken, and the specific mistakes
   already made once.
4. `crates/lilook-core/src/doc.rs` — the document model.
5. `crates/lilook-compile/src/probe.rs` — how pixels get back to byte ranges.

`docs/plan.md` is the earlier plan; §1 (the ADRs) still holds, §3 is superseded.

The next piece of work is **the M9 web shell** — everything under it already
compiles for wasm32 — and then whatever a first real manuscript makes obvious.

---

## License

MIT.
