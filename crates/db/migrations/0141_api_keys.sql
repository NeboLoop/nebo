-- API keys: the OpenAI-shaped door. A key names the models it may call —
-- an employee ("employee/<agent id>") or a workflow ("workflow/<agent id>/<name>")
-- — and the tools a run it starts may touch. The raw key is shown once; only
-- its hash is kept. Revoked keys stay for the audit trail.
CREATE TABLE IF NOT EXISTS api_keys (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    key_prefix   TEXT NOT NULL,
    agent_id     TEXT NOT NULL,
    models       TEXT NOT NULL DEFAULT '[]',   -- JSON array of model ids
    tools        TEXT NOT NULL DEFAULT '[]',   -- JSON array of tool allowlist entries
    created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
    last_used_at INTEGER,
    revoked_at   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_api_keys_agent ON api_keys(agent_id);
