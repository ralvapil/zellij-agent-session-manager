#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PLUGIN_DIR="$ROOT/plugin"
OUT_DIR="$ROOT/dist"

mkdir -p "$OUT_DIR"

cd "$PLUGIN_DIR"
cargo build --release --target wasm32-wasip1
cp target/wasm32-wasip1/release/zellij-agent-session-manager.wasm \
  "$OUT_DIR/zellij-agent-session-manager.wasm"

cargo build --release --target wasm32-wasip1 --features store-default
cp target/wasm32-wasip1/release/zellij-agent-session-manager.wasm \
  "$OUT_DIR/zellij-agent-session-manager-store.wasm"

printf 'Built:\n  %s\n  %s\n' \
  "$OUT_DIR/zellij-agent-session-manager.wasm" \
  "$OUT_DIR/zellij-agent-session-manager-store.wasm"
