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

# `${FLAGS[@]+..}` rather than `"${FLAGS[@]}"`: under `set -u`, bash 3.2 -- which
# is what macOS ships -- treats an empty array expansion as an unbound variable,
# so a debug build died with "FLAGS[@]: unbound variable" before compiling
# anything.
cargo build --target wasm32-unknown-unknown -p lilook-web ${FLAGS[@]+"${FLAGS[@]}"}

rm -rf site
mkdir -p site
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir site/pkg \
  "target/wasm32-unknown-unknown/$DIR/lilook_web.wasm"

# Does the module the browser will fetch actually load?
#
# The validator is the real engine rather than a second opinion: node is V8, the
# same parser Chrome and Safari use. This exists because `wasm-opt` accepting its
# own output proved nothing -- on 2026-07-30 the deployed module was written by a
# binaryen that could still read it back and rejected by every browser with
# "unknown type form: 0 @+152", and nothing in this script noticed.
validate() {
  if ! command -v node >/dev/null; then
    echo "note: no node, so $1 goes out unvalidated" >&2
    return 0
  fi
  node -e '
    const bytes = require("fs").readFileSync(process.argv[1]);
    try {
      new WebAssembly.Module(bytes);
    } catch (e) {
      console.error("  " + e.message);
      process.exit(1);
    }
  ' "$1"
}

# Binaryen squeezes out what rustc leaves behind. Optional: the build works
# without it, just larger.
#
# The features are listed rather than using `-all`. `-all` tells binaryen every
# proposal is permitted, which lets it emit types a browser may not accept -- and
# an Ubuntu-packaged binaryen did exactly that, writing a GC `structref` into the
# type section of a module no browser would load. Measured, the explicit list
# costs 10 KB gzipped out of 11 MB, so the whole class of failure goes away for
# nothing. Anything added here should be a feature wasm-bindgen or rustc actually
# emits.
WASM_FEATURES=(
  --enable-reference-types
  --enable-bulk-memory
  --enable-bulk-memory-opt
  --enable-sign-ext
  --enable-mutable-globals
  --enable-nontrapping-float-to-int
  --enable-multivalue
  --enable-simd
  --enable-extended-const
)
if command -v wasm-opt >/dev/null; then
  echo "wasm-opt… ($(wasm-opt --version))"
  # stderr is *not* discarded: hiding it is why the last failure was silent.
  if wasm-opt "${WASM_FEATURES[@]}" -Oz --strip-debug --strip-producers \
      site/pkg/lilook_web_bg.wasm -o site/pkg/lilook_web_bg.opt.wasm; then
    if validate site/pkg/lilook_web_bg.opt.wasm; then
      mv site/pkg/lilook_web_bg.opt.wasm site/pkg/lilook_web_bg.wasm
    else
      echo "wasm-opt wrote a module V8 will not load; shipping it unoptimised" >&2
      rm -f site/pkg/lilook_web_bg.opt.wasm
    fi
  else
    echo "wasm-opt declined this module; shipping it unoptimised" >&2
    rm -f site/pkg/lilook_web_bg.opt.wasm
  fi
fi

# Whichever path got here, the thing that will be deployed has to load. Fatal,
# because a site that cannot start is worse than a build that fails.
if ! validate site/pkg/lilook_web_bg.wasm; then
  echo "FATAL: site/pkg/lilook_web_bg.wasm will not load in V8; refusing to ship it" >&2
  exit 1
fi

# The fonts a lilaq figure needs, copied out of typst-assets rather than
# embedded: see `crates/lilook-web/Cargo.toml`. The path comes from cargo so
# this works on a CI runner that has only just fetched the crate.
ASSETS=$(cargo metadata --format-version 1 | python3 -c "
import json, os, sys
meta = json.load(sys.stdin)
path = next(p['manifest_path'] for p in meta['packages'] if p['name'] == 'typst-assets')
print(os.path.join(os.path.dirname(path), 'files', 'fonts'))
")
mkdir -p site/pkg/fonts
for f in LibertinusSerif-Regular.otf LibertinusSerif-Italic.otf \
         LibertinusSerif-Bold.otf NewCMMath-Book.otf; do
  cp "$ASSETS/$f" site/pkg/fonts/
done
echo "fonts: $(du -sh site/pkg/fonts | cut -f1)"

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
  IP=$(ipconfig getifaddr en0 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')
  echo "serving http://${IP:-0.0.0.0}:8787/  — open that on a phone on this network"
  exec python3 scripts/serve.py 8787
fi
