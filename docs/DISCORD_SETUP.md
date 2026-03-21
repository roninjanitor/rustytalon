# Discord Channel Setup

This guide covers configuring the Discord channel for RustyTalon so you can chat with the agent via Discord Direct Messages.

## Overview

The Discord channel communicates via Discord DMs using REST polling (no WebSocket/Gateway required). It supports:

- **DM-based chat**: The agent responds directly in your Discord DMs
- **DM pairing**: Approve unknown users before they can message the agent
- **Typing indicator**: Shows "Bot is typing…" while the agent processes
- **WASM sandbox**: Bot token is injected at the host boundary — the channel code never sees the raw credential

> **Note on transport**: Discord's real-time Gateway requires a persistent WebSocket connection, which isn't available inside a WASM component. Instead, the channel polls `GET /channels/{id}/messages` every 30 seconds. This is the same trade-off the Telegram channel makes when running in polling mode.

## Prerequisites

- RustyTalon running (Docker recommended — channels are pre-installed in the image)
- A Discord account with **Developer Mode** enabled
- A Discord bot token (see below)

## Quick Start (Docker)

### 1. Create a Discord Application and Bot

1. Go to [discord.com/developers/applications](https://discord.com/developers/applications)
2. Click **New Application**, give it a name (e.g. `RustyTalon`)
3. In the left sidebar, click **Bot**
4. Click **Reset Token**, copy and save your bot token
5. Under **Privileged Gateway Intents**, enable **Message Content Intent**
   - This is required for the bot to read message content in DMs

### 2. Invite the Bot to a Server (or Enable DMs)

Discord DMs with a bot only work if the user shares a server with the bot, or has previously interacted with it.

**Option A — Add to a server:**
1. In your application, go to **OAuth2 → URL Generator**
2. Select scope: `bot`
3. Select permissions: `Send Messages`, `Read Message History`
4. Open the generated URL and add the bot to a server you own

**Option B — Direct DMs (no server needed):**
If you already share a server with the bot, you can DM it directly.

### 3. Get Your Discord User ID

The channel needs your Discord user ID to open a DM with you on startup.

1. In Discord settings, go to **Advanced** → enable **Developer Mode**
2. Right-click your own username anywhere in Discord
3. Click **Copy User ID** — this is your snowflake ID (e.g. `123456789012345678`)

### 4. Set Environment Variables

Add to your `.env` file before starting the container:

```bash
# Required: your bot token
DISCORD_BOT_TOKEN=your_bot_token_here

# Required for SECRETS_MASTER_KEY to be set (encrypt tokens at rest)
SECRETS_MASTER_KEY=your_master_key_here   # openssl rand -base64 32
```

The Discord channel is pre-installed in the Docker image. On startup, RustyTalon automatically reads `DISCORD_BOT_TOKEN` and stores it in the encrypted secrets store.

### 5. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 6. Configure via the Web UI

1. Open the web UI at `http://localhost:3001` and log in
2. Go to the **Channels** tab
3. Find **Discord** — it should show as running with a green dot
4. Click the **⚙** (config) button to set your owner ID and DM policy:

```json
{
  "owner_id": "123456789012345678",
  "dm_policy": "pairing",
  "allow_from": []
}
```

5. Click **Save** — the channel picks up the new config on the next poll

Send yourself a DM from your Discord account to your bot — the agent will respond within 30 seconds.

---

## DM Pairing

When `dm_policy` is `pairing` (default), unknown users who DM the bot receive a pairing code rather than being forwarded to the agent. This protects against unauthorized access.

### Flow

1. Unknown user DMs your bot
2. Bot replies: `To pair with this bot, run: rustytalon pairing approve discord ABC12345`
3. You run: `rustytalon pairing approve discord ABC12345`
4. User is added to the allow list; future messages are delivered to the agent

---

## Configuration Reference

Configure via the web UI **Channels** tab → Discord → **⚙**:

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `owner_id` | Discord user ID (snowflake string) | `null` | The primary user the bot opens a DM with on startup |
| `dm_policy` | `pairing`, `open` | `pairing` | `pairing` sends a pairing code to unknown users; `open` allows anyone |
| `allow_from` | `["user_id", ...]` | `[]` | Discord user IDs pre-approved without pairing |

---

## Secrets

The channel expects a secret named `discord_bot_token`. The host injects it as `Authorization: Bot <token>` on every outbound Discord API request — the WASM channel code never sees the raw token.

**With Docker (recommended):**

Set `DISCORD_BOT_TOKEN=your_token` in `.env` before starting the container. RustyTalon bootstraps it into the encrypted secrets store on startup (requires `SECRETS_MASTER_KEY`).

**Update via web UI:**

Open the **Channels** tab → Discord → **Set Token** to update the token without restarting.

---

## Troubleshooting

### Messages not received

- Verify the bot token is set correctly: check logs for `401 Unauthorized`
- Ensure **Message Content Intent** is enabled in the Discord Developer Portal
- Confirm `owner_id` is set in the channel config (Channels tab → ⚙)
- Check logs for `Failed to open DM channel with owner` on startup

### Bot doesn't appear online

The bot does not maintain a persistent WebSocket connection (by design — WASM channels use REST polling). It will not show as "Online" in Discord's sidebar. This is expected.

### "403 Forbidden" when fetching messages

The bot lacks access to the channel. Ensure:
- The bot was invited to at least one shared server with the user
- The user has not blocked the bot

### Pairing code not received

- Verify `dm_policy` is `pairing` (not `open`)
- Check logs for `Pairing upsert failed`
- Ensure the bot can send messages (no DM restrictions on the target user)

### Responses delayed up to 30 seconds

This is expected — the channel polls every 30 seconds. The typing indicator (`Bot is typing…`) appears during agent processing to indicate activity.

### Channel shows as "not running" in the Channels tab

- Check that `DISCORD_BOT_TOKEN` is set in your `.env`
- If `SECRETS_MASTER_KEY` is not set, the token is injected directly from env at startup — verify it's present in `docker compose config`
- Check container logs: `docker compose -f docker-compose.prod.yml logs rustytalon`
