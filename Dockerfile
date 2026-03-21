# Multi-stage Dockerfile for the RustyTalon agent (cloud deployment).
#
# Build:
#   docker build --platform linux/amd64 -t rustytalon:latest .
#
# Run:
#   docker run --env-file .env -p 3001:3001 rustytalon:latest

# Stage 1: Install cargo-chef
FROM rust:1.92-slim-bookworm AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev cmake gcc g++ \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked

WORKDIR /app

# Stage 2: Build WASM channels (pre-compiled for Docker deployments)
FROM rust:1.92-slim-bookworm AS channels-builder

RUN rustup target add wasm32-wasip2 && \
    cargo install wasm-tools --locked

WORKDIR /channels
COPY channels-src/ .

# Build each channel; failures are non-fatal so a broken channel doesn't block the image.
RUN for dir in discord telegram slack matrix; do \
      if [ -f "$dir/build.sh" ]; then \
        echo "=== Building $dir channel ===" && \
        (cd "$dir" && bash build.sh) || echo "Warning: $dir build failed, skipping"; \
      fi; \
    done

# Stage 3: Compute the dependency recipe
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY examples/ examples/
COPY migrations/ migrations/
COPY wit/ wit/

RUN cargo chef prepare --recipe-path recipe.json

# Stage 4: Build dependencies (cached layer)
FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json

# Build only dependencies — this layer is cached unless Cargo.toml/Cargo.lock changes
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source and build the binary
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY examples/ examples/
COPY migrations/ migrations/
COPY wit/ wit/

RUN cargo build --release --bin rustytalon

# Stage 5: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/rustytalon /usr/local/bin/rustytalon
COPY --from=builder /app/migrations /app/migrations

# Non-root user
RUN useradd -m -u 1000 -s /bin/bash rustytalon

# Pre-install built WASM channels into the default channels directory.
# Users can configure them immediately via the web UI without any CLI steps.
COPY --from=channels-builder /channels /channels-built
RUN mkdir -p /home/rustytalon/.rustytalon/channels && \
    for dir in discord telegram slack matrix; do \
      wasm="/channels-built/$dir/$dir.wasm" && \
      cap="/channels-built/$dir/$dir.capabilities.json" && \
      if [ -f "$wasm" ] && [ -f "$cap" ]; then \
        cp "$wasm" "$cap" /home/rustytalon/.rustytalon/channels/ && \
        echo "Installed: $dir channel"; \
      fi; \
    done && \
    chown -R rustytalon:rustytalon /home/rustytalon/.rustytalon && \
    rm -rf /channels-built

USER rustytalon

EXPOSE 3001

# Sensible defaults for Docker deployments.
# All of these can be overridden via environment variables or --env-file.
ENV RUST_LOG=rustytalon=info \
    # Use embedded SQLite — no external database required.
    DATABASE_BACKEND=libsql \
    # Enable the web UI on all interfaces inside the container.
    GATEWAY_ENABLED=true \
    GATEWAY_HOST=0.0.0.0 \
    GATEWAY_PORT=3001

ENTRYPOINT ["rustytalon"]
