#!/usr/bin/env bash
# Build the Discord channel WASM component
#
# Prerequisites:
#   - Rust with wasm32-wasip2 target: rustup target add wasm32-wasip2
#   - wasm-tools for component creation: cargo install wasm-tools
#
# Output:
#   - discord.wasm - WASM component ready for deployment
#   - discord.capabilities.json - Capabilities file (copy alongside .wasm)

set -euo pipefail

cd "$(dirname "$0")"

echo "Building Discord channel WASM component..."

# Build the WASM module
cargo build --release --target wasm32-wasip2

WASM_PATH="target/wasm32-wasip2/release/discord_channel.wasm"

if [ -f "$WASM_PATH" ]; then
    # Create component if needed
    wasm-tools component new "$WASM_PATH" -o discord.wasm 2>/dev/null || cp "$WASM_PATH" discord.wasm

    # Strip debug symbols to reduce size
    wasm-tools strip discord.wasm -o discord.wasm

    echo "Built: discord.wasm ($(du -h discord.wasm | cut -f1))"
    echo ""
    echo "To install:"
    echo "  mkdir -p ~/.rustytalon/channels"
    echo "  cp discord.wasm discord.capabilities.json ~/.rustytalon/channels/"
    echo ""
    echo "Then configure your bot token:"
    echo "  rustytalon tool auth discord"
    echo ""
    echo "And set your Discord user ID in the config:"
    echo "  # In discord.capabilities.json, set config.owner_id to your Discord user ID"
    echo "  # Right-click your username in Discord → Copy User ID (enable Developer Mode first)"
else
    echo "Error: WASM output not found at $WASM_PATH"
    exit 1
fi
