-- An employee's memory setting is one choice with three options
-- (memory.mode): "single" (one conversation, one memory), "separate" (many
-- conversations sharing one memory) and "confidential" (every conversation a
-- sealed matter). It replaces the memory.context_isolated flag.
--
-- The flag maps to the mode it behaved as: off is "single", on is
-- "separate" (napp::agent::MemoryMode::from_context_isolated reads an
-- agent.json that still carries the flag the same way). An employee whose
-- frontmatter already names a mode keeps it, and the flag goes. Running it
-- again finds no flag to map.
-- +goose Up
UPDATE agents
SET frontmatter = json_set(
        frontmatter,
        '$.memory.mode',
        CASE json_type(frontmatter, '$.memory.context_isolated') WHEN 'true' THEN 'separate' ELSE 'single' END
    )
WHERE json_valid(frontmatter)
  AND json_type(frontmatter, '$.memory.context_isolated') IS NOT NULL
  AND json_type(frontmatter, '$.memory.mode') IS NULL;

UPDATE agents
SET frontmatter = json_remove(frontmatter, '$.memory.context_isolated')
WHERE json_valid(frontmatter)
  AND json_type(frontmatter, '$.memory.context_isolated') IS NOT NULL;
