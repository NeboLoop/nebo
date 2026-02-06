-- +goose Up
-- Cron jobs table for scheduled tasks
CREATE TABLE IF NOT EXISTS cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    schedule TEXT NOT NULL,
    command TEXT DEFAULT '',
    task_type TEXT DEFAULT 'bash',
    message TEXT DEFAULT '',
    deliver TEXT DEFAULT '',
    enabled INTEGER DEFAULT 1,
    last_run DATETIME,
    run_count INTEGER DEFAULT 0,
    last_error TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Cron execution history
CREATE TABLE IF NOT EXISTS cron_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    started_at DATETIME NOT NULL,
    finished_at DATETIME,
    success INTEGER DEFAULT 0,
    output TEXT,
    error TEXT,
    FOREIGN KEY (job_id) REFERENCES cron_jobs(id) ON DELETE CASCADE
);

CREATE INDEX idx_cron_history_job_id ON cron_history(job_id);
CREATE INDEX idx_cron_history_started_at ON cron_history(started_at);

-- +goose Down
DROP TABLE IF EXISTS cron_history;
DROP TABLE IF EXISTS cron_jobs;
