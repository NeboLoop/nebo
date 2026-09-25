-- The one loop. Every turn runs through the harness; the old loop's stored
-- shapes are converted here, once, and nothing reads them afterwards.
--
-- 1. Steering the old loop wrote into threads as user rows: the
--    auto-continue nudge, the budget-exhausted summary request, room
--    briefings and `<system-reminder>` rows that are neither a typed
--    attachment nor a notification. The model never loaded them; now they
--    are gone and the loader keeps everything it reads.
DELETE FROM chat_messages
WHERE role = 'user'
  AND (
    ltrim(content) LIKE 'Continue — your previous response committed to more work that isn''t done yet:%'
    OR content = 'You''ve reached the maximum number of tool-calling iterations allowed. Please provide a final response summarizing what you''ve found and accomplished so far, without calling any more tools.'
    OR (CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.autoContinue') END) = 1
    OR (CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.roomBriefing') END) = 1
    OR (
      ltrim(content) LIKE '<system-reminder>%'
      AND (CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.isMeta') END) = 1
      AND (CASE WHEN json_valid(metadata) THEN json_type(metadata, '$.attachment') END) IS NULL
      AND (CASE WHEN json_valid(metadata) THEN json_type(metadata, '$.notification') END) IS NULL
    )
  );

-- 2. The orchestrator's sub-agent and graph tasks become helper rows. One
--    still live belonged to a process that is gone: it failed, and a
--    finished one can be continued with send_message like any helper.
UPDATE engine_runs
SET state = 'failed',
    error = COALESCE(error, 'stopped by the upgrade'),
    ended_at = unixepoch()
WHERE kind IN ('subagent', 'dag')
  AND state IN ('queued', 'running', 'waiting', 'interrupted');
UPDATE engine_runs SET kind = 'helper' WHERE kind IN ('subagent', 'dag');

-- 3. The old loop's session state: the objective, the rolling summary (its
--    text became a checkpoint row in 0168) and the work-task snapshot.
ALTER TABLE sessions DROP COLUMN active_task;
ALTER TABLE sessions DROP COLUMN summary;
ALTER TABLE sessions DROP COLUMN last_summarized_count;
ALTER TABLE sessions DROP COLUMN work_tasks;
