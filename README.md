<h1 align="center">RustyTalon</h1>

<p align="center">
  <strong>Your secure personal AI assistant, always on your side</strong>
</p>

<p align="center">
  <a href="#philosophy">Philosophy</a> •
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#security">Security</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#api">API</a> •
  <a href="#deployment">Deployment</a>
</p>

---

> **Personal Project Notice**
>
> This is a personal fork maintained by [@roninjanitor](https://github.com/roninjanitor) to solve specific problems for my own use. Most changes were made with the help of [Claude Code](https://claude.ai/code) (Anthropic's AI coding assistant). The project works for me, but comes with no guarantees of stability or support.
>
> Contributions and issues are welcome — I'm happy to review PRs. Just know this is a nights-and-weekends project, not a product.
>
> **Original project:** [IronClaw](https://github.com/nearai/ironclaw) by [NEAR AI](https://near.ai) — all credit for the core architecture goes to the original maintainers.

---

## Philosophy

RustyTalon is built on a simple principle: **your AI assistant should work for you, not against you**.

In a world where AI systems are increasingly opaque about data handling and aligned with corporate interests, RustyTalon takes a different approach:

- **Your data stays yours** - All information is stored locally, encrypted, and never leaves your control
- **Transparency by design** - Open source, auditable, no hidden telemetry or data harvesting
- **No vendor lock-in** - Smart routing across Anthropic, OpenAI, Ollama, and any OpenAI-compatible provider
- **Self-expanding capabilities** - Build new tools on the fly without waiting for vendor updates
- **Defense in depth** - Multiple security layers protect against prompt injection and data exfiltration

RustyTalon is the AI assistant you can actually trust with your personal and professional life.

## Features

### Multi-Provider LLM Support

- **Smart Routing** - Automatically routes queries to the best provider based on complexity, cost, and quality
- **Provider Failover** - Automatic fallback when a provider is unhealthy
- **Cost Tracking** - Per-request cost recording and aggregate statistics
- **Supported Providers** - Anthropic (Claude), OpenAI (GPT), Ollama (local models), any OpenAI-compatible API

### Security First

- **WASM Sandbox** - Untrusted tools run in isolated WebAssembly containers with capability-based permissions
- **Credential Protection** - Secrets are never exposed to tools; injected at the host boundary with leak detection
- **Prompt Injection Defense** - Pattern detection, content sanitization, and policy enforcement
- **Endpoint Allowlisting** - HTTP requests only to explicitly approved hosts and paths

### Always Available

- **Multi-channel** - REPL, HTTP webhooks, WASM channels (Telegram, Slack, Discord), and web gateway
- **Docker Sandbox** - Isolated container execution with per-job tokens and orchestrator/worker pattern
- **Web Gateway** - Browser UI with real-time SSE/WebSocket streaming and 50+ API endpoints
- **Routines** - Cron schedules, event triggers, webhook handlers for background automation
- **Heartbeat System** - Proactive background execution for monitoring and maintenance tasks
- **Parallel Jobs** - Handle multiple requests concurrently with isolated contexts
- **Self-repair** - Automatic detection and recovery of stuck operations

### Self-Expanding

- **Dynamic Tool Building** - Describe what you need, and RustyTalon builds it as a WASM tool
- **MCP Protocol** - Connect to Model Context Protocol servers for additional capabilities
- **Plugin Architecture** - Drop in new WASM tools and channels without restarting

### Persistent Memory

- **Hybrid Search** - Full-text + vector search using Reciprocal Rank Fusion
- **Workspace Filesystem** - Flexible path-based storage for notes, logs, and context
- **Identity Files** - Maintain consistent personality and preferences across sessions

## Quick Start

### Prerequisites

- Rust 1.85+
- PostgreSQL 15+ with [pgvector](https://github.com/pgvector/pgvector) extension (or use libSQL for zero-dependency local mode)
- An API key for at least one LLM provider (Anthropic, OpenAI, or Ollama)

### Build from Source

```bash
git clone https://github.com/nicklozano/rustytalon.git
cd rustytalon

# Build with PostgreSQL backend (default)
cargo build --release

# Or build with embedded libSQL (no external database needed)
cargo build --release --no-default-features --features libsql

# Run tests
cargo test
```

### Database Setup

**Option A: PostgreSQL** (recommended for production)

```bash
createdb rustytalon
psql rustytalon -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

**Option B: libSQL** (zero-dependency, good for development)

No setup needed. RustyTalon creates `~/.rustytalon/rustytalon.db` automatically.

### Docker Quick Start

**Option 1: Pre-built Images (Fastest)**

Pull pre-built images from GitHub Container Registry:

```bash
# Configure your LLM provider first
cp .env.example .env
# Edit .env with your API key (see Configuration below)

# Start full stack with pre-built agent image
docker compose -f docker-compose.prod.yml up -d
```

**Option 2: Build from Source**

```bash
# Build all Docker images
make docker-build-all

# Or build individually
docker build -f Dockerfile -t rustytalon:latest .
docker build -f Dockerfile.worker -t rustytalon-worker:latest .

# Start development stack
docker compose up -d
```

**Option 3: Local Development (without Docker)**

```bash
# Start PostgreSQL container only
docker compose up -d db

# Configure and run locally
cp .env.example .env
cargo run
```

## Configuration

RustyTalon is configured via environment variables. Copy `.env.example` and set your values:

```bash
cp .env.example .env
```

### LLM Provider (pick one)

```bash
# Anthropic (default)
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-...
ANTHROPIC_MODEL=claude-sonnet-4-20250514

# OpenAI
LLM_BACKEND=openai
OPENAI_API_KEY=sk-...
OPENAI_MODEL=gpt-4o

# Ollama (local, free)
LLM_BACKEND=ollama
OLLAMA_BASE_URL=http://localhost:11434
OLLAMA_MODEL=llama3

# Any OpenAI-compatible API
LLM_BACKEND=openai_compatible
OPENAI_COMPATIBLE_BASE_URL=https://api.example.com/v1
OPENAI_COMPATIBLE_API_KEY=...
OPENAI_COMPATIBLE_MODEL=model-name
```

### Database

```bash
# PostgreSQL (default)
DATABASE_BACKEND=postgres
DATABASE_URL=postgres://user:pass@localhost:5432/rustytalon

# libSQL (embedded)
DATABASE_BACKEND=libsql
LIBSQL_PATH=~/.rustytalon/rustytalon.db

# libSQL with Turso cloud sync
LIBSQL_URL=libsql://your-db.turso.io
LIBSQL_AUTH_TOKEN=your-token
```

### Smart Routing

```bash
ROUTING_ENABLED=true
ROUTING_STRATEGY=balanced        # balanced, cost, quality, local_first
ROUTING_MAX_COST=0.05            # Max USD per request
ROUTING_MIN_QUALITY=0.5          # Min quality score 0.0-1.0
ROUTING_ENABLE_FALLBACK=true
ROUTING_MAX_RETRIES=3
ROUTING_PREFERRED_PROVIDERS=anthropic,openai
```

### Web Gateway

```bash
GATEWAY_ENABLED=true
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=3001
GATEWAY_AUTH_TOKEN=changeme      # Required for API access
```

### Embeddings (for semantic memory search)

```bash
OPENAI_API_KEY=sk-...
EMBEDDING_ENABLED=true
EMBEDDING_MODEL=text-embedding-3-small
```

### Docker Sandbox

```bash
SANDBOX_ENABLED=true
SANDBOX_IMAGE=rustytalon-worker:latest
SANDBOX_MEMORY_LIMIT_MB=512
SANDBOX_TIMEOUT_SECS=1800
```

See `.env.example` for the complete list of configuration options.

## Security

RustyTalon implements defense in depth to protect your data and prevent misuse.

### WASM Sandbox

All untrusted tools run in isolated WebAssembly containers:

- **Capability-based permissions** - Explicit opt-in for HTTP, secrets, tool invocation
- **Endpoint allowlisting** - HTTP requests only to approved hosts/paths
- **Credential injection** - Secrets injected at host boundary, never exposed to WASM code
- **Leak detection** - Scans requests and responses for secret exfiltration attempts
- **Rate limiting** - Per-tool request limits to prevent abuse
- **Resource limits** - Memory, CPU, and execution time constraints

```
WASM ──► Allowlist ──► Leak Scan ──► Credential ──► Execute ──► Leak Scan ──► WASM
         Validator     (request)     Injector       Request     (response)
```

### Prompt Injection Defense

External content passes through multiple security layers:

- Pattern-based detection of injection attempts
- Content sanitization and escaping
- Policy rules with severity levels (Block/Warn/Review/Sanitize)
- Tool output wrapping for safe LLM context injection

### Data Protection

- All data stored locally in your database
- Secrets encrypted with AES-256-GCM
- No telemetry, analytics, or data sharing
- Full audit log of all tool executions

## Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                          Channels                              │
│  ┌──────┐  ┌──────┐   ┌─────────────┐  ┌─────────────┐         │
│  │ REPL │  │ HTTP │   │WASM Channels│  │ Web Gateway │         │
│  └──┬───┘  └──┬───┘   └──────┬──────┘  │ (SSE + WS)  │         │
│     │         │              │         └──────┬──────┘         │
│     └─────────┴──────────────┴────────────────┘                │
│                              │                                 │
│                    ┌─────────▼─────────┐                       │
│                    │    Agent Loop     │  Intent routing       │
│                    └────┬──────────┬───┘                       │
│                         │          │                           │
│              ┌──────────▼────┐  ┌──▼───────────────┐           │
│              │  Scheduler    │  │ Routines Engine  │           │
│              │(parallel jobs)│  │(cron, event, wh) │           │
│              └──────┬────────┘  └────────┬─────────┘           │
│                     │                    │                     │
│       ┌─────────────┼────────────────────┘                     │
│       │             │                                          │
│   ┌───▼─────┐  ┌────▼────────────────┐                         │
│   │ Local   │  │    Orchestrator     │                         │
│   │Workers  │  │  ┌───────────────┐  │                         │
│   │(in-proc)│  │  │ Docker Sandbox│  │                         │
│   └───┬─────┘  │  │   Containers  │  │                         │
│       │        │  │ ┌───────────┐ │  │                         │
│       │        │  │ │Worker / CC│ │  │                         │
│       │        │  │ └───────────┘ │  │                         │
│       │        │  └───────────────┘  │                         │
│       │        └─────────┬───────────┘                         │
│       └──────────────────┤                                     │
│                          │                                     │
│              ┌───────────▼──────────┐                          │
│              │    Tool Registry     │                          │
│              │  Built-in, MCP, WASM │                          │
│              └──────────────────────┘                          │
│                          │                                     │
│              ┌───────────▼──────────┐                          │
│              │    Smart Router      │                          │
│              │ Anthropic│OpenAI│... │                          │
│              └──────────────────────┘                          │
└────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Purpose |
|-----------|---------|
| **Agent Loop** | Main message handling and job coordination |
| **Smart Router** | Routes queries to optimal LLM provider based on complexity and cost |
| **Scheduler** | Manages parallel job execution with priorities |
| **Worker** | Executes jobs with LLM reasoning and tool calls |
| **Orchestrator** | Container lifecycle, LLM proxying, per-job auth |
| **Web Gateway** | Browser UI with chat, memory, jobs, logs, extensions, routines |
| **Routines Engine** | Scheduled (cron) and reactive (event, webhook) background tasks |
| **Workspace** | Persistent memory with hybrid search |
| **Safety Layer** | Prompt injection defense and content sanitization |

## API

RustyTalon exposes a comprehensive HTTP API via the web gateway. All protected endpoints require a `Bearer` token set via `GATEWAY_AUTH_TOKEN`.

For the full API reference, see [docs/API.md](docs/API.md).

### Key Endpoint Groups

| Group | Endpoints | Description |
|-------|-----------|-------------|
| Chat | 9 | Send messages, SSE/WebSocket streaming, conversation history |
| Memory | 5 | Read, write, search workspace files |
| Jobs | 9 | List, cancel, restart sandbox jobs; read job files |
| Extensions | 5 | Install, activate, remove MCP/WASM extensions |
| Routines | 7 | CRUD for scheduled/reactive tasks |
| Settings | 6 | User settings management |
| Providers | 2 | LLM provider health and cost stats |
| OpenAI-compat | 2 | Drop-in `/v1/chat/completions` endpoint |

### Example: Send a Message

```bash
curl -X POST http://localhost:3001/api/chat/send \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"message": "What is the weather today?"}'
```

### Example: Stream Events (SSE)

```bash
curl -N http://localhost:3001/api/chat/events \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"
```

## Deployment

For production deployment guides, see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

### Docker Images

Pre-built images are automatically published to GitHub Container Registry on every release.

```bash
# Pull agent image
docker pull ghcr.io/nicklozano/rustytalon:latest

# Pull worker image (for Docker sandbox execution)
docker pull ghcr.io/nicklozano/rustytalon-worker:latest

# Run agent
docker run -it \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -e DATABASE_URL=postgresql://user:pass@host/rustytalon \
  ghcr.io/nicklozano/rustytalon:latest
```

**Image Tags:**
- `latest` - Latest release (tracking main branch)
- `vX.Y.Z` - Semantic version releases
- `sha-<commit>` - Commit-specific builds

**Building Locally:**

```bash
# Build both images
make docker-build-all

# Or individual builds
docker build -f Dockerfile -t rustytalon:latest .
docker build -f Dockerfile.worker -t rustytalon-worker:latest .
```

### Docker Compose (Recommended for Full Stack)

```bash
# Development stack with PostgreSQL
docker compose up -d

# Production stack with pre-built images
docker compose -f docker-compose.prod.yml up -d
```

### Standalone Binary

```bash
cargo build --release
./target/release/rustytalon
```

### With Docker Sandbox

When `SANDBOX_ENABLED=true`, RustyTalon will spawn isolated container jobs using the worker image:

```bash
# Ensure worker image is available (auto-pulled if missing)
docker pull ghcr.io/nicklozano/rustytalon-worker:latest

# Or build locally
docker build -f Dockerfile.worker -t rustytalon-worker:latest .
```

## Development

We provide a Makefile with convenient commands. Run `make help` to see all targets.

### Code Quality

```bash
# Format code
make fmt

# Lint with clippy
make clippy

# Run all tests
make test

# Full quality gate (fmt + clippy + test)
make ship
```

### Docker Development

```bash
# Build both agent and worker images
make docker-build-all

# Start dev stack (PostgreSQL + agent)
make docker-up

# View logs
make docker-logs

# Stop containers
make docker-down

# Clean images and volumes
make docker-clean
```

### Manual Commands

```bash
# Format code
cargo fmt

# Lint
cargo clippy --all --benches --tests --examples --all-features

# Run tests
cargo test

# Run specific test
cargo test test_name

# Verbose logging
RUST_LOG=rustytalon=debug cargo run
```

## Attribution

RustyTalon is derived from [IronClaw](https://github.com/nearai/ironclaw) (Copyright 2024 NEAR AI), an excellent open source AI assistant. We've preserved IronClaw's robust architecture (WASM sandbox, safety layers, database abstraction, workspace system) while removing vendor lock-in and adding multi-provider LLM support.

For detailed attribution, see [NOTICE](NOTICE).

**Original Project:** https://github.com/nearai/ironclaw

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

RustyTalon is released under the same licenses as IronClaw (MIT or Apache 2.0).
