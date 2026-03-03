-- +goose Up
-- Persistent task queue for agent recovery after restart
-- Tracks long-running tasks so they can be resumed if agent crashes/restarts

CREATE TABLE IF NOT EXISTS pending_tasks (
    id TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,           -- 'subagent', 'run', 'cron_agent'
    status TEXT NOT NULL DEFAULT 'pending',  -- 'pending', 'running', 'completed', 'failed', 'cancelled'

    -- Task definition (what to run)
    session_key TEXT NOT NULL,         -- Session to use for this task
    user_id TEXT,                      -- User who owns this task (for scoping)
    prompt TEXT NOT NULL,              -- The task/prompt to execute
    system_prompt TEXT,                -- Optional system prompt override

    -- Metadata
    description TEXT,                  -- Human-readable description
    lane TEXT DEFAULT 'main',          -- Which lane: main, cron, subagent
    priority INTEGER DEFAULT 0,        -- Higher = more urgent

    -- Execution tracking
    attempts INTEGER DEFAULT 0,        -- Number of execution attempts
    max_attempts INTEGER DEFAULT 3,    -- Max retries before marking failed
    last_error TEXT,                   -- Last error message if failed

    -- Timing
    created_at INTEGER NOT NULL,       -- Unix timestamp
    started_at INTEGER,                -- When execution started
    completed_at INTEGER,              -- When execution completed

    -- Parent task (for sub-agents spawned by other tasks)
    parent_task_id TEXT REFERENCES pending_tasks(id) ON DELETE SET NULL
);

-- Index for finding tasks to recover on startup
CREATE INDEX idx_pending_tasks_status ON pending_tasks(status);
CREATE INDEX idx_pending_tasks_lane ON pending_tasks(lane, status);
CREATE INDEX idx_pending_tasks_user ON pending_tasks(user_id, status);
CREATE INDEX idx_pending_tasks_parent ON pending_tasks(parent_task_id);

-- +goose Down
DROP INDEX IF EXISTS idx_pending_tasks_parent;
DROP INDEX IF EXISTS idx_pending_tasks_user;
DROP INDEX IF EXISTS idx_pending_tasks_lane;
DROP INDEX IF EXISTS idx_pending_tasks_status;
DROP TABLE IF EXISTS pending_tasks;
