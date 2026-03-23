# Slack Channel Setup

This guide covers configuring the Slack channel for RustyTalon so you can chat with the agent via Slack DMs and @mentions.

## Overview

The Slack channel receives events via Slack's Events API (webhook-based, no polling). It supports:

- **@mentions**: Mention the bot in any channel it's been added to
- **Direct messages**: DM the bot directly
- **Thread replies**: Agent responses are threaded to the triggering message
- **WASM sandbox**: Bot token injected by host — the channel code never sees the raw credential
- **Signature validation**: Slack request signatures validated by host before reaching the channel

> **Important**: Slack requires a **public HTTPS URL** to deliver events. You must expose RustyTalon via a reverse proxy or tunnel before completing Slack app configuration.

## Prerequisites

- RustyTalon running via Docker (channels are pre-installed in the image)
- A Slack workspace where you have permission to install apps
- A public HTTPS URL pointing to your RustyTalon instance (port 3001)

## Quick Start (Docker)

### 1. Expose RustyTalon Publicly

Slack must be able to reach your instance to deliver events. Options:

**Cloudflare Tunnel (recommended for production):**

```bash
cloudflared tunnel --url http://localhost:3001
# Note the assigned URL, e.g.: https://abc123.trycloudflare.com
```

**ngrok (for testing):**

```bash
ngrok http 3001
# Note the HTTPS URL, e.g.: https://abc123.ngrok.io
```

**Reverse proxy:** If you have a domain, configure nginx/Caddy to proxy to port 3001 with a valid TLS certificate.

Your Slack Event Request URL will be: `https://yourdomain.com/webhook/slack`

### 2. Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and click **Create New App**
2. Choose **From scratch**, give it a name (e.g. `RustyTalon`), and select your workspace
3. Click **Create App**

### 3. Add Bot Token Scopes

1. In the left sidebar, go to **OAuth & Permissions**
2. Scroll to **Scopes** → **Bot Token Scopes**
3. Add the following scopes:
   - `chat:write` — send messages
   - `app_mentions:read` — receive @mentions
   - `im:read` — receive DMs
   - `im:history` — read DM history

### 4. Enable Events API

1. In the left sidebar, go to **Event Subscriptions**
2. Toggle **Enable Events** to **On**
3. In the **Request URL** field, enter your public URL:
   ```
   https://yourdomain.com/webhook/slack
   ```
4. Wait for Slack to verify the URL (RustyTalon must already be running to respond to the challenge)
5. Under **Subscribe to bot events**, add:
   - `app_mention` — @mentions in channels
   - `message.im` — direct messages to the bot
6. Click **Save Changes**

### 5. Install the App to Your Workspace

1. In the left sidebar, go to **OAuth & Permissions**
2. Click **Install to Workspace** and authorize the permissions
3. Copy the **Bot User OAuth Token** (starts with `xoxb-`)

### 6. Get the Signing Secret

1. In the left sidebar, go to **Basic Information**
2. Scroll to **App Credentials**
3. Copy the **Signing Secret**

### 7. Set Environment Variables

Add to your `.env` file before starting the container:

```bash
# Required: bot token from OAuth & Permissions
SLACK_BOT_TOKEN=xoxb-your-token-here

# Required: signing secret from Basic Information
SLACK_SIGNING_SECRET=your-signing-secret-here

# Required for encrypting tokens at rest
SECRETS_MASTER_KEY=your_master_key_here   # openssl rand -base64 32
```

On startup, RustyTalon reads `SLACK_BOT_TOKEN` and `SLACK_SIGNING_SECRET` and stores them in the encrypted secrets store automatically.

### 8. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 9. Verify in the Web UI

1. Open the web UI at `http://localhost:3001` and log in
2. Go to the **Channels** tab
3. Find **Slack** — it should show as running with a green dot

Invite the bot to a channel (`/invite @YourBot`) or send it a DM — it will respond.

---

## How It Works

- Slack sends events to `/webhook/slack` on your RustyTalon instance
- The host validates the `X-Slack-Signature` header using `slack_signing_secret` before passing the event to the channel
- The channel returns `200 OK` immediately (Slack requires a response within 3 seconds)
- The agent processes the message and calls `chat.postMessage` to reply, threading the response to the original message

---

## Secrets

The channel uses two secrets:

| Secret name | Env var | Purpose |
|-------------|---------|---------|
| `slack_bot_token` | `SLACK_BOT_TOKEN` | Authenticates API calls to Slack (injected as Bearer header) |
| `slack_signing_secret` | `SLACK_SIGNING_SECRET` | Validates incoming webhook signatures (used by host) |

**With Docker (recommended):**

Set both env vars in `.env` before starting. RustyTalon bootstraps them automatically on startup (requires `SECRETS_MASTER_KEY`).

**Update without restart:**

Open the **Channels** tab → Slack → **Set Token** to update the bot token in place. For the signing secret, restart is required.

---

## Troubleshooting

### Channel shows as "not running" in the Channels tab

- Check that `SLACK_BOT_TOKEN` is set in `.env`
- Check container logs: `docker compose -f docker-compose.prod.yml logs rustytalon`

### Slack cannot verify the Request URL

- Ensure RustyTalon is running before saving the URL in Slack — it must respond to the `url_verification` challenge
- Confirm the URL is `https://yourdomain.com/webhook/slack` (with `/webhook/slack` path)
- Verify your tunnel or reverse proxy is active and forwarding traffic to port 3001

### "dispatch_failed" errors in Slack

- This means Slack sent the event but got an error response; check container logs for parse errors
- Ensure the signing secret is correct — a wrong secret causes signature validation to fail and the event to be rejected

### Bot doesn't receive @mentions

- Ensure you subscribed to the `app_mention` event in Event Subscriptions
- Invite the bot to the channel: `/invite @YourBot`

### Bot doesn't receive DMs

- Ensure you subscribed to the `message.im` event in Event Subscriptions
- Reinstall the app to your workspace after adding scopes (OAuth & Permissions → Install to Workspace)

### Responses not sent

- Check logs for `chat.postMessage` errors
- Verify the bot token has the `chat:write` scope
- Ensure the bot is a member of the channel it's trying to post to
