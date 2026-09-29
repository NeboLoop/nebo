-- What an employee stands on that only the owner supplies, and the one
-- Inbox item that told them.
--
-- agent_workflows.need_told kept one told need per BINDING, under whatever
-- key the source that noticed it used, and forgot it whenever a run of the
-- binding completed. The same missing thing, noticed by pre-flight
-- ("needs a telephony plugin"), by a blocked run ("account:phonecall") and by
-- heartbeat triage ("judged:capability:telephony") on the several duties of
-- one employee, overwrote itself and was told again every ninety minutes,
-- to the Inbox, the phone and the owner's email (live 2026-09-27/28).
--
-- owner_needs: one row per employee per need, under the ONE key every
-- source's need reduces to (`capability:<name>`, `plugin:<slug>`,
-- `account:<slug>`, or `something` when nobody can name it). A need with a
-- row is not news, from any duty and any source; a duty newly held on it
-- joins `duties` and the item's text is restated. notice_id is the item that
-- told it (the owner may have dismissed it). basis is what was installed
-- and connected for the employee when it was told. The row goes when the
-- need is met (its check passes, and something was installed or connected
-- since), and the item is resolved with it; a need that returns after that
-- is told once more.
-- +goose Up
CREATE TABLE owner_needs (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    need_key TEXT NOT NULL,
    notice_id TEXT NOT NULL,
    duties TEXT NOT NULL DEFAULT '[]',
    basis TEXT NOT NULL DEFAULT '[]',
    told_at INTEGER NOT NULL,
    PRIMARY KEY (agent_id, need_key)
);
ALTER TABLE agent_workflows DROP COLUMN need_told;

-- +goose Down
ALTER TABLE agent_workflows ADD COLUMN need_told TEXT;
DROP TABLE owner_needs;
