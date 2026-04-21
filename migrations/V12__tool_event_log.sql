-- Lightweight event log for tool calls originating from chat (non-job) flows.
-- job_actions requires a FK to agent_jobs, so chat-mode tool calls are recorded here.
CREATE TABLE IF NOT EXISTS tool_event_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tool_name TEXT NOT NULL,
    success BOOLEAN NOT NULL,
    duration_ms BIGINT,
    cost NUMERIC(20, 10) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tool_event_log_tool_name ON tool_event_log(tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_event_log_created_at ON tool_event_log(created_at);
