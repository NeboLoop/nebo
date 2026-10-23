-- A package's budget.total_per_run is its author's estimate and stops no
-- run. Findings a workflow audit kept from when it did ("A run used N
-- output tokens, over the M this workflow's budget allows
-- (budget.total_per_run). Raise the budget...") are gone from every report,
-- from its run limits and from its issues.
-- +goose Up
UPDATE workflow_audits
   SET report = json_set(
         report,
         '$.runLimits',
         (SELECT json_group_array(json(value)) FROM json_each(workflow_audits.report, '$.runLimits')
           WHERE COALESCE(json_extract(value, '$.message'), '') NOT LIKE '%budget.total_per_run%'),
         '$.issues',
         (SELECT json_group_array(json(value)) FROM json_each(workflow_audits.report, '$.issues')
           WHERE COALESCE(json_extract(value, '$.message'), '') NOT LIKE '%budget.total_per_run%')
       )
 WHERE json_valid(report) AND report LIKE '%budget.total_per_run%';

-- +goose Down
