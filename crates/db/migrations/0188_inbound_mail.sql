-- Every inbound message the mail intake takes: one row per message, whatever
-- the source (the bot's hosted address today; a connected mailbox, a text or
-- a website chat later). It is the intake's record of what came in, who it
-- was judged to be from (owner or external), which employee it went to, and
-- where it was taken up — and the handle a reply is sent back through, which
-- is why the email channel's reply route names a row here.
--
-- `record` keeps the whole normalized message (auth results, labels,
-- attachment metadata, thread references) as JSON, so the classifier and the
-- contacts lookup that come next read what the intake saw without another
-- column per field. A source's own id for the message (`reply_handle`) is
-- unique per source: a redelivered message is the same row.
-- +goose Up
CREATE TABLE IF NOT EXISTS inbound_mail (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    reply_handle TEXT NOT NULL DEFAULT '',
    sender_address TEXT NOT NULL DEFAULT '',
    sender_name TEXT NOT NULL DEFAULT '',
    standing TEXT NOT NULL,
    agent_id TEXT NOT NULL DEFAULT '',
    employee_tag TEXT NOT NULL DEFAULT '',
    session_key TEXT NOT NULL DEFAULT '',
    subject TEXT NOT NULL DEFAULT '',
    auto_submitted INTEGER NOT NULL DEFAULT 0,
    record TEXT NOT NULL,
    received_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_inbound_mail_source_handle
    ON inbound_mail(source, reply_handle) WHERE reply_handle != '';

-- +goose Down
DROP TABLE IF EXISTS inbound_mail;
