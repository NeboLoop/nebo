-- +goose Up
-- Where an app came from, recorded when its row is made: 1 when it was made
-- on this bot (the owner's employees built it, or he put its folder in
-- user/agents himself), 0 when it was installed (the store, a loop, an
-- install code) or nothing says. Only an app made here holds what its
-- manifest asks for without the owner's OK (`tools::app_dev::is_own_app`).
-- A duplicate keeps its source's.
ALTER TABLE agents ADD COLUMN built_here INTEGER NOT NULL DEFAULT 0;
-- Rows made before this: what the folder says (a sealed .napp, or a package
-- in nebo/agents/, is an install).
UPDATE agents SET built_here = 1
 WHERE napp_path IS NULL OR trim(napp_path) = ''
    OR (lower(napp_path) NOT LIKE '%.napp'
        AND lower(replace(napp_path, '\', '/')) NOT LIKE '%/nebo/agents/%');

-- +goose Down
ALTER TABLE agents DROP COLUMN built_here;
