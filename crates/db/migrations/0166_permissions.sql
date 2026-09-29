-- The one permission system (Turn-Controller-Technical-Design §2.12): rules,
-- modes, asks, the activity record, money spent against limits, and the
-- facts the surfaced cases read.
--
-- permission_rules: one row per rule. scope is 'company' (agent_id '') or
-- 'employee' (agent_id set). key_kind is tool | operation | capability;
-- field_kind is command_prefix | folder | domain | recipient, or '' for a
-- rule on the whole key. effect is allow | ask | deny. money is a
-- MoneyLimit JSON on an allow rule; source is a RuleSource JSON. A locked
-- rule (a law, a package's must-ask) is never removed or changed by an edit.
-- +goose Up
CREATE TABLE IF NOT EXISTS permission_rules (
    id          TEXT    PRIMARY KEY,
    scope       TEXT    NOT NULL CHECK (scope IN ('company', 'employee')),
    agent_id    TEXT    NOT NULL DEFAULT '',
    key_kind    TEXT    NOT NULL CHECK (key_kind IN ('tool', 'operation', 'capability')),
    key         TEXT    NOT NULL,
    field_kind  TEXT    NOT NULL DEFAULT '',
    field       TEXT    NOT NULL DEFAULT '',
    effect      TEXT    NOT NULL CHECK (effect IN ('allow', 'ask', 'deny')),
    money       TEXT,
    source      TEXT    NOT NULL,
    locked      INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_permission_rules_unique
    ON permission_rules (scope, agent_id, key_kind, key, field_kind, field);
CREATE INDEX IF NOT EXISTS idx_permission_rules_agent ON permission_rules (agent_id);

-- One mode per employee; the company row (agent_id '') is the default for
-- employees with none.
CREATE TABLE IF NOT EXISTS permission_modes (
    scope       TEXT    NOT NULL CHECK (scope IN ('company', 'employee')),
    agent_id    TEXT    NOT NULL DEFAULT '',
    mode        TEXT    NOT NULL CHECK (mode IN ('automatic', 'ask', 'plan', 'full_access')),
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (scope, agent_id)
);

-- A call parked on the owner. Only that step waits; the answer resumes it.
CREATE TABLE IF NOT EXISTS permission_asks (
    id           TEXT    PRIMARY KEY,
    agent_id     TEXT    NOT NULL DEFAULT '',
    session_key  TEXT    NOT NULL DEFAULT '',
    chat_id      TEXT,
    door         TEXT    NOT NULL,
    ask_case     TEXT    NOT NULL,
    sentence     TEXT    NOT NULL,
    target       TEXT    NOT NULL,
    call         TEXT    NOT NULL,
    seat         TEXT    NOT NULL,
    status       TEXT    NOT NULL DEFAULT 'open',
    answer       TEXT,
    answered_via TEXT,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    answered_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_permission_asks_status ON permission_asks (status, expires_at);
CREATE INDEX IF NOT EXISTS idx_permission_asks_session ON permission_asks (session_key);

-- Every decision the check made, with why.
CREATE TABLE IF NOT EXISTS permission_activity (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id     TEXT    NOT NULL DEFAULT '',
    session_key  TEXT    NOT NULL DEFAULT '',
    door         TEXT    NOT NULL,
    tool         TEXT    NOT NULL,
    rule_key     TEXT    NOT NULL,
    activity     TEXT    NOT NULL DEFAULT '',
    decision     TEXT    NOT NULL CHECK (decision IN ('allow', 'ask', 'deny')),
    why          TEXT    NOT NULL,
    ask_id       TEXT,
    unreviewed   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_permission_activity_agent ON permission_activity (agent_id, created_at);

-- Money spent per employee per day against its limits. counterparty '' is
-- the key's own total.
CREATE TABLE IF NOT EXISTS permission_spend (
    agent_id     TEXT    NOT NULL,
    day          TEXT    NOT NULL,
    key          TEXT    NOT NULL,
    counterparty TEXT    NOT NULL DEFAULT '',
    cents        INTEGER NOT NULL DEFAULT 0,
    count        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, day, key, counterparty)
);

-- Who an employee already works with (case 2) and what it made (case 3).
CREATE TABLE IF NOT EXISTS employee_counterparties (
    agent_id    TEXT    NOT NULL,
    address     TEXT    NOT NULL,
    kind        TEXT    NOT NULL,
    first_seen  INTEGER NOT NULL,
    via         TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (agent_id, address)
);
CREATE TABLE IF NOT EXISTS employee_created (
    agent_id     TEXT    NOT NULL,
    target_kind  TEXT    NOT NULL,
    target       TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (agent_id, target_kind, target)
);

-- The one-time conversion of the old permission shapes into rules and
-- modes: done once per install, with what it converted.
CREATE TABLE IF NOT EXISTS permission_migrations (
    name        TEXT    PRIMARY KEY,
    report      TEXT    NOT NULL,
    applied_at  INTEGER NOT NULL
);

-- +goose Down
DROP TABLE IF EXISTS permission_migrations;
DROP TABLE IF EXISTS employee_created;
DROP TABLE IF EXISTS employee_counterparties;
DROP TABLE IF EXISTS permission_spend;
DROP TABLE IF EXISTS permission_activity;
DROP TABLE IF EXISTS permission_asks;
DROP TABLE IF EXISTS permission_modes;
DROP TABLE IF EXISTS permission_rules;
