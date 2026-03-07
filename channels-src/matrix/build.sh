#!/usr/bin/env bash
# Build the Matrix channel WASM component
#
# Prerequisites:
#   - Rust with wasm32-wasip2 target: rustup target add wasm32-wasip2
#   - wasm-tools for component creation: cargo install wasm-tools
#
# Output:
#   - matrix.wasm - WASM component ready for deployment
#   - matrix.capabilities.json - Capabilities file (copy alongside .wasm)

set -euo pipefail

cd "$(dirname "$0")"

echo "Building Matrix channel WASM component..."

# Build the WASM module
cargo build --release --target wasm32-wasip2

WASM_PATH="target/wasm32-wasip2/release/matrix_channel.wasm"

if [ -f "$WASM_PATH" ]; then
    # Wrap as a WASM component (no-op if already a component)
    wasm-tools component new "$WASM_PATH" -o matrix.wasm 2>/dev/null || cp "$WASM_PATH" matrix.wasm

    # Strip debug symbols to reduce size
    wasm-tools strip matrix.wasm -o matrix.wasm

    echo "Built: matrix.wasm ($(du -h matrix.wasm | cut -f1))"
    echo ""
    echo "To install:"
    echo "  mkdir -p ~/.rustytalon/channels"
    echo "  cp matrix.wasm matrix.capabilities.json ~/.rustytalon/channels/"
    echo ""
    echo "Then configure your access token:"
    echo "  rustytalon tool auth matrix"
    echo ""
    echo "And set your homeserver + owner ID in the config:"
    echo "  # In matrix.capabilities.json, set:"
    echo "  #   config.homeserver  — e.g. \"https://matrix.org\""
    echo "  #   config.owner_id    — e.g. \"@you:matrix.org\""
else
    echo "Error: WASM output not found at $WASM_PATH"
    exit 1
fi
