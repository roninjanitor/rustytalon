# Memory & Workspace Guide

RustyTalon gives the agent a persistent memory system — a flexible filesystem of documents it can read and write across sessions. This is how the agent remembers your preferences, tracks ongoing projects, keeps daily logs, and stays informed about your world.

---

## How Memory Works

Memory is stored in a "workspace" — a virtual filesystem backed by the database. Every file has a path (like `projects/alpha/notes.md`), content (plain text or Markdown), and is automatically indexed for search.

The agent reads from memory at the start of sessions and writes to it as things come up. You can also read, write, and search memory directly via the web UI or API.

**Key principle:** Memory is explicit. The agent doesn't passively absorb everything you say — it writes to memory when you ask it to, or when it determines something is worth keeping. You can always tell it: *"Remember that I prefer dark mode"* or *"Write a note about this to projects/alpha/notes.md"*.

---

## Well-Known Files

These files have special meaning and are loaded into the agent's context automatically:

| File | Purpose |
|------|---------|
| `README.md` | Root index / runbook for the workspace |
| `MEMORY.md` | Long-term curated facts about you and your work |
| `AGENTS.md` | Behavior instructions for the agent (what to do, how to behave) |
| `USER.md` | Information about you — role, preferences, context |
| `SOUL.md` | Agent's core values and guiding principles |
| `IDENTITY.md` | Agent's name, personality, and self-concept |
| `HEARTBEAT.md` | Checklist the agent runs during periodic heartbeat checks |

You can create and edit any of these via the Memory tab in the web UI or by asking the agent directly.

---

## Directory Structure

The workspace supports any structure you find useful. Common conventions:

```
workspace/
├── MEMORY.md          <- Curated long-term memory
├── AGENTS.md          <- Agent behavior rules
├── USER.md            <- About you
├── IDENTITY.md        <- Agent identity
├── SOUL.md            <- Agent values
├── HEARTBEAT.md       <- Periodic checklist
├── context/           <- Extended context docs
│   ├── vision.md
│   └── priorities.md
├── daily/             <- Daily logs
│   ├── 2024-01-15.md
│   └── 2024-01-16.md
└── projects/          <- Project-specific notes
    └── alpha/
        ├── README.md
        └── notes.md
```

You can create any directories and files that make sense for your use.

---

## Common Memory Operations

### Ask the agent to remember something

In chat, just ask naturally:

> *"Remember that my preferred time zone is US/Eastern"*
> *"Add a note to MEMORY.md that I'm working on Project Alpha through Q2"*
> *"Write today's session summary to daily/2024-01-15.md"*

### Ask the agent to recall something

> *"What do you know about my preferences?"*
> *"Search memory for anything about Project Alpha"*
> *"What did we work on last week?"*

The agent searches memory using hybrid full-text + semantic search before answering questions about prior work.

### Read or edit memory yourself

Use the **Memory tab** in the web UI to browse, read, and write files directly without going through the agent.

---

## Writing Memory via API

```bash
# Write a file
curl -X POST http://localhost:3001/api/memory/write \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path": "projects/alpha/notes.md", "content": "# Project Alpha\n\nKey decision: use PostgreSQL."}'

# Read a file
curl "http://localhost:3001/api/memory/read?path=MEMORY.md" \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# List a directory
curl "http://localhost:3001/api/memory/list?path=projects/" \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# View full tree
curl http://localhost:3001/api/memory/tree \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"
```

---

## Searching Memory

Search uses a hybrid algorithm combining keyword (full-text) and semantic (meaning) matching. Results from both methods are combined using Reciprocal Rank Fusion — documents that appear in both get boosted scores.

```bash
curl -X POST http://localhost:3001/api/memory/search \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "project deadlines Q1", "limit": 5}'
```

> **Note:** Semantic search requires `EMBEDDING_ENABLED=true` and an OpenAI API key for embeddings. Without it, only keyword search is used.

---

## Heartbeat Memory

The `HEARTBEAT.md` file is a special checklist the agent runs during periodic heartbeat checks (every 30 minutes by default, if enabled). It's a good place to put things you want the agent to monitor proactively.

Example `HEARTBEAT.md`:

```markdown
# Heartbeat Checklist

- Check if any daily log is missing for the past 7 days
- Review MEMORY.md for anything that needs follow-up
- Check for any overdue reminders in context/priorities.md
- Summarize any new projects/ files added since last heartbeat
```

During a heartbeat run, the agent reads this checklist, acts on it, and notifies you if it finds anything worth your attention. If everything is normal, it replies `HEARTBEAT_OK` internally and does not send a notification.

Trigger a manual heartbeat anytime by typing `/heartbeat` in chat.

---

## Agent Behavior Files

### AGENTS.md

Instructions for how the agent should behave. Write rules in plain language:

```markdown
# Agent Behavior

- Always search memory before answering questions about past work
- When I say "remember this", write it to MEMORY.md immediately
- Keep daily logs concise — bullet points, not paragraphs
- Ask before making any changes to files outside the workspace
```

### USER.md

Context about you that helps the agent tailor its responses:

```markdown
# About Me

- Software engineer, 10 years experience with Python and Go
- Currently working on Project Alpha (internal API rewrite)
- Prefer concise responses; skip explanations I'd already know
- Time zone: US/Eastern
```

### SOUL.md and IDENTITY.md

These define the agent's personality. You can customize them if you want the agent to have a specific name, tone, or set of values. Edit them via the Memory tab or ask the agent to update them.

---

## Tips

- **Be explicit.** The agent won't remember something unless it writes it down. When something matters, say *"make a note of this"*.
- **Use paths intentionally.** Group related notes under `projects/<name>/` so the agent can find them together.
- **Daily logs are automatic (if you ask).** Tell the agent to keep a daily log and it will write session summaries to `daily/YYYY-MM-DD.md`.
- **Search before asking.** The agent is prompted to search memory before answering questions about prior sessions, but you can also search manually from the Memory tab.
