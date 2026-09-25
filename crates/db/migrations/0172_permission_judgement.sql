-- Automatic mode's judgement (Turn-Controller-Technical-Design §2.12.4): the
-- verdict a judge gave a call the code could not decide, kept beside the
-- decision (JSON: {mode, verdict, by, reason}; NULL when no judge was asked),
-- and the company setting that moves it from shadow to enforcing.
-- +goose Up
ALTER TABLE permission_activity ADD COLUMN judgement TEXT;
CREATE INDEX IF NOT EXISTS idx_permission_activity_unreviewed
    ON permission_activity (unreviewed, created_at);
-- 'shadow' | 'enforce'
ALTER TABLE settings ADD COLUMN permission_judgement TEXT NOT NULL DEFAULT 'shadow';

-- +goose Down
ALTER TABLE settings DROP COLUMN permission_judgement;
DROP INDEX IF EXISTS idx_permission_activity_unreviewed;
ALTER TABLE permission_activity DROP COLUMN judgement;
