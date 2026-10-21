-- The Workforce dashboard (GET /dashboard, which the mobile app polls every
-- 15 s) read most of the database on every call.
--
-- Its 14-day count of the owner's chat turns filters chat_messages by role
-- and created_at and joins on chat_id. The only index it could use was
-- created_at alone, so every message in the window was fetched from the
-- table for its role and chat: about one page per message, since content,
-- metadata and tool output share the row. On the owner's desktop
-- (2026-10-21) that was 22k page reads for 58k messages; on a cloud bot,
-- 2.5-3.7 s and ~79 MB per call. With (created_at, role, chat_id) the count
-- reads the window from the index alone and never touches a message row.
-- created_at leads, not role: with role first, every read of one chat's
-- messages by role (`chat_id = ? AND role = ?`) took this index, which then
-- covered the count, over the chat's own (chat_id, created_at) index, and
-- walked every message of that role in every chat.
--
-- The prompt-schedule fires it lists (task runs whose external_ref is a
-- `cron:` job) were read through the state index: every ended run of every
-- kind, definition and inputs included. (external_ref, kind, state) seeks
-- only the schedules' ended runs. external_ref leads so a read that narrows
-- by kind and a session-key range keeps its session index.
--
-- The recent workflow runs and their per-day counts filter workflow_runs by
-- started_at, which had no index: a full scan that grows with every run.
-- +goose Up
CREATE INDEX IF NOT EXISTS idx_chat_messages_time_role ON chat_messages(created_at, role, chat_id);
CREATE INDEX IF NOT EXISTS idx_engine_runs_ref_kind_state ON engine_runs(external_ref, kind, state);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_started ON workflow_runs(started_at);

-- +goose Down
DROP INDEX IF EXISTS idx_workflow_runs_started;
DROP INDEX IF EXISTS idx_engine_runs_ref_kind_state;
DROP INDEX IF EXISTS idx_chat_messages_time_role;
