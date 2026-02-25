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

- RustyTalon installed and running (`cargo run` or `rustytalon run`)
- A Discord account with **Developer Mode** enabled
- A Discord bot token (see below)

## Quick Start

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

### 4. Build and Install the Channel

```bash
# One-time: add the WASM target
rustup target add wasm32-wasip2
cargo install wasm-tools

# Build
cd channels-src/discord
./build.sh

# Install
mkdir -p ~/.rustytalon/channels
cp discord.wasm discord.capabilities.json ~/.rustytalon/channels/
```

### 5. Configure Owner ID

Edit `~/.rustytalon/channels/discord.capabilities.json` and set your user ID:

```json
{
  "config": {
    "owner_id": "123456789012345678",
    "dm_policy": "pairing",
    "allow_from": []
  }
}
```

### 6. Configure the Bot Token

The bot token is stored securely in RustyTalon's encrypted secrets store. Set it via environment variable:

```bash
DISCORD_BOT_TOKEN=your_bot_token_here
```

Or via the secrets store:

```bash
rustytalon tool auth discord
```

### 7. Run RustyTalon

```bash
cargo run
# or
rustytalon run
```

On startup, the Discord channel will:
1. Fetch the bot's own user ID (for self-message filtering)
2. Open a DM channel with your configured `owner_id`
3. Begin polling every 30 seconds for new messages

Send yourself a DM from your Discord account to your bot — the agent will respond within 30 seconds.

## DM Pairing

When `dm_policy` is `pairing` (default), unknown users who DM the bot receive a pairing code rather than being forwarded to the agent. This protects against unauthorized access.

### Flow

1. Unknown user DMs your bot
2. Bot replies: `To pair with this bot, run: rustytalon pairing approve discord ABC12345`
3. You run: `rustytalon pairing approve discord ABC12345`
4. User is added to the allow list; future messages are delivered to the agent

### Commands

```bash
# List pending pairing requests
rustytalon pairing list discord

# Approve a user
rustytalon pairing approve discord ABC12345
```

### Skip Pairing

To allow all users without pairing (not recommended for public bots):

```json
{
  "config": {
    "dm_policy": "open"
  }
}
```

To pre-approve specific users by their Discord user ID:

```json
{
  "config": {
    "dm_policy": "pairing",
    "allow_from": ["123456789012345678"]
  }
}
```

## Configuration Reference

Edit `~/.rustytalon/channels/discord.capabilities.json`:

| Option | Values | Default | Description |
|--------|--------|---------|-------------|
| `owner_id` | Discord user ID (snowflake string) | `null` | The primary user the bot opens a DM with on startup |
| `dm_policy` | `pairing`, `open` | `pairing` | `pairing` sends a pairing code to unknown users; `open` allows anyone |
| `allow_from` | `["user_id", ...]` | `[]` | Discord user IDs pre-approved without pairing |

## Secrets

The channel expects a secret named `discord_bot_token`. The host injects it as `Authorization: Bot <token>` on every outbound Discord API request — the WASM channel code never sees the raw token.

Configure via:

- **Environment variable**: `DISCORD_BOT_TOKEN=your_token`
- **Secrets store**: `rustytalon tool auth discord`

## Troubleshooting

### Messages not received

- Verify the bot token is set correctly: check logs for `401 Unauthorized`
- Ensure **Message Content Intent** is enabled in the Discord Developer Portal
- Confirm `owner_id` is set in the capabilities config
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
