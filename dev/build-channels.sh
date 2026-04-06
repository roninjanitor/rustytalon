#!/usr/bin/env bash
# Build all WASM channels and copy them into dev/channels/ for local dev.
#
# Prerequisites:
#   rustup target add wasm32-wasip2
#   cargo install wasm-tools
#
# Usage:
#   ./dev/build-channels.sh           # build all channels
#   ./dev/build-channels.sh discord   # build one channel
#
# After running, restart the dev container to pick up changes:
#   docker compose -f docker-compose.dev.yml restart rustytalon

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$SCRIPT_DIR/channels"

CHANNELS=("discord" "telegram" "slack" "matrix" "whatsapp")

# If arguments provided, build only those channels
if [ $# -gt 0 ]; then
    CHANNELS=("$@")
fi

mkdir -p "$OUT_DIR"

for channel in "${CHANNELS[@]}"; do
    src="$REPO_ROOT/channels-src/$channel"
    if [ ! -f "$src/build.sh" ]; then
        echo "Skipping $channel (no build.sh found)"
        continue
    fi

    echo "=== Building $channel ==="
    (cd "$src" && bash build.sh)

    wasm="$src/$channel.wasm"
    cap="$src/$channel.capabilities.json"

    if [ -f "$wasm" ] && [ -f "$cap" ]; then
        cp "$wasm" "$cap" "$OUT_DIR/"
        echo "Copied $channel.wasm + $channel.capabilities.json → dev/channels/"
    else
        echo "Warning: expected $wasm or $cap not found after build"
    fi
done

echo ""
echo "Done. Restart the dev container to apply:"
echo "  docker compose -f docker-compose.dev.yml restart rustytalon"
