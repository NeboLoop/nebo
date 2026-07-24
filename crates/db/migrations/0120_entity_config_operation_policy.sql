-- Per-employee approval policy over gated interface operations (three-state:
-- always / approval / blocked). JSON blob of tools::policy::OperationPolicy,
-- NULL = inherit the seat's declared defaults. Rides the same per-entity
-- override pipeline as `permissions`.
ALTER TABLE entity_config ADD COLUMN operation_policy TEXT;
