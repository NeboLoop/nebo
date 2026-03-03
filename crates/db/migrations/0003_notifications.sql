-- +goose Up
-- In-App Notifications

-- =============================================================================
-- NOTIFICATIONS
-- =============================================================================

CREATE TABLE IF NOT EXISTS notifications (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    type TEXT NOT NULL, -- invite, billing, system, team, etc.
    title TEXT NOT NULL,
    body TEXT,
    action_url TEXT,
    icon TEXT,
    read_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications(user_id);
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread ON notifications(user_id, read_at);
CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at DESC);

-- =============================================================================
-- ADD INAPP NOTIFICATIONS PREFERENCE
-- =============================================================================

ALTER TABLE user_preferences ADD COLUMN inapp_notifications INTEGER NOT NULL DEFAULT 1;

-- +goose Down
ALTER TABLE user_preferences DROP COLUMN inapp_notifications;

DROP INDEX IF EXISTS idx_notifications_created;
DROP INDEX IF EXISTS idx_notifications_user_unread;
DROP INDEX IF EXISTS idx_notifications_user;
DROP TABLE IF EXISTS notifications;
