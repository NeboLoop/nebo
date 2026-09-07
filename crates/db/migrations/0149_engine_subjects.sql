-- +goose Up
-- Identity is business state, not execution state: it lives beside the
-- engine, not inside it. A SUBJECT is the person or thing a case is about;
-- an email, a phone, a CRM id are its ALIASES, never its identity. One open
-- case per (case type, subject) — a person can have a sales case and a
-- support case at once, owned by different employees.
--
-- Merging is conservative and deterministic (owner's rules, 2026-09-07):
-- exact normalized email or E.164 phone is the same alias; the same CRM id
-- is the same subject; an email and a phone observed together in a
-- customer-originated record join their subjects. Similarity never merges.
-- A merge is reversible: the losing subject is aliased to the winner
-- (`merged_into`), never destroyed, and every alias keeps where it came from.
CREATE TABLE IF NOT EXISTS engine_subjects (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    merged_into TEXT REFERENCES engine_subjects(id),
    merged_at INTEGER,
    merge_reason TEXT
);

CREATE TABLE IF NOT EXISTS engine_subject_aliases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id TEXT NOT NULL REFERENCES engine_subjects(id),
    -- email | phone | crm | <any other exact external id kind>
    kind TEXT NOT NULL,
    -- normalized: lowercased email, E.164 phone, trimmed id
    value TEXT NOT NULL,
    -- where the alias was first observed (a binding, a webhook, a merge)
    source TEXT NOT NULL DEFAULT '',
    -- the subject this alias was created under, kept across merges
    first_subject_id TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(kind, value)
);
CREATE INDEX IF NOT EXISTS idx_engine_subject_aliases_subject ON engine_subject_aliases(subject_id);

-- +goose Down
