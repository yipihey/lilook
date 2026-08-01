#!/usr/bin/env bash
# The order crates must be published in, derived from the manifests.
#
# crates.io indexes each crate before the next can verify against it, so the
# order is not optional. It is derived rather than written down because writing
# it down got it wrong: `lilook-ui` is a *dev*-dependency of `lilook-compile`,
# `cargo publish` verifies those too, and a hand-written order put them the wrong
# way round and failed mid-publish.
#
#   scripts/publish-order.sh          # print the order
#   scripts/publish-order.sh --run    # publish, stopping at the first failure
set -euo pipefail
cd "$(dirname "$0")/.."

order() {
  python3 - <<'PY'
import glob, tomllib
deps = {}
for p in glob.glob('crates/*/Cargo.toml'):
    d = tomllib.load(open(p, 'rb'))
    if d['package'].get('publish') is False:
        continue
    inner = set()
    for section in ('dependencies', 'dev-dependencies', 'build-dependencies'):
        inner |= {k for k in d.get(section, {}) if k.startswith('lilook-')}
    deps[d['package']['name']] = inner
out, seen = [], set()
def visit(n):
    if n in seen:
        return
    seen.add(n)
    for d in sorted(deps.get(n, ())):
        if d in deps:
            visit(d)
    out.append(n)
for n in sorted(deps):
    visit(n)
print(" ".join(out))
PY
}

CRATES=$(order)
if [ "${1:-}" != "--run" ]; then
  echo "$CRATES"
  exit 0
fi

for c in $CRATES; do
  if cargo search "$c" 2>/dev/null | grep -q "^$c = \"$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)\""; then
    echo "=== $c already published, skipping ==="
    continue
  fi
  echo "=== $c ==="
  cargo publish -p "$c"
  # The index needs a moment before the next crate can verify against this one.
  sleep 30
done
