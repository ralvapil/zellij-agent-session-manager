#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ZELLIJ_CONFIG_DIR=${ZELLIJ_CONFIG_DIR:-"$HOME/.config/zellij"}
OPENCODE_CONFIG_DIR=${OPENCODE_CONFIG_DIR:-"$HOME/.config/opencode"}

"$ROOT/scripts/build.sh"

mkdir -p "$ZELLIJ_CONFIG_DIR/plugins" "$ZELLIJ_CONFIG_DIR/layouts" "$OPENCODE_CONFIG_DIR/plugins"

cp "$ROOT/dist/zellij-agent-session-manager.wasm" "$ZELLIJ_CONFIG_DIR/plugins/"
cp "$ROOT/dist/zellij-agent-session-manager-store.wasm" "$ZELLIJ_CONFIG_DIR/plugins/"
cp "$ROOT/zellij/layouts/agent-session-manager.kdl" "$ZELLIJ_CONFIG_DIR/layouts/"
cp "$ROOT/opencode/plugins/zellij-sidebar-alerts.js" "$OPENCODE_CONFIG_DIR/plugins/"

cat <<EOF
Installed files.

Add the snippets from:
  $ROOT/zellij/snippets/plugins.kdl
  $ROOT/zellij/snippets/keybinds.kdl

Then set or use layout:
  agent-session-manager

Restart Zellij and OpenCode after updating config.
EOF
