-- V10: Audit log table for production audit logging.
--
-- Records tool calls, approval decisions, safety blocks, and job-state
-- transitions so operators can trace what the agent did and why.

CREATE TABLE audit_log (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    event_type    TEXT NOT NULL,        -- 'tool_call','approval','safety_block','job_state','auth'
    user_id       TEXT,
    session_id    UUID,                 -- conversation / thread id
    job_id        UUID,
    actor         TEXT,                 -- 'agent','user','system'
    tool_name     TEXT,                 -- for tool_call events
    input_hash    TEXT,                 -- SHA-256 of stringified input params
    input_summary TEXT,                 -- first 500 chars of input, redacted
    outcome       TEXT,                 -- 'success','failure','blocked','approved','denied'
    error_msg     TEXT,
    duration_ms   BIGINT,
    cost_usd      TEXT,
    metadata      JSONB
);

CREATE INDEX audit_log_created_at    ON audit_log (created_at DESC);
CREATE INDEX audit_log_user_session  ON audit_log (user_id, session_id);
CREATE INDEX audit_log_event_type    ON audit_log (event_type);
