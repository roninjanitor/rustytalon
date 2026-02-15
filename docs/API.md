# RustyTalon API Reference

The web gateway exposes an HTTP API on the configured `GATEWAY_HOST:GATEWAY_PORT` (default `127.0.0.1:3001`).

## Authentication

All protected endpoints require a Bearer token in the `Authorization` header:

```
Authorization: Bearer <GATEWAY_AUTH_TOKEN>
```

Set `GATEWAY_AUTH_TOKEN` in your environment or `.env` file. The `/api/health` endpoint does not require authentication.

---

## Public Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/health` | Health check (returns 200 OK) |
| GET | `/` | Serve web UI (index.html) |
| GET | `/style.css` | Serve CSS stylesheet |
| GET | `/app.js` | Serve JavaScript application |

---

## Chat

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/chat/send` | Send a message to the agent |
| POST | `/api/chat/approval` | Submit tool execution approval or denial |
| POST | `/api/chat/auth-token` | Submit auth token to extension manager |
| POST | `/api/chat/auth-cancel` | Cancel in-progress auth flow |
| GET | `/api/chat/events` | SSE stream of chat events |
| GET | `/api/chat/ws` | WebSocket upgrade for real-time chat |
| GET | `/api/chat/history` | Retrieve conversation history (paginated) |
| GET | `/api/chat/threads` | List all conversation threads |
| POST | `/api/chat/thread/new` | Create new conversation thread |

### Send a Message

```bash
curl -X POST http://localhost:3001/api/chat/send \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello, what can you do?"}'
```

### Stream Events (SSE)

```bash
curl -N http://localhost:3001/api/chat/events \
  -H "Authorization: Bearer $TOKEN"
```

Events are sent as Server-Sent Events with the following types:
- `thinking` - Agent reasoning/planning
- `message` - Text response from the agent
- `tool_call` - Tool invocation request
- `tool_result` - Tool execution result
- `approval_request` - Tool requires user approval
- `auth_request` - Extension needs authentication
- `error` - Error occurred
- `job_update` - Sandbox job status change

### WebSocket

Connect to `/api/chat/ws` for bidirectional real-time communication. Send JSON messages, receive the same event types as SSE.

---

## Memory (Workspace)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/memory/tree` | Get workspace directory tree |
| GET | `/api/memory/list` | List files in a directory |
| GET | `/api/memory/read` | Read file content |
| POST | `/api/memory/write` | Write or create a file |
| POST | `/api/memory/search` | Hybrid search (FTS + vector) |

### Search Memory

```bash
curl -X POST http://localhost:3001/api/memory/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "user preferences", "limit": 5}'
```

### Read a File

```bash
curl "http://localhost:3001/api/memory/read?path=MEMORY.md" \
  -H "Authorization: Bearer $TOKEN"
```

### Write a File

```bash
curl -X POST http://localhost:3001/api/memory/write \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path": "notes/todo.md", "content": "# My Notes\n\n- Item 1"}'
```

---

## Jobs (Sandbox)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/jobs` | List all sandbox jobs |
| GET | `/api/jobs/summary` | Job count summary (total, pending, in_progress, completed, failed) |
| GET | `/api/jobs/{id}` | Get job details |
| POST | `/api/jobs/{id}/cancel` | Cancel a running job |
| POST | `/api/jobs/{id}/restart` | Restart a failed job |
| POST | `/api/jobs/{id}/prompt` | Send follow-up prompt to Claude Code sandbox |
| GET | `/api/jobs/{id}/events` | Get persisted job events |
| GET | `/api/jobs/{id}/files/list` | List files in job project directory |
| GET | `/api/jobs/{id}/files/read` | Read file from job project |

### List Jobs

```bash
curl http://localhost:3001/api/jobs \
  -H "Authorization: Bearer $TOKEN"
```

### Cancel a Job

```bash
curl -X POST http://localhost:3001/api/jobs/abc123/cancel \
  -H "Authorization: Bearer $TOKEN"
```

---

## Extensions

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/extensions` | List installed extensions |
| GET | `/api/extensions/tools` | List all registered tools (builtin + extensions) |
| POST | `/api/extensions/install` | Install new extension (MCP or WASM) |
| POST | `/api/extensions/{name}/activate` | Activate an installed extension |
| POST | `/api/extensions/{name}/remove` | Remove an installed extension |

### Install an Extension

```bash
# Install an MCP server
curl -X POST http://localhost:3001/api/extensions/install \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"type": "mcp", "url": "https://mcp-server.example.com"}'

# Install a WASM tool
curl -X POST http://localhost:3001/api/extensions/install \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"type": "wasm", "path": "/path/to/tool.wasm"}'
```

---

## Routines

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/routines` | List all routines |
| GET | `/api/routines/summary` | Routine summary (total, enabled, disabled, failing, runs_today) |
| GET | `/api/routines/{id}` | Get routine details |
| POST | `/api/routines/{id}/trigger` | Manually trigger a routine |
| POST | `/api/routines/{id}/toggle` | Enable or disable a routine |
| DELETE | `/api/routines/{id}` | Delete a routine |
| GET | `/api/routines/{id}/runs` | Get execution history (last 50 runs) |

### Trigger a Routine

```bash
curl -X POST http://localhost:3001/api/routines/routine-id/trigger \
  -H "Authorization: Bearer $TOKEN"
```

---

## Settings

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/settings` | List all user settings |
| GET | `/api/settings/{key}` | Get a specific setting |
| PUT | `/api/settings/{key}` | Create or update a setting |
| DELETE | `/api/settings/{key}` | Delete a setting |
| GET | `/api/settings/export` | Export all settings as JSON |
| POST | `/api/settings/import` | Import settings from JSON |

### Set a Setting

```bash
curl -X PUT http://localhost:3001/api/settings/theme \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"value": "dark"}'
```

---

## Providers (LLM Monitoring)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/providers/health` | Get provider health status from smart router |
| GET | `/api/providers/costs` | Get LLM usage and cost statistics |

### Check Provider Health

```bash
curl http://localhost:3001/api/providers/health \
  -H "Authorization: Bearer $TOKEN"
```

Response includes per-provider health scores, failure counts, and availability status.

### Get Cost Statistics

```bash
curl http://localhost:3001/api/providers/costs \
  -H "Authorization: Bearer $TOKEN"
```

---

## OpenAI-Compatible API

RustyTalon exposes an OpenAI-compatible endpoint for drop-in integration with tools that support the OpenAI API format.

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/chat/completions` | Chat completions (OpenAI format) |
| GET | `/v1/models` | List available models |

### Chat Completions

```bash
curl -X POST http://localhost:3001/v1/chat/completions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "Hello"}]
  }'
```

---

## Logs

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/logs/events` | SSE stream of system logs with history replay |

### Stream Logs

```bash
curl -N http://localhost:3001/api/logs/events \
  -H "Authorization: Bearer $TOKEN"
```

---

## Gateway Status

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/gateway/status` | Connection counts (SSE, WebSocket, total) |

---

## Project Files

Serve files from sandbox job project directories.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/projects/{project_id}/` | Serve project index.html |
| GET | `/projects/{project_id}/{path}` | Serve any file under project directory |

Path traversal protection is enforced.

---

## Orchestrator API (Internal)

The orchestrator runs on a separate port and is used by worker containers. These endpoints are not intended for external use.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check (no auth) |
| GET | `/worker/{job_id}/job` | Get job description |
| POST | `/worker/{job_id}/llm/complete` | Proxy LLM completion |
| POST | `/worker/{job_id}/llm/complete_with_tools` | Proxy LLM tool-use completion |
| POST | `/worker/{job_id}/status` | Report job status |
| POST | `/worker/{job_id}/complete` | Report job completion |
| POST | `/worker/{job_id}/event` | Send job event |
| GET | `/worker/{job_id}/prompt` | Get queued follow-up prompt |

Worker endpoints use per-job bearer token authentication.
