-- Store the NeboAI agent-space conversation id for each exposed agent. The
-- in-memory ConvMaps (conv_id → agent) is rebuilt from JOIN updates on every
-- reconnect, so an inbound DM that arrives before the join completes — or
-- after a restart — could not be resolved to its agent and forked into a new
-- conversation. This column is the durable side of that mapping: written
-- through whenever the conv↔agent association is observed, read as the
-- fallback when ConvMaps misses. Nullable: NULL means "not yet observed".
ALTER TABLE agents ADD COLUMN loop_conv_id TEXT;
