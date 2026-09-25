-- A workflow step parked on an ask waits on the run's own suspension: the
-- ask names the run, so the owner's answer releases that run.
ALTER TABLE permission_asks ADD COLUMN run_id TEXT;
