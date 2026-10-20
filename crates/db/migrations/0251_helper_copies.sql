-- A helper that changes the owner's files works in its own copy, held until
-- the conversation that started it lands or discards its changes. The
-- copy's whole state (the project it was made from, its branch, the commit
-- it sits on, the conflicts waiting in it, and why it can't land yet) is
-- kept on the helper's run, so a restart finds it again and a follow-up
-- continues in it. NULL once its changes are landed or discarded.
ALTER TABLE engine_runs ADD COLUMN held_copy TEXT;
