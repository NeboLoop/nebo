-- Every engine tick (5 s) lists the enabled schedules, and each job's row
-- carries its last run, run count and last error: three subqueries over
-- engine_runs by `external_ref = 'cron:<id>'`. With no index on that column
-- each one was a full scan of engine_runs — whose `definition` and `inputs`
-- come before `external_ref` and spill to overflow pages, so every row read
-- them from disk. On a long-used desktop (2026-10-09) one tick outlasted its
-- interval, the engine scanned without pause, and with the other hot
-- queries it held SQLite's shared page cache until the server stopped
-- answering. With this index each subquery is a seek.
-- +goose Up
CREATE INDEX IF NOT EXISTS idx_engine_runs_external_ref ON engine_runs(external_ref, created_at);

-- +goose Down
DROP INDEX IF EXISTS idx_engine_runs_external_ref;
