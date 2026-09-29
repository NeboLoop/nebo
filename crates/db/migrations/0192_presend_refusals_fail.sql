-- Sends Nebo refused before the plugin ran were filed as "outcome unknown"
-- and held: their ledger rows stayed pending forever, the engine logged
-- them every tick, and the owner was told to check his sent items for mail
-- that never left (live 2026-09-26: three workflow emails from an employee
-- with no gmail connected). The send path now fails such a send at once;
-- this fails the rows it held before.
--
-- A row is failed only on its recorded refusal: the notice the send path
-- wrote for it (`attention:effect:<id>`) quotes what came back, and the
-- refusals matched here are Nebo's own words, returned before any plugin
-- process starts. A row whose plugin ran (a timeout, a failure after
-- launch, output with no typed outcome) matches none of them and stays for
-- the owner to answer. The false notices go.
-- +goose Up
CREATE TEMP TABLE presend_refused AS
SELECT CAST(substr(n.id, 18) AS INTEGER) AS effect_id,
       substr(n.body,
              instr(n.body, 'the plugin reported no typed outcome: ') + 38,
              CASE WHEN instr(n.body, '. It was not retried') > 0
                   THEN instr(n.body, '. It was not retried') - instr(n.body, 'the plugin reported no typed outcome: ') - 38
                   ELSE length(n.body) END) AS refusal,
       n.id AS notice_id
FROM notifications n
WHERE n.id LIKE 'attention:effect:%'
  AND (
       n.body LIKE '%the plugin reported no typed outcome: No % account is connected for this agent. Connect one in this agent''s Settings, Plugins before using %'
    OR n.body LIKE '%the plugin reported no typed outcome: No % account named "%" for this agent. Connected %'
    OR n.body LIKE '%the plugin reported no typed outcome: The % account didn''t finish connecting. No % account is connected for this agent.%'
    OR n.body LIKE '%the plugin reported no typed outcome: Plugin ''%'' not found. Available: %'
    OR n.body LIKE '%the plugin reported no typed outcome: Could not parse command ''%'' (unbalanced quotes).%'
    OR n.body LIKE '%the plugin reported no typed outcome: `%` is a shell operator and `%` runs directly, with no shell%'
    OR n.body LIKE '%the plugin reported no typed outcome: I can''t sign in to or re-authenticate % on my own%'
    OR n.body LIKE '%the plugin reported no typed outcome: command is required: the subcommand and flags%'
  );

UPDATE engine_effects
SET state = 'failed',
    result = 'Not sent: Nebo refused it before the plugin ran. ' || (SELECT p.refusal FROM presend_refused p WHERE p.effect_id = engine_effects.id),
    completed_at = unixepoch()
WHERE state = 'pending'
  AND class = 'messaging'
  AND id IN (SELECT effect_id FROM presend_refused);

DELETE FROM notifications
WHERE id IN (
    SELECT p.notice_id FROM presend_refused p
    JOIN engine_effects e ON e.id = p.effect_id
    WHERE e.state = 'failed' AND e.result LIKE 'Not sent: Nebo refused it before the plugin ran.%'
);

DROP TABLE presend_refused;
