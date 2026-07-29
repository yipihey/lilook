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

PROFILE="${1:-debug}"
SERVE=1
for a in "$@"; do [ "$a" = "--no-serve" ] && SERVE=0; done

FLAGS=()
[ "$PROFILE" = "release" ] && FLAGS+=(--release)

# A local build uses the typst package cache; CI points at a fetched one.
if [ -z "${TYPST_PACKAGE_CACHE_PATH:-}" ] && [ -d .packages ]; then
  export TYPST_PACKAGE_CACHE_PATH="$PWD/.packages"
fi

cargo build --target wasm32-unknown-unknown -p lilook-web "${FLAGS[@]}"

rm -rf site
mkdir -p site
cp crates/lilook-web/index.html site/
wasm-bindgen \
  --target web \
  --no-typescript \
  --out-dir site/pkg \
  "target/wasm32-unknown-unknown/$PROFILE/lilook_web.wasm"

# Pages serves .wasm with the right type already; this is for the local server,
# which does not, and for anyone copying the directory somewhere plainer.
printf 'application/wasm wasm\n' > site/.mime.types

echo "wasm: $(du -h site/pkg/*.wasm | cut -f1)  (about $(gzip -9 -c site/pkg/*.wasm | wc -c | awk '{printf "%.0f", $1/1048576}') MB over the wire)"

if [ "$SERVE" = "1" ]; then
  echo "serving http://0.0.0.0:8787/  — reachable from another device on this network"
  cd site && python3 -m http.server 8787 --bind 0.0.0.0
fi
