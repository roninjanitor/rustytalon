# Telegram Channel Setup

This guide covers configuring the Telegram channel for RustyTalon, including DM pairing for access control.

## Overview

The Telegram channel lets you interact with RustyTalon via Telegram DMs and groups. It supports:

- **Polling mode**: No tunnel required; ~30s delay
- **Webhook mode** (optional): Instant delivery via tunnel
- **DM pairing**: Approve unknown users before they can message the agent
- **Group mentions**: `@YourBot` or `/command` to trigger in groups

## Prerequisites

- RustyTalon running (Docker recommended — channels are pre-installed in the image)
- A Telegram bot token from [@BotFather](https://t.me/BotFather)

## Quick Start (Docker)

### 1. Create a Bot

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot` and follow the prompts
3. Copy the bot token (e.g., `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`)

### 2. Set Environment Variables

Add to your `.env` file before starting the container:

```bash
# Required: your bot token
TELEGRAM_BOT_TOKEN=your_bot_token_here

# Required for encrypting tokens at rest
SECRETS_MASTER_KEY=your_master_key_here   # openssl rand -base64 32
```

The Telegram channel is pre-installed in the Docker image. On startup, RustyTalon automatically reads `TELEGRAM_BOT_TOKEN` and stores it in the encrypted secrets store.

### 3. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 4. Verify in the Web UI

1. Open the web UI at `http://localhost:3001` and log in
2. Go to the **Channels** tab
3. Find **Telegram** — it should show as running with a green dot
4. Click **⚙** to configure DM policy and other options if needed

The channel starts in polling mode by default. Send a message to your bot and it will respond within ~30 seconds.

---

## (Optional) Configure Tunnel for Webhooks

For instant message delivery, expose your agent via a tunnel:

```bash
# ngrok
ngrok http 3001

# Cloudflare
cloudflared tunnel --url http://localhost:3001
```

Set the tunnel URL in settings or via `TUNNEL_URL` env var. Without a tunnel, the channel uses polling (~30s delay).

---

## DM Pairing

When an unknown user DMs your bot, they receive a pairing code. You must approve them before they can message the agent.

### Flow

1. Unknown user sends a message to your bot
2. Bot replies: `To pair with this bot, run: rustytalon pairing approve telegram ABC12345`
3. You run: `rustytalon pairing approve telegram ABC12345`
4. User is added to the allow list; future messages are delivered

---

## Configuration Reference

Configure via the web UI **Channels** tab → Telegram → **⚙**:

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `dm_policy` | `open`, `allowlist`, `pairing` | `pairing` | `open` = allow all; `allowlist` = config + approved only; `pairing` = allowlist + send pairing reply to unknown |
| `allow_from` | `["user_id", "username", "*"]` | `[]` | Pre-approved IDs/usernames. `*` allows everyone. |
| `owner_id` | Telegram user ID | `null` | When set, only this user can message (overrides dm_policy) |
| `bot_username` | Bot username (no @) | `null` | Used for mention detection in groups |
| `respond_to_all_group_messages` | `true`/`false` | `false` | When true, respond to all group messages; when false, only @mentions and /commands |

---

## Secrets

The channel expects a secret named `telegram_bot_token`.

**With Docker (recommended):**

Set `TELEGRAM_BOT_TOKEN=your_token` in `.env` before starting the container. RustyTalon bootstraps it into the encrypted secrets store on startup (requires `SECRETS_MASTER_KEY`).

**Update via web UI:**

Open the **Channels** tab → Telegram → **Set Token** to update the token without restarting.

## Webhook Secret (Optional)

For webhook validation, set `telegram_webhook_secret` in secrets. Telegram will send `X-Telegram-Bot-Api-Secret-Token` with each request; the host validates it before forwarding.

---

## Troubleshooting

### Messages not delivered

- **Polling mode**: Check logs for `getUpdates` errors. Ensure the bot token is valid.
- **Webhook mode**: Verify tunnel is running and `TUNNEL_URL` is correct. Telegram requires HTTPS.

### Pairing code not received

- Verify the channel can send messages (HTTP allowlist includes `api.telegram.org`)
- Check `dm_policy` is `pairing` (not `allowlist` which blocks without reply)

### Group mentions not working

- Set `bot_username` in config to your bot's username (e.g., `MyRustyTalonBot`)
- Ensure the message contains `@YourBot` or starts with `/`

### Channel shows as "not running" in the Channels tab

- Check that `TELEGRAM_BOT_TOKEN` is set in your `.env`
- Check container logs: `docker compose -f docker-compose.prod.yml logs rustytalon`
