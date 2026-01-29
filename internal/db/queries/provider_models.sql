-- name: ListProviderModels :many
SELECT * FROM provider_models WHERE profile_id = ? ORDER BY display_name;

-- name: ListActiveModels :many
SELECT * FROM provider_models WHERE profile_id = ? AND is_active = 1 ORDER BY display_name;

-- name: GetDefaultModel :one
SELECT * FROM provider_models WHERE profile_id = ? AND is_default = 1 LIMIT 1;

-- name: GetProviderModel :one
SELECT * FROM provider_models WHERE id = ?;

-- name: GetProviderModelByModelId :one
SELECT * FROM provider_models WHERE profile_id = ? AND model_id = ?;

-- name: CreateProviderModel :one
INSERT INTO provider_models (id, profile_id, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: UpdateProviderModelActive :exec
UPDATE provider_models SET is_active = ?, updated_at = ? WHERE id = ?;

-- name: SetDefaultModel :exec
UPDATE provider_models SET is_default = CASE WHEN id = ? THEN 1 ELSE 0 END, updated_at = ? WHERE profile_id = ?;

-- name: DeleteProviderModel :exec
DELETE FROM provider_models WHERE id = ?;

-- name: DeleteProviderModelsByProfile :exec
DELETE FROM provider_models WHERE profile_id = ?;
