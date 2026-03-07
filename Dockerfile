# Multi-stage Dockerfile for the RustyTalon agent (cloud deployment).
#
# Build:
#   docker build --platform linux/amd64 -t rustytalon:latest .
#
# Run:
#   docker run --env-file .env -p 3000:3000 rustytalon:latest

# Stage 1: Install cargo-chef
FROM rust:1.92-slim-bookworm AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev cmake gcc g++ \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked

WORKDIR /app

# Stage 2: Compute the dependency recipe
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY examples/ examples/
COPY migrations/ migrations/
COPY wit/ wit/

RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Build dependencies (cached layer)
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

# Stage 4: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/rustytalon /usr/local/bin/rustytalon
COPY --from=builder /app/migrations /app/migrations

# Non-root user
RUN useradd -m -u 1000 -s /bin/bash rustytalon
USER rustytalon

EXPOSE 3000

ENV RUST_LOG=rustytalon=info

ENTRYPOINT ["rustytalon"]
