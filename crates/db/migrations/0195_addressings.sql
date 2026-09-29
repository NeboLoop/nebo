-- Who was addressed, and whether they have answered: the ONE routing record
-- for team posts and messages between employees (owner rule 2026-09-28: an
-- employee speaks only when it is addressed, and it answers each addressing
-- once).
--
-- Live 2026-09-29: the owner told a team lead to stop everybody; the lead's
-- @everyone was answered by every member, each answer woke the lead, the
-- lead posted again, each post woke the members, and ten deliveries went by
-- in four minutes. A reply woke whoever asked, every time it came, and a
-- turn a reply woke could reach its asker again: nothing recorded that the
-- question had been answered already.
--
-- One row per (post, employee it addressed). post_id is the team post's row
-- id, or the message id of a message between employees. seat_session is the
-- conversation the addressed employee answers in. asker_session is
-- the conversation that asked ('' = the owner in a team thread, who reads
-- the thread); asker_agent the employee that asked ('' = the owner, or the
-- main employee). state: asked -> answered; a seat whose own asks have all
-- been answered is 'collected' (its next answer is final and asks no one).
-- The asker hears every answer once, together, when the last one it is
-- waiting for comes in (reported marks the answers it has heard).
-- +goose Up
CREATE TABLE addressings (
    post_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    team_id TEXT NOT NULL DEFAULT '',
    seat_session TEXT NOT NULL,
    asker_session TEXT NOT NULL DEFAULT '',
    asker_agent TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'asked',
    answer TEXT NOT NULL DEFAULT '',
    provenance TEXT NOT NULL DEFAULT '[]',
    handoff_depth INTEGER NOT NULL DEFAULT 0,
    reported INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    answered_at INTEGER,
    PRIMARY KEY (post_id, agent_id)
);
CREATE INDEX idx_addressings_asker ON addressings(asker_session, state);
CREATE INDEX idx_addressings_seat ON addressings(seat_session, state);

-- +goose Down
DROP INDEX idx_addressings_seat;
DROP INDEX idx_addressings_asker;
DROP TABLE addressings;
