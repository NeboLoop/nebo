-- The tool round's loop guards are gone (identical-call, spiral, read-failure
-- and same-error), and with them their Settings → Developer thresholds.
ALTER TABLE settings DROP COLUMN guardrails;
