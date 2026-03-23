# Getting Started with RustyTalon

This guide walks you through getting RustyTalon running for the first time — no prior Rust experience required.

---

## What is RustyTalon?

RustyTalon is a personal AI assistant that runs on your own hardware. Your conversations, memory, and data never leave your control. You interact with it through a web browser, messaging apps (Telegram, Slack, Discord), or the command line.

---

## Step 1 — Choose Your Setup

| Option | Effort | Best For |
|--------|--------|----------|
| [Docker (recommended)](#option-a-docker) | Low | Most users |
| [Local binary (libSQL)](#option-b-local-binary-no-database) | Low | Developers or lightweight use |
| [Local binary + PostgreSQL](#option-c-local-binary-with-postgresql) | Medium | Full features (semantic search) |

---

## Option A — Docker

The easiest way to run RustyTalon. Requires [Docker Desktop](https://www.docker.com/products/docker-desktop/) or Docker Engine.

### 1. Clone the repository

```bash
git clone https://github.com/roninjanitor/rustytalon.git
cd rustytalon
```

### 2. Configure your environment

```bash
cp .env.example .env
```

Open `.env` in a text editor and set at minimum:

```bash
# Your LLM provider API key (pick one)
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-your-key-here
ANTHROPIC_MODEL=claude-sonnet-4-20250514

# Optional: set a fixed token for the web UI. If omitted, a random token is
# generated at startup and printed in the log as part of the web UI URL.
# GATEWAY_AUTH_TOKEN=choose-a-strong-password-here

# Required to install extensions (Telegram, Discord, Google tools, etc.)
# Generate with: openssl rand -base64 32
SECRETS_MASTER_KEY=your-generated-key-here

# Required for docker-compose.prod.yml — set a strong password
POSTGRES_PASSWORD=choose-a-strong-db-password
```

> **Where do I get an API key?**
> - Anthropic (Claude): [console.anthropic.com](https://console.anthropic.com)
> - OpenAI (GPT): [platform.openai.com/api-keys](https://platform.openai.com/api-keys)
> - Ollama (free, local): Install [ollama.ai](https://ollama.ai) and set `LLM_BACKEND=ollama`

### 3. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 4. Open the web UI

Visit [http://localhost:3001](http://localhost:3001). If you set `GATEWAY_AUTH_TOKEN`, use that to log in. If you left it unset, check the container logs for the startup line — it will include a direct URL with the auto-generated token:

```bash
docker compose -f docker-compose.prod.yml logs rustytalon | grep "Web UI"
```

---

## Option B — Local Binary (No Database)

Runs with an embedded SQLite database — no external services needed.

### Prerequisites

- [Rust](https://rustup.rs) 1.85+

### 1. Clone and build

```bash
git clone https://github.com/roninjanitor/rustytalon.git
cd rustytalon
cargo build --release --no-default-features --features libsql
```

### 2. Configure

```bash
cp .env.example .env
```

Edit `.env`:

```bash
DATABASE_BACKEND=libsql
LLM_BACKEND=anthropic
ANTHROPIC_API_KEY=sk-ant-your-key-here
ANTHROPIC_MODEL=claude-sonnet-4-20250514
# GATEWAY_AUTH_TOKEN=choose-a-strong-password-here  # optional; auto-generated if omitted

# Required to install extensions (Telegram, Discord, Google tools, etc.)
# Generate with: openssl rand -base64 32
SECRETS_MASTER_KEY=your-generated-key-here
```

### 3. Run

```bash
./target/release/rustytalon
```

Visit [http://localhost:3001](http://localhost:3001).

---

## Option C — Local Binary with PostgreSQL

Needed for full-featured semantic memory search with vector embeddings.

### Prerequisites

- [Rust](https://rustup.rs) 1.85+
- PostgreSQL 15+ with the [pgvector](https://github.com/pgvector/pgvector) extension

**Quick PostgreSQL setup using Docker:**

```bash
docker compose up -d db   # starts postgres with pgvector
```

### 1. Clone and build

```bash
git clone https://github.com/roninjanitor/rustytalon.git
cd rustytalon
cargo build --release
```

### 2. Configure

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
# GATEWAY_AUTH_TOKEN=choose-a-strong-password-here  # optional; auto-generated if omitted

# Required to install extensions (Telegram, Discord, Google tools, etc.)
# Generate with: openssl rand -base64 32
SECRETS_MASTER_KEY=your-generated-key-here

# Optional: enable semantic memory search
EMBEDDING_ENABLED=true
OPENAI_API_KEY=sk-your-openai-key
```

### 3. Run

```bash
./target/release/rustytalon
```

Visit [http://localhost:3001](http://localhost:3001).

---

## First Steps in the Web UI

Once you're in the web UI:

1. **Chat tab** — Type a message and press Enter. The agent responds in real time.
2. **Memory tab** — Browse and search files the agent has stored about you and your projects.
3. **Jobs tab** — See background tasks the agent is working on.
4. **Routines tab** — Set up scheduled or event-driven automations.
5. **Extensions tab** — Install new tools and integrations.

See [WEB_UI.md](WEB_UI.md) for a full walkthrough.

---

## Connecting Messaging Apps (Optional)

You can chat with RustyTalon through messaging apps instead of (or alongside) the web UI:

- **Telegram** — [TELEGRAM_SETUP.md](TELEGRAM_SETUP.md)
- **Discord** — [DISCORD_SETUP.md](DISCORD_SETUP.md)
- **Matrix** — [MATRIX_SETUP.md](MATRIX_SETUP.md)

---

## Troubleshooting

### "Connection refused" on http://localhost:3001

- Verify the container or process is running: `docker compose ps` or check your terminal
- Check that `GATEWAY_ENABLED=true` in `.env`
- If you changed the port, use `http://localhost:<your-port>` instead

### API key errors

- Double-check the key is correct and has not expired
- Make sure there are no extra spaces or quotes around the key in `.env`

### Docker containers keep restarting

- Run `docker compose -f docker-compose.prod.yml logs rustytalon` to see the error
- Usually caused by a missing or invalid API key

### "Extension installation requires a master key" banner

Set `SECRETS_MASTER_KEY` in `.env` and restart. Generate the key with:

```bash
openssl rand -base64 32
```

See [TOOLS_AND_EXTENSIONS.md](TOOLS_AND_EXTENSIONS.md#prerequisites) for details.

### More help

See [DEPLOYMENT.md](DEPLOYMENT.md) for advanced setup options and the full troubleshooting section.
