-- +goose Up
-- Inbound hub dedupe rides the engine (design of record, 2026-09-06). A
-- processed wire msg_id is an event with idempotency key 'comm:<id>' that
-- is already delivered: the engine never claims it, and a redelivery hits
-- the unique key and is reported as a duplicate — I-2, the same guarantee
-- every other event has. Rows are transient and age out on the engine TTL.
INSERT INTO engine_events (kind, target_type, target_id, idem_key, retention, created_at, delivered_at, attempts)
SELECT 'seen', 'entity', 'comm', 'comm:' || id, 'transient', seen_at, seen_at, 1
FROM comm_seen_messages
WHERE true
ON CONFLICT(idem_key) DO NOTHING;

DROP TABLE comm_seen_messages;

-- +goose Down
