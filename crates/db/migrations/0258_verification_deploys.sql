-- The apps a verification's work deployed into (their pages, built from
-- its project): each one's live load is one of its checks, kept on the row
-- so a later turn resuming the work checks them too. deploy_unchecked: a
-- deploy came after the last round, so the work's verdict waits on a new
-- one.
-- +goose Up
ALTER TABLE verifications ADD COLUMN deployed_apps_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE verifications ADD COLUMN deploy_unchecked INTEGER NOT NULL DEFAULT 0;

-- +goose Down
ALTER TABLE verifications DROP COLUMN deploy_unchecked;
ALTER TABLE verifications DROP COLUMN deployed_apps_json;
