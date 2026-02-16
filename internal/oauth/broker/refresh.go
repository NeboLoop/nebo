package broker

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	mcpclient "github.com/neboloop/nebo/internal/mcp/client"
)

// StartRefreshLoop starts a background goroutine that refreshes tokens
// expiring within 5 minutes. It runs every 60 seconds until ctx is cancelled.
func (b *Broker) StartRefreshLoop(ctx context.Context) {
	go func() {
		ticker := time.NewTicker(60 * time.Second)
		defer ticker.Stop()

		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				if err := b.RefreshExpiring(ctx); err != nil {
					fmt.Printf("[oauth-broker] refresh sweep error: %v\n", err)
				}
			}
		}
	}()
}

// RefreshExpiring refreshes all grants expiring within 5 minutes.
func (b *Broker) RefreshExpiring(ctx context.Context) error {
	grants, err := b.db.ListExpiringOAuthGrants(ctx, sql.NullString{String: "5", Valid: true})
	if err != nil {
		return fmt.Errorf("list expiring grants: %w", err)
	}

	for _, grant := range grants {
		if err := b.refreshGrant(ctx, grant.AppID, grant.Provider, grant.RefreshToken); err != nil {
			fmt.Printf("[oauth-broker] failed to refresh %s/%s: %v\n", grant.AppID, grant.Provider, err)
			continue
		}
	}
	return nil
}

// refreshGrant performs a token refresh for a single grant.
func (b *Broker) refreshGrant(ctx context.Context, appID, providerName, encryptedRefreshToken string) error {
	b.mu.RLock()
	provider, ok := b.providers[providerName]
	b.mu.RUnlock()
	if !ok {
		return fmt.Errorf("unknown provider: %s", providerName)
	}

	refreshToken, err := mcpclient.DecryptString(encryptedRefreshToken, b.encryptionKey)
	if err != nil {
		return fmt.Errorf("decrypt refresh token: %w", err)
	}

	tokenEndpoint := b.resolveEndpoint(provider.TokenEndpoint, provider.TenantID)

	data := url.Values{}
	data.Set("grant_type", "refresh_token")
	data.Set("refresh_token", refreshToken)
	data.Set("client_id", provider.ClientID)
	if provider.ClientSecret != "" {
		data.Set("client_secret", provider.ClientSecret)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", tokenEndpoint, strings.NewReader(data.Encode()))
	if err != nil {
		return fmt.Errorf("create refresh request: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("Accept", "application/json")

	resp, err := b.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("refresh request: %w", err)
	}
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("refresh failed with status %d: %s", resp.StatusCode, string(body))
	}

	var tokenResp tokenResponse
	if err := json.Unmarshal(body, &tokenResp); err != nil {
		return fmt.Errorf("decode refresh response: %w", err)
	}

	return b.storeAndPushTokens(ctx, appID, providerName, &tokenResp, refreshToken)
}
