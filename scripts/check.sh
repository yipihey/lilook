#!/usr/bin/env bash
# The full gate: formatting, lints, tests, plus an end-to-end check that edited
# output still compiles. The trailing-comma insertion bug passed the round-trip
# test and was caught only by the last step.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --bin lilook

TYPST="${TYPST:-typst}"
if ! command -v "$TYPST" >/dev/null; then
  echo "typst not on PATH; skipping the CLI recompile gate" >&2
  exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cat > "$TMP/f.typ" <<'EOF'
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 5pt)
#let xs = lq.linspace(0, 10)
#lq.diagram(
  width: 6cm, height: 4cm,
  lq.plot(xs, xs.map(x => calc.sin(x)), stroke: red),
)
EOF
./target/debug/lilook set "$TMP/f.typ" 2 stroke "blue"
./target/debug/lilook add "$TMP/f.typ" 1 xlabel "[Time]"
"$TYPST" compile "$TMP/f.typ" "$TMP/f.svg" --format svg
echo "edited output recompiles cleanly"

# The GUI's own recompile gate lives in `lilook-compile`'s test suite, which
# compiles in process: `tests/gestures.rs` drives a pan and a point drag through
# the document and recompiles the result. Nothing here needs a display.

# The browser path. `lilook-web`'s own tests run natively -- they drive the
# browser app through a real egui context and compile every gallery example
# against the bundled packages -- so the only thing a browser adds is pixels.
# The browser path, checked here and built by `scripts/web.sh`: `lilook-core` and `lilook-ui` port
# unchanged, and `lilook-compile` without its `system` feature carries the whole
# in-process backend. What is missing is a shell and a package bundle, not an
# architecture -- see `docs/findings.md`.
if rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
  cargo check --quiet --target wasm32-unknown-unknown -p lilook-core -p lilook-ui -p lilook-editor
  cargo check --quiet --target wasm32-unknown-unknown -p lilook-compile --no-default-features
  cargo check --quiet --target wasm32-unknown-unknown -p lilook-web
  echo "wasm32 targets check cleanly"
fi
