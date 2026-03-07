# Matrix Channel Setup

This guide covers configuring the Matrix channel for RustyTalon so you can chat with the agent via Matrix rooms (including DMs).

## Overview

The Matrix channel communicates via the Matrix Client-Server API using REST polling. It supports:

- **Any homeserver** — matrix.org, Element Matrix Services, self-hosted Synapse, Dendrite, Conduit, etc.
- **DM-based chat** — the agent responds directly in a Matrix DM room
- **Multi-room** — join the bot to any number of rooms; it polls all of them
- **Auto-invite accept** — the bot accepts invites from allowed users automatically
- **Typing indicator** — shows the typing notification while the agent processes
- **DM pairing** — approve unknown users before they can message the agent
- **WASM sandbox** — access token is injected at the host boundary; channel code never sees the raw credential

> **Note on transport**: The Matrix sync endpoint (`/_matrix/client/v3/sync`) supports both long-polling and non-blocking modes. This channel uses `timeout=0` (non-blocking) called every 30 seconds, matching the same trade-off as the Telegram and Discord channels.

## Two Accounts Required

Like Discord, this channel requires **two separate Matrix accounts**:

| Account | Purpose | Whose token goes in config |
|---------|---------|---------------------------|
| **Bot account** | The identity RustyTalon runs as (e.g. `@rustytalon:matrix.org`) | ✅ Yes — this token |
| **Your personal account** | You, chatting in Element (e.g. `@you:matrix.org`) | ❌ No — set as `owner_id` |

On startup the bot creates a DM room and invites your personal account. You accept and chat normally; the agent responds as the bot.

## Prerequisites

- RustyTalon installed and running (`cargo run` or `rustytalon run`)
- Two Matrix accounts (see above) — both can be on matrix.org for testing

## Quick Start

### 1. Create a Bot Account

Create a second Matrix account for the bot at [app.element.io](https://app.element.io) → Create account.

Pick a name like `rustytalon` or `myagent`. You can use matrix.org as the homeserver.

### 2. Get the Bot Account's Access Token

Sign into Element **as the bot account**, then:

**Option A — Element Web / Desktop:**

1. Go to **Settings** (gear icon) → **Help & About**
2. Scroll to the bottom and click **Access Token** → copy it

**Option B — curl:**

```bash
curl -XPOST 'https://matrix.org/_matrix/client/v3/login' \
  -H 'Content-Type: application/json' \
  -d '{
    "type": "m.login.password",
    "user": "@rustytalon:matrix.org",
    "password": "bot-account-password"
  }'
```

The response includes `"access_token"`. Copy it.

> **Important**: This must be the **bot account's** token, not your personal account's token.

### 3. Build and Install the Channel

```bash
# One-time: add the WASM target
rustup target add wasm32-wasip2
cargo install wasm-tools

# Build
cd channels-src/matrix
./build.sh

# Install
mkdir -p ~/.rustytalon/channels
cp matrix.wasm matrix.capabilities.json ~/.rustytalon/channels/
```

### 4. Configure Homeserver and Owner ID

Edit `~/.rustytalon/channels/matrix.capabilities.json`:

```json
{
  "config": {
    "homeserver": "https://matrix.org",
    "owner_id": "@you:matrix.org",
    "dm_policy": "pairing",
    "allow_from": []
  }
}
```

- `homeserver` — the bot account's homeserver
- `owner_id` — **your personal** Matrix user ID (not the bot's)

### 5. Configure the Access Token

The bot account's access token is stored in RustyTalon's encrypted secrets store.

Via environment variable:

```bash
MATRIX_ACCESS_TOKEN=bot_account_token_here
```

Or via the secrets store:

```bash
rustytalon tool auth matrix
```

### 6. Run RustyTalon

```bash
cargo run
# or
rustytalon run
```

On startup, the Matrix channel will:
1. Validate the bot's access token via `/account/whoami`
2. Look for an existing DM room with your `owner_id` in `m.direct` account data
3. If no DM room exists, create one and invite your `owner_id`
4. Begin polling every 30 seconds

Open Element signed in as **your personal account**, accept the bot's invite, and send a message — the agent will respond within 30 seconds.

## DM Pairing

When `dm_policy` is `pairing` (default), unknown users who message the bot receive a pairing code rather than being forwarded to the agent. This protects against unauthorized access.

### Flow

1. Unknown user sends a message to the bot
2. Bot replies: `To pair with this bot, run: rustytalon pairing approve matrix ABC12345`
3. You run: `rustytalon pairing approve matrix ABC12345`
4. User is added to the allow list; future messages are delivered to the agent

### Commands

```bash
# List pending pairing requests
rustytalon pairing list matrix

# Approve a user
rustytalon pairing approve matrix ABC12345
```

### Skip Pairing (for testing)

To allow all users without pairing:

```json
{
  "config": {
    "dm_policy": "open"
  }
}
```

To pre-approve specific users by their Matrix user ID:

```json
{
  "config": {
    "dm_policy": "pairing",
    "allow_from": ["@trusted:matrix.org"]
  }
}
```

## Using Multiple Rooms

The bot responds to any room it is joined to. To add the bot to a room:

1. Invite the bot's Matrix user ID to the room from your personal account
2. If your `dm_policy` is `open`, or the inviter is in the `allow_from` list, the bot auto-accepts
3. Otherwise, approve the invite via `rustytalon pairing approve matrix <code>`

The agent maintains separate conversation context per room (using the room ID as the thread ID).

## Self-Hosted Homeservers

If you run your own Synapse, Dendrite, or Conduit instance, set `homeserver` to your server's base URL:

```json
{
  "config": {
    "homeserver": "https://your-server.example.com",
    "owner_id": "@you:your-server.example.com"
  }
}
```

No other changes are needed — the channel speaks standard Matrix CS API v3.

## Configuration Reference

Edit `~/.rustytalon/channels/matrix.capabilities.json`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `homeserver` | string | `"https://matrix.org"` | Base URL of the **bot account's** homeserver |
| `owner_id` | string \| null | `null` | Your **personal** Matrix user ID (e.g. `@you:matrix.org`). The bot opens a DM with this user on startup |
| `dm_policy` | `"pairing"` \| `"open"` | `"pairing"` | `pairing` sends a pairing code to unknown users; `open` allows anyone |
| `allow_from` | `["@user:server", ...]` | `[]` | Matrix user IDs pre-approved without pairing |

## Secrets

The channel expects a secret named `matrix_access_token` — this is the **bot account's** token. The host injects it as `Authorization: Bearer <token>` on every outbound Matrix API request — the WASM channel code never sees the raw token.

Configure via:

- **Environment variable**: `MATRIX_ACCESS_TOKEN=bot_token`
- **Secrets store**: `rustytalon tool auth matrix`

## Troubleshooting

### "Failed to authenticate with Matrix homeserver"

- Verify you're using the **bot account's** token, not your personal account's
- Check that `homeserver` URL is reachable and does not have a trailing slash
- Try the token manually: `curl -H "Authorization: Bearer <token>" https://your-server/_matrix/client/v3/account/whoami`

### Bot doesn't receive messages

- Confirm the bot is joined to the room (check in Element signed in as the bot account)
- Verify the poll is running: look for `Matrix /sync failed` errors in logs
- Check that `dm_policy` allows the sender (or set to `open` for testing)

### "createRoom returned 403"

Your bot account may not have permission to create rooms. Check your homeserver's room creation policies, or create the DM manually in Element (as the bot account) and invite your personal account instead.

### Responses delayed up to 30 seconds

This is expected — the channel polls every 30 seconds. The typing indicator (`…is typing`) appears during agent processing to indicate activity.

### Token expired

Matrix access tokens can expire or be invalidated. Regenerate a token in Element (signed in as the bot account) and update via `rustytalon tool auth matrix`.
