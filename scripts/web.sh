#!/usr/bin/env bash
# Build the browser bundle, and serve it unless told not to.
#
#   scripts/web.sh                  debug build, serve on :8787
#   scripts/web.sh release          optimised build (much smaller, much faster)
#   scripts/web.sh release --no-serve   just build `site/`, for CI
#
# The output directory `site/` is what GitHub Pages publishes: index.html plus
# the wasm-bindgen output. Nothing else is needed -- the packages and fonts are
# inside the binary.
set -euo pipefail
cd "$(dirname "$0")/.."

# `release` here means the `wasm-release` profile: fat LTO, one codegen unit,
# `panic = "abort"` and stripped symbols. An ordinary release build is 50 MB,
# which an iPhone will not load.
PROFILE="${1:-debug}"
SERVE=1
for a in "$@"; do [ "$a" = "--no-serve" ] && SERVE=0; done

FLAGS=()
DIR="debug"
if [ "$PROFILE" = "release" ]; then
  FLAGS+=(--profile wasm-release)
  DIR="wasm-release"
fi

# A local build uses the typst package cache; CI points at a fetched one.
if [ -z "${TYPST_PACKAGE_CACHE_PATH:-}" ] && [ -d .packages ]; then
  export TYPST_PACKAGE_CACHE_PATH="$PWD/.packages"
fi

cargo build --target wasm32-unknown-unknown -p lilook-web "${FLAGS[@]}"

rm -rf site
mkdir -p site
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir site/pkg \
  "target/wasm32-unknown-unknown/$DIR/lilook_web.wasm"

# Binaryen squeezes out what rustc leaves behind. Optional: the build works
# without it, just larger.
# `-all` because wasm-bindgen emits reference types and bulk memory, which
# binaryen rejects unless told they are allowed. A failure here is not fatal:
# the build works without it, just larger.
if command -v wasm-opt >/dev/null; then
  echo "wasm-opt…"
  if wasm-opt -all -Oz --strip-debug --strip-producers \
      site/pkg/lilook_web_bg.wasm -o site/pkg/lilook_web_bg.opt.wasm 2>/dev/null; then
    mv site/pkg/lilook_web_bg.opt.wasm site/pkg/lilook_web_bg.wasm
  else
    echo "wasm-opt declined this module; shipping it unoptimised" >&2
    rm -f site/pkg/lilook_web_bg.opt.wasm
  fi
fi

# The loader shows a progress bar, and needs the uncompressed size to do it: a
# compressing server reports the compressed length, but the stream the page
# reads is decompressed.
BYTES=$(wc -c < site/pkg/lilook_web_bg.wasm | tr -d ' ')
sed "s/__WASM_BYTES__/$BYTES/" crates/lilook-web/index.html > site/index.html

# Pages serves .wasm with the right type already; this is for the local server,
# which does not, and for anyone copying the directory somewhere plainer.
printf 'application/wasm wasm\n' > site/.mime.types

echo "wasm: $(du -h site/pkg/*.wasm | cut -f1)  (about $(gzip -9 -c site/pkg/*.wasm | wc -c | awk '{printf "%.0f", $1/1048576}') MB over the wire)"

if [ "$SERVE" = "1" ]; then
  echo "serving http://0.0.0.0:8787/  — reachable from another device on this network"
  cd site && python3 -m http.server 8787 --bind 0.0.0.0
fi
