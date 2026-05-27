-- Preserve channel context across scheduled tasks so an agent that set a
-- timer from inside a Slack thread can reply into the same thread when the
-- timer fires. Without these columns the cron-fired RunRequest has no agent
-- and no channel destination → the reply goes nowhere visible from Slack.
--
-- See docs/publishers-guide/channel-plugins.md and
-- crates/server/src/scheduler.rs::execute_agent for the consumer.
ALTER TABLE cron_jobs ADD COLUMN agent_id TEXT;
ALTER TABLE cron_jobs ADD COLUMN channel_ctx_json TEXT;
