#!/usr/bin/env bash
# Clone the pinned lilaq checkout and its docs-site (which carries lilaq's own
# tidy doc-comment parser), then regenerate the schema.
set -euo pipefail
cd "$(dirname "$0")/.."

LILAQ_TAG="${LILAQ_TAG:-v0.6.0}"
VENDOR="${VENDOR:-.vendor}"
mkdir -p "$VENDOR"

if [ ! -d "$VENDOR/lilaq" ]; then
  git clone --depth 1 --branch "$LILAQ_TAG" \
    https://github.com/lilaq-project/lilaq.git "$VENDOR/lilaq" 2>/dev/null \
  || git clone --depth 1 https://github.com/lilaq-project/lilaq.git "$VENDOR/lilaq"
fi
if [ ! -d "$VENDOR/docsite" ]; then
  git clone --depth 1 \
    https://github.com/lilaq-project/lilaq-project.github.io.git "$VENDOR/docsite"
fi

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$VENDOR/lilaq/typst.toml" | head -1)
python3 tools/extract_schema.py \
  "$VENDOR/lilaq/src" "$VENDOR/docsite/scripts" \
  "crates/lilook-core/assets/lilaq-${VERSION}.schema.json"

echo
echo "Regenerated crates/lilook-core/assets/lilaq-${VERSION}.schema.json"
echo "If the version changed, update the include! paths in:"
echo "  crates/lilook-core/src/bin/lilook.rs"
echo "  crates/lilook-core/src/bin/lilook-mcp.rs"
echo "  crates/lilook-core/tests/core.rs"
echo "  crates/lilook-ui/tests/inspector.rs"
echo "  crates/lilook-app/src/main.rs"
