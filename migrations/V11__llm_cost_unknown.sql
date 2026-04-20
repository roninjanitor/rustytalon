-- Track whether a cost rate was known at recording time.
-- False (default) means a configured or built-in rate was used.
-- True means the fallback default rate was used and cost data may be inaccurate.
ALTER TABLE llm_calls ADD COLUMN IF NOT EXISTS cost_unknown BOOLEAN NOT NULL DEFAULT FALSE;
