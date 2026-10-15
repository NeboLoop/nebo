-- What an ask's card says beyond the call itself: the task the employee was
-- working on when it asked (`why`), the line it wrote for the owner about
-- the call (`stated_reason`), and a preview of what the call would send or
-- change (`details`, JSON [{kind, value}]). Asks from before have none.
-- +goose Up
ALTER TABLE permission_asks ADD COLUMN why TEXT NOT NULL DEFAULT '';
ALTER TABLE permission_asks ADD COLUMN stated_reason TEXT;
ALTER TABLE permission_asks ADD COLUMN details TEXT NOT NULL DEFAULT '[]';

-- +goose Down
ALTER TABLE permission_asks DROP COLUMN details;
ALTER TABLE permission_asks DROP COLUMN stated_reason;
ALTER TABLE permission_asks DROP COLUMN why;
