-- The workflow audit (`napp::workflow_check::audit_workflow`).
--
-- workflow_audits: one row per employee workflow, the last audit of it.
-- definition_hash is the hash of the definition the audit left; a run
-- whose definition hashes the same needs no audit (the pre-run check),
-- and '' marks the row stale. report is what the audit added, fixed and
-- still finds wrong; healed is every tool a step called without declaring
-- it, kept across audits.
--
-- workflow_step_picks: the tools the embedding shortlist and the model
-- picked for an open-ended step, keyed by the step's words and the tool
-- catalog (`napp::workflow_check::step_key`), so the same step never costs
-- a second model call.
--
-- A workflow step no longer pauses for a tool it doesn't declare: an open
-- card from that pause is an ordinary ask now, answered the same way.
-- +goose Up
CREATE TABLE IF NOT EXISTS workflow_audits (
    agent_id        TEXT    NOT NULL,
    binding_name    TEXT    NOT NULL,
    definition_hash TEXT    NOT NULL DEFAULT '',
    report          TEXT    NOT NULL DEFAULT '{}',
    healed          TEXT    NOT NULL DEFAULT '[]',
    audited_at      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, binding_name)
);

CREATE TABLE IF NOT EXISTS workflow_step_picks (
    step_key   TEXT    PRIMARY KEY,
    tools      TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);

UPDATE permission_asks SET ask_case = '{"kind":"ask_mode"}'
 WHERE status = 'open' AND json_extract(ask_case, '$.kind') = 'step_tool';

-- +goose Down
DROP TABLE IF EXISTS workflow_step_picks;
DROP TABLE IF EXISTS workflow_audits;
