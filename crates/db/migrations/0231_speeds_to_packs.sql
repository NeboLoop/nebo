-- Every stored choice of a Nebo AI speed becomes the Nebo AI pack at its
-- level, once: a conversation's model, a session's override, an employee's
-- model preference, and the model of each activity of a workflow and of an
-- employee's own workflows (agents.frontmatter, their source of truth). The apps and the engine
-- read only pack references after this; nothing maps a speed id at read
-- time. Pack levels (intelligence_packs) name the speeds as the models a
-- level runs on and are not choices: they are left as they are, as are the
-- synced provider_models rows and workflow_runs.model (a record).
-- +goose Up
CREATE TEMP TABLE speed_packs (speed TEXT PRIMARY KEY, pack TEXT NOT NULL);
INSERT INTO speed_packs (speed, pack) VALUES
    ('janus/nebo-1', 'pack/nebo-ai'),
    ('nebo-1', 'pack/nebo-ai'),
    ('janus/nebo-1-flash', 'pack/nebo-ai/instant'),
    ('nebo-1-flash', 'pack/nebo-ai/instant'),
    ('janus/nebo-1-medium', 'pack/nebo-ai/medium'),
    ('nebo-1-medium', 'pack/nebo-ai/medium'),
    ('janus/nebo-1-pro', 'pack/nebo-ai/high'),
    ('nebo-1-pro', 'pack/nebo-ai/high');

UPDATE chats SET model = (SELECT pack FROM speed_packs WHERE speed = trim(chats.model))
WHERE trim(model) IN (SELECT speed FROM speed_packs);

UPDATE sessions SET model_override = (SELECT pack FROM speed_packs WHERE speed = trim(sessions.model_override))
WHERE trim(model_override) IN (SELECT speed FROM speed_packs);

UPDATE entity_config SET model_preference = (SELECT pack FROM speed_packs WHERE speed = trim(entity_config.model_preference))
WHERE trim(model_preference) IN (SELECT speed FROM speed_packs);

UPDATE workflows SET definition = json_set(definition, '$.activities', (
    SELECT json_group_array(json(activity)) FROM (
        SELECT CASE
            WHEN sp.pack IS NOT NULL THEN json_set(a.value, '$.model', sp.pack)
            ELSE a.value
        END AS activity
        FROM json_each(workflows.definition, '$.activities') AS a
        LEFT JOIN speed_packs AS sp ON sp.speed = trim(json_extract(a.value, '$.model'))
        ORDER BY a.key
    )
))
WHERE json_valid(definition)
  AND json_type(definition, '$.activities') = 'array'
  AND EXISTS (
    SELECT 1 FROM json_each(workflows.definition, '$.activities') AS a
    WHERE trim(json_extract(a.value, '$.model')) IN (SELECT speed FROM speed_packs)
  );

UPDATE agents SET frontmatter = json_set(frontmatter, '$.workflows', (
    SELECT json_group_object(w.key, json(CASE
        WHEN json_type(w.value, '$.activities') = 'array' THEN json_set(w.value, '$.activities', (
            SELECT json_group_array(json(activity)) FROM (
                SELECT CASE
                    WHEN sp.pack IS NOT NULL THEN json_set(a.value, '$.model', sp.pack)
                    ELSE a.value
                END AS activity
                FROM json_each(w.value, '$.activities') AS a
                LEFT JOIN speed_packs AS sp ON sp.speed = trim(json_extract(a.value, '$.model'))
                ORDER BY a.key
            )
        ))
        ELSE w.value
    END))
    FROM json_each(agents.frontmatter, '$.workflows') AS w
))
WHERE json_valid(frontmatter)
  AND json_type(frontmatter, '$.workflows') = 'object'
  AND EXISTS (
    SELECT 1 FROM json_each(agents.frontmatter, '$.workflows') AS w, json_each(w.value, '$.activities') AS a
    WHERE trim(json_extract(a.value, '$.model')) IN (SELECT speed FROM speed_packs)
  );

DROP TABLE speed_packs;

-- +goose Down
SELECT 1;
