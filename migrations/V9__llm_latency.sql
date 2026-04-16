-- Add latency_ms to llm_calls for per-model response time tracking.
ALTER TABLE llm_calls ADD COLUMN IF NOT EXISTS latency_ms BIGINT;
