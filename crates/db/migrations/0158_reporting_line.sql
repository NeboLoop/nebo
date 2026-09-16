-- +goose Up
-- Structure on an employee: the part of the company it sits in, and the one
-- seat it answers to. Both the owner's to set, both optional — a company of
-- three employees has no hierarchy and must not be made to invent one.
--
-- `department` is not new (0119): a package declares one and the install writes
-- it. What was missing is the owner's claim on it. `department_locked` is the
-- contract `name_locked` (0132) already gave a packaged employee's name: once
-- the owner sets the department it is theirs, and a later package sync can
-- never move it back. Rows that already carry a department got it from a
-- package, not from the owner, so they stay unlocked and the package keeps it.
--
-- `reports_to` is a local agent id, NULL when the seat answers to the owner —
-- which is every seat until the owner draws a line. No foreign key on purpose:
-- an employee can be deleted while a report still names it (delete_agent
-- cleans up with no cascade), and a manager id that no longer resolves reads
-- as "answers to the owner" instead of failing the delete.
ALTER TABLE agents ADD COLUMN reports_to TEXT;
ALTER TABLE agents ADD COLUMN department_locked INTEGER NOT NULL DEFAULT 0;

-- Read on the manager's side ("who answers to me") by the org view and by
-- every escalation that walks up the line.
CREATE INDEX IF NOT EXISTS idx_agents_reports_to ON agents(reports_to);

-- +goose Down
DROP INDEX IF EXISTS idx_agents_reports_to;
ALTER TABLE agents DROP COLUMN department_locked;
ALTER TABLE agents DROP COLUMN reports_to;
