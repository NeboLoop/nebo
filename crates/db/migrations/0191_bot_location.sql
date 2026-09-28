-- The bot's Location (Bot settings → Location): where the office is. The
-- owner names it (`location_label`, an address as they write it); its
-- coordinates come from a geocoder, which only the phone has, so a label
-- typed on the web or desktop is kept without them until the phone next
-- opens and fills them in. Stored in Nebo's own database: it works with no
-- NeboAI account. An empty label means no Location is set.
-- +goose Up
ALTER TABLE settings ADD COLUMN location_label TEXT NOT NULL DEFAULT '';
ALTER TABLE settings ADD COLUMN location_latitude REAL;
ALTER TABLE settings ADD COLUMN location_longitude REAL;

-- +goose Down
ALTER TABLE settings DROP COLUMN location_longitude;
ALTER TABLE settings DROP COLUMN location_latitude;
ALTER TABLE settings DROP COLUMN location_label;
