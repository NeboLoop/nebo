-- +goose Up
-- Per-scope memory write activity for the consolidation gate (Phase 2,
-- docs/plans/memory-rock-solid.md). One row per memory user_id scope.
-- write_events counts distinct write BURSTS since the scope's last
-- consolidation: writes closer together than the burst gap coalesce into a
-- single event, so one extraction flush of many facts counts once. The
-- curator gates on this activity signal, not on scope size — a 5-memory
-- case scope that is actively written deserves curation as much as a
-- 200-memory one.
CREATE TABLE IF NOT EXISTS memory_scope_activity (
    user_id TEXT PRIMARY KEY,
    write_events INTEGER NOT NULL DEFAULT 0,
    last_write_at DATETIME,
    last_consolidated_at DATETIME
);
