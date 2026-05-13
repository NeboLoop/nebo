-- Index workflow_runs for the two hot queries:
--   list_workflow_runs: WHERE workflow_id = ? ORDER BY started_at DESC
--   has_running_run:    WHERE workflow_id = ? AND status = 'running'
--   count_workflow_runs: WHERE workflow_id = ?

CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_started
    ON workflow_runs(workflow_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_status
    ON workflow_runs(workflow_id, status);

-- Index workflow_activity_results for run detail lookups
CREATE INDEX IF NOT EXISTS idx_workflow_activity_results_run
    ON workflow_activity_results(run_id);
