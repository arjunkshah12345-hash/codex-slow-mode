#!/bin/sh
# Install this tree's Codex binary as `codex-slow`.
# Does not replace an existing `codex` on PATH.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
DEST="${CODEX_SLOW_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$DEST"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build codex-slow" >&2
  exit 1
fi

echo "Building codex from $ROOT"
echo "The installed command will be $DEST/codex-slow"
echo "An existing codex binary is not modified."

(
  cd "$ROOT/codex-rs"
  cargo build -p codex-cli --release --bin codex
)

cp "$ROOT/codex-rs/target/release/codex" "$DEST/codex-slow"
chmod +x "$DEST/codex-slow"

echo
echo "Installed $DEST/codex-slow"
echo "Add $DEST to PATH if it is not already there, then run:"
echo "  codex-slow"
echo "  /slow-mode"
