-- What a schedule does when it comes due while its last fire, and the
-- workflow that fire started, is still going (Temporal's schedule overlap
-- policy): skip this occurrence (the default), buffer one to start when the
-- last ends, or allow every fire to start.
ALTER TABLE cron_jobs ADD COLUMN overlap_policy TEXT NOT NULL DEFAULT 'skip'
    CHECK (overlap_policy IN ('skip', 'buffer_one', 'allow_all'));
