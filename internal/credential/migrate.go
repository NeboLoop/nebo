package credential

import (
	"context"
	"database/sql"
	"fmt"
	"log/slog"
)

// Migrate encrypts all plaintext credentials in the database.
// Runs in a single transaction — rolls back entirely on failure.
// Idempotent: skips values that already have the "enc:" prefix.
func Migrate(ctx context.Context, rawDB *sql.DB) error {
	tx, err := rawDB.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("credential migration: begin tx: %w", err)
	}
	defer tx.Rollback()

	var total int

	n, err := migrateAuthProfiles(ctx, tx)
	if err != nil {
		return fmt.Errorf("credential migration: auth_profiles: %w", err)
	}
	total += n

	n, err = migrateMCPCredentials(ctx, tx)
	if err != nil {
		return fmt.Errorf("credential migration: mcp_credentials: %w", err)
	}
	total += n

	n, err = migrateAppOAuthGrants(ctx, tx)
	if err != nil {
		return fmt.Errorf("credential migration: app_oauth_grants: %w", err)
	}
	total += n

	n, err = migratePluginSecrets(ctx, tx)
	if err != nil {
		return fmt.Errorf("credential migration: plugin_settings: %w", err)
	}
	total += n

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("credential migration: commit: %w", err)
	}

	if total > 0 {
		slog.Info("Credential migration complete", "encrypted", total)
	}
	return nil
}

// migrateAuthProfiles encrypts plaintext api_key values in auth_profiles.
func migrateAuthProfiles(ctx context.Context, tx *sql.Tx) (int, error) {
	rows, err := tx.QueryContext(ctx, "SELECT id, api_key FROM auth_profiles WHERE api_key != ''")
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	type row struct {
		id, apiKey string
	}
	var toUpdate []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.apiKey); err != nil {
			return 0, err
		}
		if IsEncrypted(r.apiKey) {
			continue
		}
		toUpdate = append(toUpdate, r)
	}
	if err := rows.Err(); err != nil {
		return 0, err
	}

	for _, r := range toUpdate {
		enc, err := Encrypt(r.apiKey)
		if err != nil {
			return 0, fmt.Errorf("encrypt api_key for %s: %w", r.id, err)
		}
		if _, err := tx.ExecContext(ctx, "UPDATE auth_profiles SET api_key = ?, updated_at = unixepoch() WHERE id = ?", enc, r.id); err != nil {
			return 0, fmt.Errorf("update auth_profile %s: %w", r.id, err)
		}
	}
	return len(toUpdate), nil
}

// migrateMCPCredentials encrypts plaintext credential_value in mcp_integration_credentials.
// Skips oauth_token type (already encrypted by the MCP OAuth flow).
func migrateMCPCredentials(ctx context.Context, tx *sql.Tx) (int, error) {
	rows, err := tx.QueryContext(ctx,
		"SELECT id, credential_type, credential_value FROM mcp_integration_credentials WHERE credential_value != ''")
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	type row struct {
		id, credType, credValue string
	}
	var toUpdate []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.credType, &r.credValue); err != nil {
			return 0, err
		}
		if r.credType == "oauth_token" || IsEncrypted(r.credValue) {
			continue
		}
		toUpdate = append(toUpdate, r)
	}
	if err := rows.Err(); err != nil {
		return 0, err
	}

	for _, r := range toUpdate {
		enc, err := Encrypt(r.credValue)
		if err != nil {
			return 0, fmt.Errorf("encrypt credential %s: %w", r.id, err)
		}
		if _, err := tx.ExecContext(ctx, "UPDATE mcp_integration_credentials SET credential_value = ?, updated_at = unixepoch() WHERE id = ?", enc, r.id); err != nil {
			return 0, fmt.Errorf("update mcp_credential %s: %w", r.id, err)
		}
	}
	return len(toUpdate), nil
}

// migrateAppOAuthGrants encrypts plaintext access_token and refresh_token in app_oauth_grants.
func migrateAppOAuthGrants(ctx context.Context, tx *sql.Tx) (int, error) {
	rows, err := tx.QueryContext(ctx,
		"SELECT id, access_token, refresh_token FROM app_oauth_grants WHERE access_token != '' OR refresh_token != ''")
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	type row struct {
		id, accessToken, refreshToken string
	}
	var toUpdate []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.accessToken, &r.refreshToken); err != nil {
			return 0, err
		}
		needsUpdate := (!IsEncrypted(r.accessToken) && r.accessToken != "") ||
			(!IsEncrypted(r.refreshToken) && r.refreshToken != "")
		if !needsUpdate {
			continue
		}
		toUpdate = append(toUpdate, r)
	}
	if err := rows.Err(); err != nil {
		return 0, err
	}

	for _, r := range toUpdate {
		accessToken := r.accessToken
		if accessToken != "" && !IsEncrypted(accessToken) {
			var encErr error
			accessToken, encErr = Encrypt(accessToken)
			if encErr != nil {
				return 0, fmt.Errorf("encrypt access_token for grant %s: %w", r.id, encErr)
			}
		}

		refreshToken := r.refreshToken
		if refreshToken != "" && !IsEncrypted(refreshToken) {
			var encErr error
			refreshToken, encErr = Encrypt(refreshToken)
			if encErr != nil {
				return 0, fmt.Errorf("encrypt refresh_token for grant %s: %w", r.id, encErr)
			}
		}

		if _, err := tx.ExecContext(ctx,
			"UPDATE app_oauth_grants SET access_token = ?, refresh_token = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
			accessToken, refreshToken, r.id); err != nil {
			return 0, fmt.Errorf("update app_oauth_grant %s: %w", r.id, err)
		}
	}
	return len(toUpdate), nil
}

// migratePluginSecrets encrypts plaintext setting_value in plugin_settings where is_secret = 1.
func migratePluginSecrets(ctx context.Context, tx *sql.Tx) (int, error) {
	rows, err := tx.QueryContext(ctx,
		"SELECT id, setting_value FROM plugin_settings WHERE is_secret = 1 AND setting_value != ''")
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	type row struct {
		id, value string
	}
	var toUpdate []row
	for rows.Next() {
		var r row
		if err := rows.Scan(&r.id, &r.value); err != nil {
			return 0, err
		}
		if IsEncrypted(r.value) {
			continue
		}
		toUpdate = append(toUpdate, r)
	}
	if err := rows.Err(); err != nil {
		return 0, err
	}

	for _, r := range toUpdate {
		enc, err := Encrypt(r.value)
		if err != nil {
			return 0, fmt.Errorf("encrypt plugin_setting %s: %w", r.id, err)
		}
		if _, err := tx.ExecContext(ctx, "UPDATE plugin_settings SET setting_value = ?, updated_at = unixepoch() WHERE id = ?", enc, r.id); err != nil {
			return 0, fmt.Errorf("update plugin_setting %s: %w", r.id, err)
		}
	}
	return len(toUpdate), nil
}
