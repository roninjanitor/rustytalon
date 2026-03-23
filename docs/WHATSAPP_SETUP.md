# WhatsApp Channel Setup

This guide covers configuring the WhatsApp channel for RustyTalon using the WhatsApp Cloud API.

## Overview

The WhatsApp channel receives messages via Meta's webhook system (push-only, no polling). It supports:

- **Text messages**: Receive and respond to WhatsApp messages
- **Reply context**: Responses reference the original message
- **WASM sandbox**: Access token injected by host — the channel code never sees the raw credential
- **Webhook verification**: Meta's `hub.verify_token` challenge handled by host

> **Important**: WhatsApp Cloud API requires a **public HTTPS URL** to deliver webhooks. You must expose RustyTalon via a reverse proxy or tunnel before completing Meta app configuration.

## Prerequisites

- RustyTalon running via Docker (channels are pre-installed in the image)
- A [Meta Developer account](https://developers.facebook.com)
- A public HTTPS URL pointing to your RustyTalon instance (port 3001)

## Quick Start (Docker)

### 1. Expose RustyTalon Publicly

Meta must be able to reach your instance to deliver webhook events.

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

Your WhatsApp webhook URL will be: `https://yourdomain.com/webhook/whatsapp`

### 2. Create a Meta App

1. Go to [developers.facebook.com](https://developers.facebook.com) and log in
2. Click **My Apps** → **Create App**
3. Select **Business** as the app type
4. Fill in the name and contact email, then click **Create App**

### 3. Add WhatsApp to Your App

1. In your app dashboard, click **Add Product**
2. Find **WhatsApp** and click **Set Up**
3. Follow the prompts to connect a Meta Business Account (or create one)

### 4. Get Your Access Token

1. In the left sidebar, go to **WhatsApp** → **API Setup**
2. Under **Temporary access token**, click **Copy** — this is your `WHATSAPP_ACCESS_TOKEN`

> **Note**: Temporary tokens expire after 24 hours. For production, create a **System User** with a permanent token: Business Settings → System Users → Add.

### 5. Note Your Phone Number ID

On the same **API Setup** page, copy the **Phone number ID** — you'll need this to test sending messages. RustyTalon reads it automatically from incoming webhook payloads.

### 6. Configure the Webhook

1. In the left sidebar, go to **WhatsApp** → **Configuration**
2. Under **Webhook**, click **Edit**
3. Set:
   - **Callback URL**: `https://yourdomain.com/webhook/whatsapp`
   - **Verify token**: A string you choose (e.g., `rustytalon-verify-abc123`) — this becomes `WHATSAPP_VERIFY_TOKEN`
4. Click **Verify and Save** (RustyTalon must already be running to respond to the challenge)
5. Under **Webhook fields**, click **Manage** and enable:
   - `messages` — incoming messages and status updates

### 7. Set Environment Variables

Add to your `.env` file before starting the container:

```bash
# Required: access token from API Setup page
WHATSAPP_ACCESS_TOKEN=your_access_token_here

# Required: verify token you chose in webhook configuration
WHATSAPP_VERIFY_TOKEN=rustytalon-verify-abc123

# Required for encrypting tokens at rest
SECRETS_MASTER_KEY=your_master_key_here   # openssl rand -base64 32
```

On startup, RustyTalon reads both env vars and stores them in the encrypted secrets store automatically.

### 8. Start RustyTalon

```bash
docker compose -f docker-compose.prod.yml up -d
```

### 9. Verify in the Web UI

1. Open the web UI at `http://localhost:3001` and log in
2. Go to the **Channels** tab
3. Find **WhatsApp** — it should show as running with a green dot

Send a WhatsApp message to your test phone number — the agent will respond.

---

## How It Works

- Meta sends webhook events to `POST /webhook/whatsapp` on your RustyTalon instance
- Meta also sends a `GET /webhook/whatsapp` challenge on first setup; RustyTalon responds automatically using the verify token
- The host validates the `X-Hub-Signature-256` header and the `hub.verify_token` query parameter before passing the event to the channel
- The channel emits the message to the agent and calls `graph.facebook.com` to send the reply

---

## Secrets

The channel uses two secrets:

| Secret name | Env var | Purpose |
|-------------|---------|---------|
| `whatsapp_access_token` | `WHATSAPP_ACCESS_TOKEN` | Authenticates API calls to Meta Graph API |
| `whatsapp_verify_token` | `WHATSAPP_VERIFY_TOKEN` | Validates incoming webhook requests from Meta |

**With Docker (recommended):**

Set both env vars in `.env` before starting. RustyTalon bootstraps them automatically on startup (requires `SECRETS_MASTER_KEY`).

**Update without restart:**

Open the **Channels** tab → WhatsApp → **Set Token** to update the access token in place.

---

## Production: Permanent Access Token

Temporary tokens expire after 24 hours. For production deployments:

1. In your Meta Business account, go to **Business Settings** → **Users** → **System Users**
2. Click **Add** and create a system user with **Employee** or **Admin** role
3. Click **Add Assets** and assign your WhatsApp app with **Full Control**
4. Click **Generate New Token** and select your app
5. Under permissions, enable `whatsapp_business_messaging` and `whatsapp_business_management`
6. Copy the generated token and update `WHATSAPP_ACCESS_TOKEN` in `.env`

---

## Troubleshooting

### Channel shows as "not running" in the Channels tab

- Check that `WHATSAPP_ACCESS_TOKEN` is set in `.env`
- Check container logs: `docker compose -f docker-compose.prod.yml logs rustytalon`

### Meta cannot verify the webhook URL

- Ensure RustyTalon is running before clicking Verify in Meta's dashboard
- Confirm the URL is `https://yourdomain.com/webhook/whatsapp`
- Verify your tunnel or reverse proxy is active and forwarding to port 3001
- Confirm `WHATSAPP_VERIFY_TOKEN` matches exactly what you entered in Meta's dashboard

### Messages not received

- Confirm the `messages` webhook field is enabled in WhatsApp → Configuration → Webhook fields
- Check logs for `Failed to parse webhook payload`
- Verify the `X-Hub-Signature-256` signature validation is passing (wrong verify token causes rejection)

### Responses not sent (403 or 401 from Meta)

- Your access token may have expired — temporary tokens last 24 hours
- Check logs for `WhatsApp API error` with a specific error code
- Verify the access token has `whatsapp_business_messaging` permission

### Only text messages are supported

The channel currently handles text messages only. Image, audio, video, and document messages are logged and skipped.
