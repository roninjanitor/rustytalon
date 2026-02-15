#!/usr/bin/env bash
# Build RustyTalon and all bundled WASM channels.
#
# Run this before release or when channel/tool sources have changed.
# The main binary bundles Telegram WASM module via build.rs; it must exist.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Building bundled WASM channels..."
if [ -d "tools-src/telegram" ]; then
    echo "    Building Telegram WASM channel..."
    cargo build --manifest-path tools-src/telegram/Cargo.toml --target wasm32-wasip2 --release
fi

echo ""
echo "==> Building RustyTalon binary..."
cargo build --release

echo ""
echo "✓ Done. Binaries:"
echo "  - Main agent: target/release/rustytalon"
echo ""
echo "To build Docker images:"
echo "  docker build -f Dockerfile -t rustytalon:latest ."
echo "  docker build -f Dockerfile.worker -t rustytalon-worker:latest ."
echo ""
echo "Or use: make docker-build-all"
