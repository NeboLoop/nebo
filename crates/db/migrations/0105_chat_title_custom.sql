-- Track whether a chat's title was set explicitly by the user (a rename) vs
-- auto-generated/default. The auto-namer skips title_custom = 1 chats so it never
-- clobbers a user's chosen name — mirroring Claude desktop's "skip if explicitly
-- renamed" check (instead of matching per-locale default-title strings).
ALTER TABLE chats ADD COLUMN title_custom INTEGER NOT NULL DEFAULT 0;
