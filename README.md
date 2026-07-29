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

## Status

The figure renders, and you can click it, pan it, zoom it and drag its points;
every gesture lands as a surgical edit in the `.typ` file and is one undo step.
Every parameter in the lilaq schema has a control, document-level `lq.set-*`
styling is editable from its own panel, and series copy and paste between
figures carrying the bindings they need. 90 tests green.

| crate | what it is | state |
| --- | --- | --- |
| `lilook-core` | document, intents, history, scene, schema | working, 40 tests |
| `lilook-compile` | in-process typst: World, compile actor, probes, raster | working, 21 tests |
| `lilook-ui` | egui-only inspector and canvas | working, 29 tests |
| `lilook-editor` | the editor itself: panels, gestures, transactions | working |
| `lilook-app` | desktop shell: window, file, compile thread | working, renders (see `--screenshot`) |
| `lilook-web` | browser shell: gallery of lilaq examples, wasm | working, 4 tests |
| `lilook-ffi` | C ABI for Swift / Python / Julia | working |
| `bindings/python` | ctypes wrapper | working |
| `swift/` | SwiftUI package | **never compiled — no toolchain was available** |

Done: M0–M10 of `docs/plan-1.0.md`, browser build included. The same editor runs
in a window and in a page; only the shell differs. Not done: the resize gesture,
and anything Swift.

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
