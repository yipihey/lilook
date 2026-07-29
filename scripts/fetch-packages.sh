#!/usr/bin/env bash
# Populate a typst package cache without running typst.
#
# `crates/lilook-web/build.rs` bakes the lilaq package tree into the wasm
# binary, and reads it from the local typst cache. A CI runner has no such
# cache, so this fetches the same files from Typst Universe into a directory
# that `TYPST_PACKAGE_CACHE_PATH` can point at.
#
#   scripts/fetch-packages.sh [dir]     # default: .packages
set -euo pipefail
cd "$(dirname "$0")/.."

DIR="${1:-.packages}"

# lilaq 0.6.0 and everything it imports. Kept in step with build.rs, which
# fails loudly naming any package it cannot find.
PACKAGES=(
  "lilaq 0.6.0"
  "elembic 1.1.1"
  "zero 0.6.1"
  "tiptoe 0.4.0"
)

for entry in "${PACKAGES[@]}"; do
  set -- $entry
  name="$1" version="$2"
  target="$DIR/preview/$name/$version"
  if [ -f "$target/typst.toml" ]; then
    echo "have $name:$version"
    continue
  fi
  echo "fetching $name:$version"
  mkdir -p "$target"
  curl -sSfL "https://packages.typst.org/preview/$name-$version.tar.gz" \
    | tar -xz -C "$target"
done

echo "package cache ready: $DIR"
echo "build with: TYPST_PACKAGE_CACHE_PATH=$(cd "$DIR" && pwd) cargo build ..."
