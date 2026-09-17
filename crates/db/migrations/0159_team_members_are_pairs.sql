-- A team member is a (bot, agent) pair.
--
-- Until now a member was a bare local agent id, which quietly assumed every
-- member lives on this machine. The owner's workforce is spread across several
-- machines, so a team has to be able to name someone who runs elsewhere.
-- Storing the pair from the first line makes a cross-bot team a feature rather
-- than a later migration.
--
-- An empty botId means THIS machine, which is also what an unlinked Nebo has
-- and so is the only honest default. A remote member carries the bot that runs
-- it, and then agentId is the HUB agent id, because that is the id a mention
-- token must carry for the far machine to resolve it.
--
-- `name` is a label, not an identity. A local member's name is resolved live
-- from the roster; a remote member has no local roster to ask, so the name
-- recorded when it joined is what gets shown.

ALTER TABLE teams RENAME COLUMN member_agent_ids TO members;

UPDATE teams
SET members = (
    SELECT COALESCE(
        json_group_array(json_object('botId', '', 'agentId', value, 'name', '')),
        '[]'
    )
    FROM json_each(teams.members)
)
WHERE json_valid(members);

-- A team with no members serialises to 'null' through json_group_array over an
-- empty set; the readers expect a list.
UPDATE teams SET members = '[]' WHERE members IS NULL OR members = 'null';
