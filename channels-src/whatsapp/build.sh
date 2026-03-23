#!/usr/bin/env bash
# Build the WhatsApp channel WASM component
#
# Prerequisites:
#   - Rust with wasm32-wasip2 target: rustup target add wasm32-wasip2
#   - wasm-tools for component creation: cargo install wasm-tools
#
# Output:
#   - whatsapp.wasm - WASM component ready for deployment
#   - whatsapp.capabilities.json - Capabilities file (copy alongside .wasm)

set -euo pipefail

cd "$(dirname "$0")"

echo "Building WhatsApp channel WASM component..."

# Build the WASM module
cargo build --release --target wasm32-wasip2

WASM_PATH="target/wasm32-wasip2/release/whatsapp_channel.wasm"

if [ -f "$WASM_PATH" ]; then
    # Create component if needed
    wasm-tools component new "$WASM_PATH" -o whatsapp.wasm 2>/dev/null || cp "$WASM_PATH" whatsapp.wasm

    # Strip debug symbols to reduce size
    wasm-tools strip whatsapp.wasm -o whatsapp.wasm

    echo "Built: whatsapp.wasm ($(du -h whatsapp.wasm | cut -f1))"
    echo ""
    echo "To install:"
    echo "  mkdir -p ~/.rustytalon/channels"
    echo "  cp whatsapp.wasm whatsapp.capabilities.json ~/.rustytalon/channels/"
    echo ""
    echo "Then add your WhatsApp Cloud API credentials:"
    echo "  rustytalon tool auth whatsapp"
else
    echo "Error: WASM output not found at $WASM_PATH"
    exit 1
fi
