-- Allow multiple companion chats (sessions) per user.
-- The old unique index forced one chat per user_id, blocking /new from creating additional sessions.

DROP INDEX IF EXISTS idx_chats_user_id;
CREATE INDEX idx_chats_user_id ON chats(user_id);
