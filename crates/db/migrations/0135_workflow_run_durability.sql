-- Crash-durable runs (WS4). `definition` snapshots the workflow JSON at
-- launch so a resume executes the definition the run STARTED with — never
-- the current one, which races owner edits and package sync (the 2026-08-24
-- revert would have swapped the definition under a resuming run).
-- `resume_attempted` is poison-run protection: one resume per run, ever —
-- a run that crashes the process again on resume is failed, not boot-looped.
ALTER TABLE workflow_runs ADD COLUMN definition TEXT;
ALTER TABLE workflow_runs ADD COLUMN resume_attempted INTEGER NOT NULL DEFAULT 0;
