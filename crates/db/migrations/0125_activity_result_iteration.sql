-- Loop iterations need their own identity in the activity log.
--
-- workflow_activity_results is an append-only execution log (no uniqueness on
-- run_id+activity_id), but the resume fast-forward looked activities up by
-- activity_id alone. A loop body writes one completed row per iteration, so
-- iteration 2 matched iteration 1's row and replayed its output instead of
-- running — the body executed once no matter how many items, while the loop
-- still reported the full count.
--
-- `iteration` is a dotted scope path, not a counter: '' for activities outside
-- any loop (every pre-existing row, hence the default — linear workflows keep
-- their exact current behavior), '0'/'1'/... inside a loop, and '0.2' for a
-- nested loop's third item within the outer loop's first. Nesting is reachable
-- today: loop_body_set validation forbids entering a body from outside, but
-- nothing forbids a loop activity inside another loop's body.
ALTER TABLE workflow_activity_results ADD COLUMN iteration TEXT NOT NULL DEFAULT '';

-- Resume must land on the same iteration it suspended in, or a run parked mid
-- loop fast-forwards the wrong one.
ALTER TABLE workflow_run_suspensions ADD COLUMN iteration TEXT NOT NULL DEFAULT '';
