#!/usr/bin/env bash
# Build and test the Swift package, and optionally the iOS XCFramework.
#
#   scripts/swift.sh          build + test against the host library
#   scripts/swift.sh --ios    also build Lilook.xcframework for device + simulator
#
# The Swift package consumes the same C ABI as the Python binding. Until now it
# had never been compiled -- there was no toolchain where it was written -- so
# this exists to keep that from being true again.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v swift >/dev/null; then
  echo "no swift toolchain; skipping" >&2
  exit 0
fi

cargo build -p lilook-ffi
(cd swift && swift build -Xlinker -L../target/debug && swift test -Xlinker -L../target/debug)

[ "${1:-}" = "--ios" ] || exit 0

# iOS: a static library per slice, then one XCFramework over both. The headers
# travel with each slice, module map included, so Swift can import CLilook.
OUT="target/ios"
rm -rf "$OUT" && mkdir -p "$OUT"
for triple in aarch64-apple-ios aarch64-apple-ios-sim; do
  rustup target add "$triple" >/dev/null 2>&1 || true
  cargo build -p lilook-ffi --target "$triple" --release
  mkdir -p "$OUT/$triple/Headers"
  cp crates/lilook-ffi/include/lilook.h "$OUT/$triple/Headers/"
  cp swift/Sources/CLilook/module.modulemap "$OUT/$triple/Headers/"
  cp "target/$triple/release/liblilook_ffi.a" "$OUT/$triple/"
done

xcodebuild -create-xcframework \
  -library "$OUT/aarch64-apple-ios/liblilook_ffi.a" \
  -headers "$OUT/aarch64-apple-ios/Headers" \
  -library "$OUT/aarch64-apple-ios-sim/liblilook_ffi.a" \
  -headers "$OUT/aarch64-apple-ios-sim/Headers" \
  -output "$OUT/Lilook.xcframework" > /dev/null

echo "built $OUT/Lilook.xcframework ($(ls "$OUT/Lilook.xcframework" | tr '\n' ' '))"
