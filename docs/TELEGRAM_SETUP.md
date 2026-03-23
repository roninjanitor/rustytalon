# Telegram Channel Setup

This guide covers configuring the Telegram channel for RustyTalon so you can chat with the agent via Telegram.

## Overview

The Telegram channel communicates via the Bot API. It supports:

- **Polling mode** (default): No public URL required; ~30s message delay
- **Webhook mode** (optional): Instant delivery — requires a public HTTPS URL
- **DM pairing**: Approve unknown users before they can message the agent
- **Group mentions**: `@YourBot` or `/command` to trigger in groups

## Prerequisites

- RustyTalon running via Docker (channels are pre-installed in the image)
- A Telegram bot token from [@BotFather](https://t.me/BotFather)

## Quick Start (Docker)

### 1. Create a Bot

1. Open Telegram and message [@BotFather](https://t.me/BotFather)
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

On startup, RustyTalon reads `TELEGRAM_BOT_TOKEN` and stores it in the encrypted secrets store automatically.

### 3. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 4. Verify in the Web UI

1. Open the web UI at `http://localhost:3001` and log in
2. Go to the **Channels** tab
3. Find **Telegram** — it should show as running with a green dot

The channel starts in **polling mode** by default (no tunnel needed). Send a DM to your bot and it will respond within ~30 seconds.

---

## (Optional) Configure Webhook for Instant Delivery

For instant message delivery, expose RustyTalon via a public HTTPS tunnel and set the tunnel URL in the web UI.

**Start a tunnel:**

```bash
# ngrok
ngrok http 3001

# Cloudflare Tunnel
cloudflared tunnel --url http://localhost:3001
```

**Configure in the web UI:**

1. Go to **Channels** tab → Telegram → **⚙**
2. Set `tunnel_url` to your tunnel URL (e.g., `https://abc123.ngrok.io`)
3. Click **Save** and restart RustyTalon

On startup the channel automatically registers the webhook URL with Telegram and switches from polling to push delivery.

---

## DM Pairing

When `dm_policy` is `pairing` (default), unknown users who DM your bot receive a pairing code instead of being forwarded to the agent. This protects against unauthorized access.

### Flow

1. Unknown user sends a DM to your bot
2. Bot replies: `To pair with this bot, run: rustytalon pairing approve telegram ABC12345`
3. You run: `rustytalon pairing approve telegram ABC12345`
4. User is added to the allow list; future messages are delivered

---

## Configuration Reference

Configure via the web UI **Channels** tab → Telegram → **⚙**:

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `dm_policy` | `pairing`, `allowlist`, `open` | `pairing` | `pairing` = send pairing code to unknown users; `allowlist` = silently drop unknowns; `open` = allow anyone |
| `allow_from` | `["user_id", "username"]` | `[]` | Pre-approved user IDs or usernames (merged with pairing-approved list) |
| `owner_id` | Telegram user ID | `null` | When set, only this user can message the agent (all others silently dropped) |
| `bot_username` | Bot username (no @) | `null` | Required for @mention detection in group chats |
| `respond_to_all_group_messages` | `true`/`false` | `false` | When `true`, respond to all group messages; when `false`, only @mentions and /commands |
| `tunnel_url` | HTTPS URL | `null` | Injected by host from global tunnel settings; enables webhook mode when set |

---

## Secrets

The channel expects a secret named `telegram_bot_token`.

**With Docker (recommended):**

Set `TELEGRAM_BOT_TOKEN=your_token` in `.env` before starting. RustyTalon bootstraps it automatically on startup (requires `SECRETS_MASTER_KEY`).

**Update without restart:**

Open the **Channels** tab → Telegram → **Set Token** to update the token in place.

---

## Troubleshooting

### Channel shows as "not running" in the Channels tab

- Check that `TELEGRAM_BOT_TOKEN` is set in your `.env`
- Check container logs: `docker compose -f docker-compose.prod.yml logs rustytalon`
- Verify the token is valid: `curl https://api.telegram.org/bot<token>/getMe`

### Messages not delivered (polling mode)

- Check logs for `getUpdates` errors — the bot token may be invalid or revoked
- Confirm only one bot instance is running (two pollers on the same token conflict)

### Messages not delivered (webhook mode)

- Verify the tunnel is running and `tunnel_url` is the correct public HTTPS URL
- Telegram requires HTTPS — plain HTTP tunnels won't work
- Check logs for `Failed to register webhook`

### Pairing code not received

- Confirm `dm_policy` is `pairing` (not `allowlist` which silently drops unknowns)
- Check the channel can reach `api.telegram.org` (HTTP allowlist)

### Group mentions not working

- Set `bot_username` in config to your bot's username without the `@` (e.g., `MyRustyTalonBot`)
- Ensure the message contains `@YourBot` or starts with `/`
