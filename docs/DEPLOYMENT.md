# RustyTalon Deployment Guide

This guide covers deploying RustyTalon in various configurations, from local development to production.

## Quick Reference

| Deployment | Database | Best For |
|-----------|----------|----------|
| [Local binary](#local-binary) | libSQL (embedded) | Development, personal use |
| [Local binary + PostgreSQL](#local-binary-with-postgresql) | PostgreSQL | Personal use with full features |
| [Docker Compose](#docker-compose) | PostgreSQL (containerized) | Self-hosted production |
| [With Docker Sandbox](#enabling-docker-sandbox) | PostgreSQL | Production with isolated job execution |

---

## Local Binary

The simplest deployment -- a single binary with an embedded database.

### Build

```bash
# Build with embedded libSQL (no external database)
cargo build --release --no-default-features --features libsql
```

### Configure

```bash
cp .env.example .env
```

Edit `.env` with minimum required settings:

```bash
DATABASE_BACKEND=libsql
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-your-key-here
ANTHROPIC_MODEL=claude-sonnet-4-20250514
GATEWAY_AUTH_TOKEN=a-strong-random-token
```

### Run

```bash
./target/release/rustytalon
```

The web UI is available at `http://localhost:3001`.

---

## Local Binary with PostgreSQL

For full-featured deployment with vector search and production-grade persistence.

### Prerequisites

- PostgreSQL 15+ with [pgvector](https://github.com/pgvector/pgvector)
- Or use the development Docker Compose to run PostgreSQL:

```bash
docker compose up -d   # starts pgvector/pgvector:pg16
```

### Build

```bash
cargo build --release   # PostgreSQL is the default feature
```

### Configure

```bash
cp .env.example .env
```

Edit `.env`:

```bash
DATABASE_BACKEND=postgres
DATABASE_URL=postgres://rustytalon:rustytalon@localhost:5432/rustytalon
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-your-key-here
ANTHROPIC_MODEL=claude-sonnet-4-20250514
GATEWAY_AUTH_TOKEN=a-strong-random-token

# Optional: enable semantic search
EMBEDDING_ENABLED=true
OPENAI_API_KEY=sk-your-openai-key
EMBEDDING_MODEL=text-embedding-3-small
```

### Run

```bash
./target/release/rustytalon
```

---

## Docker Compose

Full containerized deployment with PostgreSQL.

### Setup

```bash
# Copy and configure environment
cp .env.example .env
# Edit .env -- at minimum set:
#   ANTHROPIC_API_KEY (or another provider)
#   GATEWAY_AUTH_TOKEN
#   POSTGRES_PASSWORD
```

### Start

```bash
docker compose -f docker-compose.prod.yml up -d
```

This starts:
- **postgres** - PostgreSQL 16 with pgvector
- **rustytalon** - The agent, connected to postgres, web gateway on port 3001

### Verify

```bash
# Check health
curl http://localhost:3001/api/health

# Check logs
docker compose -f docker-compose.prod.yml logs -f rustytalon
```

### Stop

```bash
docker compose -f docker-compose.prod.yml down

# To also remove data volumes:
docker compose -f docker-compose.prod.yml down -v
```

---

## Enabling Docker Sandbox

The Docker sandbox lets RustyTalon execute jobs in isolated containers. This is required for Claude Code mode and recommended for any tool execution involving untrusted code.

### Build the Worker Image

```bash
docker build -f Dockerfile.worker -t rustytalon-worker:latest .
```

The worker image includes:
- Rust toolchain (1.92)
- Node.js and npm
- Python 3 with pip
- Git and common build tools
- Claude Code CLI (for Claude Code mode)

### Configure

Add to your `.env`:

```bash
SANDBOX_ENABLED=true
SANDBOX_IMAGE=rustytalon-worker:latest
SANDBOX_MEMORY_LIMIT_MB=512
SANDBOX_TIMEOUT_SECS=1800
```

### Docker Socket Access

The RustyTalon process needs access to the Docker socket to create worker containers. If running RustyTalon in Docker:

```yaml
# In docker-compose.prod.yml, uncomment:
volumes:
  - /var/run/docker.sock:/var/run/docker.sock
```

### Claude Code Mode

To enable Claude Code delegation inside sandbox containers:

```bash
CLAUDE_CODE_ENABLED=true
CLAUDE_CODE_MODEL=claude-sonnet-4-20250514
CLAUDE_CODE_MAX_TURNS=50
```

---

## Multi-Provider Setup

RustyTalon can route between multiple LLM providers with automatic failover and cost optimization.

### Configure Multiple Providers

Set API keys for each provider you want to use:

```bash
# Primary provider
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-...
ANTHROPIC_MODEL=claude-sonnet-4-20250514

# Additional providers (used by smart router for fallback)
OPENAI_API_KEY=sk-...
OPENAI_MODEL=gpt-4o

OLLAMA_BASE_URL=http://localhost:11434
OLLAMA_MODEL=llama3
```

### Smart Routing

```bash
ROUTING_ENABLED=true
ROUTING_STRATEGY=balanced              # balanced | cost | quality | local_first
ROUTING_ENABLE_FALLBACK=true
ROUTING_MAX_RETRIES=3
ROUTING_PREFERRED_PROVIDERS=anthropic   # comma-separated priority list
```

**Strategies:**
- `balanced` - Weighs cost, quality, and latency equally
- `cost` - Prefer cheapest provider that meets quality threshold
- `quality` - Always use highest-quality provider
- `local_first` - Prefer Ollama/local, fall back to cloud

---

## Channel Configuration

### Web Gateway (default)

Always enabled when `GATEWAY_ENABLED=true`. Provides the browser UI and API.

```bash
GATEWAY_HOST=127.0.0.1    # Use 0.0.0.0 for external access
GATEWAY_PORT=3001
GATEWAY_AUTH_TOKEN=changeme
```

### HTTP Webhooks

Accept messages via HTTP POST:

```bash
HTTP_HOST=0.0.0.0
HTTP_PORT=8080
HTTP_WEBHOOK_SECRET=your-webhook-secret
```

### Telegram

```bash
TELEGRAM_BOT_TOKEN=your-bot-token
```

See [TELEGRAM_SETUP.md](TELEGRAM_SETUP.md) for detailed setup instructions.

### Slack

```bash
SLACK_BOT_TOKEN=xoxb-...
SLACK_APP_TOKEN=xapp-...
SLACK_SIGNING_SECRET=...
```

---

## Routines and Heartbeat

### Routines

Enable background task execution:

```bash
ROUTINES_ENABLED=true
ROUTINES_CRON_INTERVAL=60       # Check cron triggers every 60s
ROUTINES_MAX_CONCURRENT=3       # Max concurrent routine executions
```

### Heartbeat

Proactive periodic execution (reads `HEARTBEAT.md` and reports findings):

```bash
HEARTBEAT_ENABLED=true
HEARTBEAT_INTERVAL_SECS=1800    # Every 30 minutes
HEARTBEAT_NOTIFY_CHANNEL=cli    # Channel to send notifications
HEARTBEAT_NOTIFY_USER=default
```

---

## Security Checklist

Before exposing RustyTalon to a network:

- [ ] Set a strong `GATEWAY_AUTH_TOKEN` (not the default `changeme`)
- [ ] Use HTTPS via a reverse proxy (nginx, Caddy, etc.)
- [ ] Set `GATEWAY_HOST=127.0.0.1` if only accessed locally
- [ ] Set a strong `POSTGRES_PASSWORD`
- [ ] Enable `SAFETY_INJECTION_CHECK_ENABLED=true` (default)
- [ ] Review `SANDBOX_MEMORY_LIMIT_MB` and `SANDBOX_TIMEOUT_SECS`
- [ ] Use full-disk encryption for the host if using libSQL backend
- [ ] Never commit your `.env` file to version control

### Reverse Proxy (nginx example)

```nginx
server {
    listen 443 ssl;
    server_name rustytalon.example.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location / {
        proxy_pass http://127.0.0.1:3001;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_buffering off;           # Required for SSE
        proxy_read_timeout 86400s;     # Long timeout for SSE/WS
    }
}
```

---

## Logging

```bash
# Production (default)
RUST_LOG=rustytalon=info

# Debug
RUST_LOG=rustytalon=debug

# Verbose with HTTP tracing
RUST_LOG=rustytalon=debug,tower_http=debug

# Specific module
RUST_LOG=rustytalon::agent=debug,rustytalon::llm=trace
```

Logs are streamed via the web gateway at `GET /api/logs/events` (SSE).

---

## Troubleshooting

### Agent won't start

1. Check database connectivity: `psql $DATABASE_URL -c "SELECT 1"`
2. Verify pgvector extension: `psql $DATABASE_URL -c "SELECT extname FROM pg_extension"`
3. Check logs: `RUST_LOG=rustytalon=debug ./target/release/rustytalon`

### Web gateway unreachable

1. Verify `GATEWAY_ENABLED=true`
2. Check binding: `GATEWAY_HOST=0.0.0.0` for external access
3. Check port conflicts: `lsof -i :3001`

### Docker sandbox containers not starting

1. Verify Docker is running: `docker ps`
2. Check worker image exists: `docker images rustytalon-worker`
3. Verify Docker socket access if running in a container
4. Check sandbox logs: `docker logs <container-id>`

### Provider errors

1. Verify API key is set and valid
2. Check provider health: `curl http://localhost:3001/api/providers/health -H "Authorization: Bearer $TOKEN"`
3. If using Ollama, verify it's running: `curl http://localhost:11434/api/tags`
