#!/usr/bin/env bash
# Points the Homebrew tap's formula at a new lilook-app version.
#
# The formula builds from the crates.io source tarball, so the only things
# that change per release are the url and its sha256 -- both derived here,
# never hand-typed.
#
# Usage: scripts/bump-homebrew-tap.sh <version> <tap-checkout-dir>
set -euo pipefail

version="$1"
tap_dir="$2"
formula="$tap_dir/Formula/lilook.rb"
url="https://static.crates.io/crates/lilook-app/lilook-app-${version}.crate"

[ -f "$formula" ] || { echo "no formula at $formula" >&2; exit 1; }

# crates.io's static CDN usually has a freshly published tarball within
# seconds, but this runs right after the publish job, so retry rather than
# race it.
sha=""
for _ in 1 2 3 4 5 6; do
  if body=$(curl -sfL "$url"); then
    sha=$(printf '%s' "$body" | shasum -a 256 | cut -d' ' -f1)
    break
  fi
  sleep 10
done
[ -n "$sha" ] || { echo "could not fetch $url" >&2; exit 1; }

sed -i.bak \
  -e "s#url \".*lilook-app-.*\.crate\"#url \"$url\"#" \
  -e "s#sha256 \".*\"#sha256 \"$sha\"#" \
  "$formula"
rm -f "$formula.bak"

echo "bumped $formula to $version ($sha)"
