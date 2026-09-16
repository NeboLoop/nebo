-- The company level of the ONE operation policy: the constitution. The owner
-- writes it once; every seat rule must fit inside it; the General Manager
-- cannot grant past it. One row, the CompanyPolicy JSON
-- (tools::policy::CompanyPolicy).
CREATE TABLE IF NOT EXISTS company_policy (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    policy     TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Today's tallies per standing grant, so a grant's per-day bounds can be
-- checked before an unattended operation runs. Keyed on the rule
-- (`<agent_id>:<operation suffix>`, or `company` for the company-wide totals),
-- the local day, and the counterparty ('' for the rule's own totals).
CREATE TABLE IF NOT EXISTS operation_counters (
    rule_key           TEXT    NOT NULL,
    day                TEXT    NOT NULL,
    counterparty       TEXT    NOT NULL DEFAULT '',
    count              INTEGER NOT NULL DEFAULT 0,
    cents              INTEGER NOT NULL DEFAULT 0,
    counterparty_cents INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (rule_key, day, counterparty)
);
