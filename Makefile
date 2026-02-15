.PHONY: help build test lint docker-build docker-build-agent docker-build-worker docker-build-all docker-up docker-down docker-logs docker-clean ship fmt clippy clean

help:
	@echo "RustyTalon Development Makefile"
	@echo ""
	@echo "Build targets:"
	@echo "  make build              - Build release binary"
	@echo "  make build-all          - Build channels and binary (wrapper for scripts/build-all.sh)"
	@echo ""
	@echo "Code quality:"
	@echo "  make fmt                - Format code with cargo fmt"
	@echo "  make clippy             - Run clippy linter"
	@echo "  make test               - Run all tests"
	@echo "  make lint               - Run fmt --check and clippy"
	@echo ""
	@echo "Quality gate (before shipping):"
	@echo "  make ship               - Run fmt, clippy, and tests (full quality gate)"
	@echo ""
	@echo "Docker targets:"
	@echo "  make docker-build-agent     - Build agent Docker image"
	@echo "  make docker-build-worker    - Build worker Docker image"
	@echo "  make docker-build-all       - Build both agent and worker images"
	@echo "  make docker-up              - Start services with docker-compose"
	@echo "  make docker-down            - Stop and remove containers"
	@echo "  make docker-logs-agent      - View agent logs"
	@echo "  make docker-logs-db         - View database logs"
	@echo "  make docker-logs            - View all logs (follow mode)"
	@echo "  make docker-clean           - Remove images and volumes"
	@echo "  make docker-shell           - Open shell in agent container"
	@echo ""
	@echo "Cleanup:"
	@echo "  make clean              - Remove build artifacts"

# ============================================================================
# Build Targets
# ============================================================================

build:
	cargo build --release

build-all:
	bash scripts/build-all.sh

# ============================================================================
# Code Quality Targets
# ============================================================================

fmt:
	cargo fmt

clippy:
	cargo clippy --all --benches --tests --examples --all-features -- -D warnings

test:
	cargo test

lint: fmt-check clippy
	@echo "✓ Linting passed"

fmt-check:
	cargo fmt -- --check

ship: fmt clippy test
	@echo "✓ Quality gate passed - ready to ship!"

# ============================================================================
# Docker Build Targets
# ============================================================================

docker-build-agent:
	docker build -f Dockerfile -t rustytalon:latest -t rustytalon:dev .
	@echo "✓ Agent image built: rustytalon:latest"

docker-build-worker:
	docker build -f Dockerfile.worker -t rustytalon-worker:latest -t rustytalon-worker:dev .
	@echo "✓ Worker image built: rustytalon-worker:latest"

docker-build-all: docker-build-agent docker-build-worker
	@echo "✓ All Docker images built"

# ============================================================================
# Docker Compose Targets
# ============================================================================

docker-up:
	docker-compose -f docker-compose.yml up -d
	@echo "✓ Services started"
	@echo ""
	@echo "Available services:"
	@echo "  - PostgreSQL: localhost:5432 (postgres/postgres)"
	@echo "  - RustyTalon: http://localhost:3000 (when agent runs)"
	@echo ""
	@echo "View logs with: make docker-logs"

docker-down:
	docker-compose -f docker-compose.yml down
	@echo "✓ Services stopped"

docker-logs:
	docker-compose -f docker-compose.yml logs -f

docker-logs-agent:
	docker-compose -f docker-compose.yml logs -f rustytalon

docker-logs-db:
	docker-compose -f docker-compose.yml logs -f db

docker-shell:
	docker-compose -f docker-compose.yml exec -it rustytalon /bin/bash

docker-clean:
	docker-compose -f docker-compose.yml down -v
	docker rmi rustytalon:latest rustytalon:dev 2>/dev/null || true
	docker rmi rustytalon-worker:latest rustytalon-worker:dev 2>/dev/null || true
	@echo "✓ Docker images and volumes cleaned"

# ============================================================================
# Cleanup
# ============================================================================

clean:
	cargo clean
	rm -rf target/
	@echo "✓ Build artifacts removed"

# ============================================================================
# Helper Targets
# ============================================================================

version:
	@cargo pkgid | cut -d'@' -f2

.PHONY: docker-logs docker-logs-agent docker-logs-db docker-shell version
