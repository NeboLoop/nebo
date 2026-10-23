-- The Inbox pointer a one-shot owes the owner once its request is in his
-- conversation. A fire writes its request (a hidden row) into the
-- conversation it was asked in and ends there; the employee's answer comes
-- from the turn that takes the request in. This row waits for that turn to
-- end, then is settled once: pointed at the answer, or closed unanswered.
-- One row per request. It carries the schedule's name and employee, so the
-- pointer still goes out when the schedule was deleted before the answer.
--
-- state: pending (its turn has not ended) | asked_again (the turn that took
-- it in ended with the process; the conversation was asked to answer it,
-- once) | pointed (the owner was pointed at answer_id) | unanswered (a turn
-- ended without answering it: stopped, failed, or the owner wrote first).
-- resume_after: set when a turn that took it in ended waiting on the owner
-- (an approval, a question): the last message row then. The answer comes
-- after it, from the turn his reply resumes. awaited_at: when that wait
-- began; a wait past a day is closed unanswered.
-- +goose Up
CREATE TABLE schedule_follow_ups (
    request_id TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    job_id INTEGER NOT NULL,
    job_name TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    asked_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    asked_again_at INTEGER,
    resume_after INTEGER,
    awaited_at INTEGER,
    answer_id TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    settled_at INTEGER
);
CREATE INDEX idx_schedule_follow_ups_open ON schedule_follow_ups(session_key) WHERE state IN ('pending', 'asked_again');

-- +goose Down
DROP INDEX IF EXISTS idx_schedule_follow_ups_open;
DROP TABLE IF EXISTS schedule_follow_ups;
