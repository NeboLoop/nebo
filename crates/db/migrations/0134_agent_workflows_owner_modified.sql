-- Owner edits to a workflow binding must survive the boot FS→DB sync.
-- A cloud bot's image roll re-extracts the packaged agent.json and
-- sync_agent_workflows upserted it over live owner edits (Biss's
-- order-intake restructure was silently reverted by a pod restart,
-- 2026-08-24). Same class as agents.name_locked (0132): the flag marks
-- rows the owner (API/tool) wrote; package sync may not touch them.
ALTER TABLE agent_workflows ADD COLUMN owner_modified INTEGER NOT NULL DEFAULT 0;
