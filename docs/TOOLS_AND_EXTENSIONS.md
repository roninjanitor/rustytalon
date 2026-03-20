# Tools & Extensions Guide

RustyTalon comes with a set of built-in tools and supports installing additional tools and integrations as extensions. This guide covers what's available and how to add more.

---

## Built-in Tools

These tools are always available without any installation:

| Tool | What it does |
|------|-------------|
| `echo` | Return text (useful for testing) |
| `time` | Get the current date and time |
| `json` | Parse and format JSON data |
| `http` | Make HTTP requests (requires approval) |
| `read_file` | Read a file from disk |
| `write_file` | Write a file to disk (requires approval) |
| `list_dir` | List directory contents |
| `apply_patch` | Apply a unified diff to a file (requires approval) |
| `shell` | Run a shell command (requires approval) |
| `memory_search` | Search the workspace with hybrid FTS + vector search |
| `memory_write` | Write a file to the workspace |
| `memory_read` | Read a file from the workspace |
| `memory_tree` | View the workspace directory structure |
| `create_job` | Create a background sandbox job |
| `list_jobs` | List all jobs |
| `job_status` | Check the status of a specific job |
| `cancel_job` | Cancel a running job |
| `routine_create` | Create a new routine |
| `routine_list` | List all routines |
| `routine_update` | Update a routine |
| `routine_delete` | Delete a routine |
| `routine_history` | View routine execution history |
| `extension_install` | Install an extension |
| `extension_auth` | Authenticate an extension |
| `extension_activate` | Activate an installed extension |
| `extension_remove` | Remove an extension |

Type `/tools` in chat to see the current list of all available tools, including any installed extensions.

---

## Tool Approval

Some tools can cause irreversible side effects and require your explicit approval before the agent can use them:

- `shell` — Execute shell commands
- `http` — Make HTTP requests to external services
- `write_file` — Write or overwrite files
- `apply_patch` — Modify files via patch
- `build_software` — Dynamically build new WASM tools

When the agent wants to use one of these, it will pause and show you:
- Which tool it wants to use
- What parameters it will pass
- What it's trying to accomplish

You can respond:
- **`yes`** — Approve this one time
- **`always`** — Auto-approve this tool for the rest of the session
- **`no`** — Deny and let the agent try a different approach

---

## Extensions

Extensions add capabilities beyond the built-in tools. There are two types:

### WASM Extensions

WASM tools run inside the agent process in a sandboxed WebAssembly environment. They have:

- Explicit capability declarations (which URLs they can call, which secrets they need)
- Credential injection at the host boundary (the tool code never sees raw API keys)
- Memory and CPU limits
- Output scanning for secret leakage

**Examples of WASM extensions:**
- Telegram channel
- Slack channel
- Discord channel
- Google Workspace tools (Gmail, Calendar, Drive, Sheets, Docs, Slides)

### MCP Extensions

MCP (Model Context Protocol) servers are external processes that expose tools via a standard protocol. They run separately from the agent and can be written in any language. A large ecosystem of pre-built MCP servers exists for popular services.

**Use MCP when:**
- A good server already exists for the service you need
- You need WebSocket connections or background polling
- You want to prototype quickly

**Use WASM when:**
- You're handling sensitive credentials (email, banking, etc.)
- You want the security guarantees of the WASM sandbox
- You're building something you'll maintain long-term

---

## Installing Extensions

### Prerequisites

Extension installation requires a **secrets master key** to encrypt stored credentials (API tokens, OAuth secrets). Without it, the catalog is still browsable but installation is disabled.

Set `SECRETS_MASTER_KEY` in your `.env` file before trying to install anything:

```bash
# Generate a random key
openssl rand -base64 32

# Add to .env
SECRETS_MASTER_KEY=<paste the output here>
```

Restart RustyTalon after adding the key. The web UI will show a setup banner if this key is missing.

### Via the Web UI

1. Open the **Extensions** tab
2. Browse or search the **Catalog**
3. Click **Install** on any extension
4. Follow the setup wizard (it guides you through credentials if needed)
5. The extension is activated automatically once authentication is complete

### Via the Agent

Ask in chat:

> *"Install the MCP server at https://my-server.example.com"*
> *"Install the WASM tool from /path/to/my-tool.wasm"*

### Via the API

```bash
# Install a WASM tool
curl -X POST http://localhost:3001/api/extensions/install \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"type": "wasm", "path": "/path/to/tool.wasm"}'

# Install an MCP server
curl -X POST http://localhost:3001/api/extensions/install \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"type": "mcp", "url": "https://mcp-server.example.com"}'
```

---

## Authenticating Extensions

Many extensions need credentials (API keys, OAuth tokens). After installing, authenticate the extension before using it.

### Via the Web UI

1. Select the extension in the Extensions tab
2. Click **Authenticate**
3. Follow the prompts:
   - **OAuth tools**: A browser window opens for you to log in to the service
   - **Manual token tools**: Enter your API key in the field provided

### Via the agent

> *"Authenticate the Telegram extension"*
> *"Set up auth for the Google Workspace tool"*

### OAuth Setup

For OAuth-based tools, you need to register a public OAuth application with the service (e.g. Notion, Google) and set environment variables for the client ID and client secret. The tool's documentation will tell you what's needed. Once configured, users just click through a standard browser login.

Redirect URIs to configure: `http://localhost:9876/callback` through `http://localhost:9886/callback`.

---

## Dynamic Tool Building

The agent can build new WASM tools on the fly when you describe what you need:

> *"Build me a tool that checks the status of my Kubernetes pods"*
> *"Create a tool that searches my company's internal Confluence wiki"*

The agent will:
1. Write the Rust source code for the tool
2. Compile it to WASM
3. Validate it
4. Register it in the tool registry

The newly built tool becomes available in the same session. You can authorize it to make HTTP requests or access secrets via its capabilities file.

---

## Viewing Installed Extensions

```bash
# List installed extensions
curl http://localhost:3001/api/extensions \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# List all registered tools (built-in + extensions)
curl http://localhost:3001/api/extensions/tools \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"
```

Or type `/tools` in chat for a quick list.

---

## Messaging Channels

Channels are a special type of WASM extension that let you receive messages from external services. Currently available:

| Channel | Guide |
|---------|-------|
| Telegram | [TELEGRAM_SETUP.md](TELEGRAM_SETUP.md) |
| Discord | [DISCORD_SETUP.md](DISCORD_SETUP.md) |
| Matrix | [MATRIX_SETUP.md](MATRIX_SETUP.md) |
| Slack | Set `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `SLACK_SIGNING_SECRET` in `.env` |
| HTTP Webhook | Set `HTTP_HOST`, `HTTP_PORT`, `HTTP_WEBHOOK_SECRET` in `.env` |
