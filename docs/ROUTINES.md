# Routines Guide

Routines let RustyTalon run tasks automatically — on a schedule, in response to events, triggered by a webhook, or on demand. They're the primary way to set up proactive automations without having to initiate every interaction yourself.

---

## Routine Types

| Type | When it runs |
|------|-------------|
| **Cron** | On a time-based schedule (e.g. every morning at 9am) |
| **Event** | When a named internal event fires (e.g. job completed) |
| **Webhook** | When an HTTP POST arrives at the routine's endpoint |
| **Manual** | Only when you explicitly trigger it |

---

## Creating Routines

### Via the Web UI

1. Open the **Routines** tab
2. Click **New Routine**
3. Fill in:
   - **Name** — A short label
   - **Description** — What this routine does (shown in history)
   - **Trigger** — Choose Cron, Event, Webhook, or Manual
   - **Prompt** — The message the agent receives when the routine fires
   - **Guardrails** (optional) — Max run time, allowed tools, max cost
4. Click **Save**
5. Toggle it **Enabled** to activate

### Via the Agent

You can also ask the agent to create routines in chat:

> *"Create a cron routine that runs every Monday at 9am and checks my MEMORY.md for any pending items"*
>
> *"Set up a routine that fires when a job fails and sends me a summary of what went wrong"*

---

## Cron Schedules

Use standard cron syntax (5 fields: minute, hour, day-of-month, month, day-of-week):

| Expression | Meaning |
|-----------|---------|
| `0 9 * * 1` | Every Monday at 9:00am |
| `0 9 * * 1-5` | Weekdays at 9:00am |
| `0 * * * *` | Every hour |
| `*/30 * * * *` | Every 30 minutes |
| `0 9,17 * * *` | At 9am and 5pm daily |
| `0 0 * * *` | Midnight every day |
| `0 0 1 * *` | First day of every month |

The cron engine checks for due routines on the interval set by `ROUTINES_CRON_INTERVAL` (default: 60 seconds).

---

## Event Triggers

Event routines fire when a named internal event occurs. Available events:

| Event Name | Fires When |
|-----------|-----------|
| `job.completed` | A sandbox job finishes successfully |
| `job.failed` | A sandbox job fails |
| `job.stuck` | A job is detected as hung |
| `heartbeat` | A heartbeat check runs |
| `message.received` | A new message arrives on any channel |

---

## Routine Prompts

The prompt is what the agent receives as its task when the routine fires. Write it as you would write a message in chat.

**Good prompts are specific:**

```
Check MEMORY.md for any items marked "TODO" and send me a summary.
Do not make any changes — just report what you find.
```

```
Review today's daily log at daily/{date}.md.
If it's missing, create it with a brief summary of what I might have worked on based on recent memory.
```

**Use guardrails for safety:**

- **Max run time** — Prevents runaway routines
- **Allowed tools** — Restrict which tools the routine can use
- **Max cost** — Cap LLM spend per run

---

## Managing Routines

### Web UI

- **Toggle** — Enable/disable without deleting
- **Trigger** — Run immediately regardless of schedule
- **History** — View the last 50 runs with status, duration, and output
- **Delete** — Remove permanently

### API

```bash
# List all routines
curl http://localhost:3001/api/routines \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# Get a specific routine
curl http://localhost:3001/api/routines/<id> \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# Trigger immediately
curl -X POST http://localhost:3001/api/routines/<id>/trigger \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# Enable/disable
curl -X POST http://localhost:3001/api/routines/<id>/toggle \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# View run history
curl http://localhost:3001/api/routines/<id>/runs \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"

# Delete
curl -X DELETE http://localhost:3001/api/routines/<id> \
  -H "Authorization: Bearer $GATEWAY_AUTH_TOKEN"
```

---

## Heartbeat vs. Routines

RustyTalon has two systems for proactive execution:

| | Heartbeat | Routines |
|-|-----------|---------|
| **Purpose** | Single recurring agent check-in | Multiple independent automations |
| **Config** | `HEARTBEAT_ENABLED`, `HEARTBEAT_INTERVAL_SECS` | Created per-routine in UI or via agent |
| **Prompt source** | Reads `HEARTBEAT.md` from workspace | Each routine has its own prompt |
| **Notification** | Only notifies if findings; silent if `HEARTBEAT_OK` | Always creates a job and logs output |
| **Use for** | Monitoring, catching things you'd forget | Specific recurring tasks |

You can use both simultaneously. Heartbeat is good for *"check if anything needs attention"*; routines are good for *"do this specific thing every Tuesday"*.

---

## Example Routines

### Daily standup summary

- **Type:** Cron
- **Schedule:** `0 9 * * 1-5` (weekdays at 9am)
- **Prompt:**
  ```
  Read today's priorities from context/priorities.md and yesterday's daily log.
  Write a brief standup summary (what I worked on, what's next, any blockers) to daily/{today}.md.
  Keep it under 200 words.
  ```

### Weekly memory review

- **Type:** Cron
- **Schedule:** `0 10 * * 1` (Monday at 10am)
- **Prompt:**
  ```
  Review MEMORY.md and the last 7 daily logs.
  Summarize key themes and suggest any items to add or remove from MEMORY.md.
  Do not modify any files — just report your findings.
  ```

### Job failure alert

- **Type:** Event — `job.failed`
- **Prompt:**
  ```
  A background job has just failed. Read the job's event log and write a one-paragraph summary
  of what failed and why to context/job-failures.md. Append, do not overwrite.
  ```

### On-demand research task

- **Type:** Manual
- **Prompt:**
  ```
  Search memory for any notes about competitor analysis and summarize what we know.
  Then search the web for recent news about the top 3 competitors and add any notable findings
  to projects/research/competitors.md.
  ```
