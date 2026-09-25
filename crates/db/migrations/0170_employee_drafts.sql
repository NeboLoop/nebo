-- Consent to a job (Turn-Controller-Technical-Design §2.12.6-2.12.7).
--
-- employee_drafts: a drafted employee, or a drafted edit to one, whose needs
-- were worked out and shown to the owner as one plain line. The create (or
-- edit) call that names the draft grants those needs only when an owner
-- message arrived in that chat after the line was shown. kind is create |
-- edit; agent_id is the employee an edit changes ('' for a create). input is
-- the call's input as drafted, so what runs is what the owner saw. needs is a
-- tools::needs::Needs JSON. status is open | used.
--
-- employee_ceilings: an employee made by an employee, still waiting on the
-- owner's card for the needs beyond its creator's grant. While a row stands
-- the employee works under its creator's grant (Ceiling::Creator).
-- +goose Up
CREATE TABLE IF NOT EXISTS employee_drafts (
    id          TEXT    PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('create', 'edit')),
    agent_id    TEXT    NOT NULL DEFAULT '',
    creator_id  TEXT    NOT NULL DEFAULT '',
    chat_id     TEXT    NOT NULL DEFAULT '',
    name        TEXT    NOT NULL,
    input       TEXT    NOT NULL,
    needs       TEXT    NOT NULL,
    line        TEXT    NOT NULL,
    shown_at    INTEGER NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'open',
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS employee_ceilings (
    agent_id    TEXT    PRIMARY KEY,
    creator_id  TEXT    NOT NULL,
    extras      TEXT    NOT NULL,
    ask_id      TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

-- +goose Down
DROP TABLE IF EXISTS employee_ceilings;
DROP TABLE IF EXISTS employee_drafts;
