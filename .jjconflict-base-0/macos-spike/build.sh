#!/usr/bin/env bash
# Build + run the qwertty-term FFI Swift driver.
#
# NOT an Xcode project -- one cargo build + one swiftc invocation. This is the
# whole ceremony to consume the C ABI from Swift, which is itself a spike
# finding (see docs/analysis/ffi-spike.md).
#
# Usage: macos-spike/build.sh   (run from anywhere; paths are repo-relative)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILE="${PROFILE:-debug}"

echo "==> Building qwertty-term-ffi staticlib ($PROFILE)"
if [ "$PROFILE" = "release" ]; then
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p qwertty-term-ffi --release
else
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p qwertty-term-ffi
fi

LIB_DIR="$REPO_ROOT/target/$PROFILE"
OUT="$SCRIPT_DIR/qwertty-term-spike"

echo "==> Compiling Swift driver"
# -I imports the clang module map (CQwerttyTerm -> the cbindgen header).
# The Rust staticlib needs the system libs it depends on; on macOS a Rust
# staticlib pulls in libSystem + libc++ + libresolv via these link flags.
swiftc \
  -o "$OUT" \
  -I "$SCRIPT_DIR" \
  -L "$LIB_DIR" \
  -lqwertty_term_ffi \
  -framework CoreFoundation \
  "$SCRIPT_DIR/main.swift"

echo "==> Running Swift driver"
echo "-----------------------------------------------------------------------"
"$OUT"
