package neboloop

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

const (
	neboLoopOAuthClientID = "nbl_nebo_desktop"
	oauthFlowTimeout      = 10 * time.Minute
)

// oauthFlowState tracks a pending OAuth authorization code flow.
type oauthFlowState struct {
	CodeVerifier string
	CreatedAt    time.Time
	Completed    bool
	Error        string
	Email        string
	DisplayName  string
}

var pendingFlows sync.Map // state string -> *oauthFlowState

func init() {
	go cleanupExpiredFlows()
}

func cleanupExpiredFlows() {
	ticker := time.NewTicker(1 * time.Minute)
	for range ticker.C {
		now := time.Now()
		pendingFlows.Range(func(key, value any) bool {
			if flow, ok := value.(*oauthFlowState); ok {
				if now.Sub(flow.CreatedAt) > oauthFlowTimeout {
					pendingFlows.Delete(key)
				}
			}
			return true
		})
	}
}

// --- PKCE helpers (RFC 7636) ---

func generateCodeVerifier() string {
	b := make([]byte, 32)
	rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func computeCodeChallenge(verifier string) string {
	h := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(h[:])
}

func generateState() string {
	b := make([]byte, 16)
	rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

// neboLoopFrontendURL derives the frontend URL from the API URL.
// e.g. "https://api.neboloop.com" → "https://neboloop.com"
// The OAuth authorize page is a frontend page, not an API endpoint.
func neboLoopFrontendURL(apiURL string) string {
	u, err := url.Parse(apiURL)
	if err != nil {
		return apiURL
	}
	host := u.Hostname()
	if strings.HasPrefix(host, "api.") {
		u.Host = strings.TrimPrefix(host, "api.")
		if u.Port() != "" {
			u.Host = u.Host + ":" + u.Port()
		}
	}
	return u.String()
}

// --- Handlers ---

// openBrowser opens a URL in the user's default system browser.
func openBrowser(targetURL string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", targetURL)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", targetURL)
	default:
		cmd = exec.Command("xdg-open", targetURL)
	}
	cmd.Stdin = strings.NewReader("")
	cmd.Stdout = nil
	cmd.Stderr = nil
	if err := cmd.Start(); err != nil {
		fmt.Printf("[NeboLoop OAuth] Failed to open browser: %v\n", err)
	}
}

// NeboLoopOAuthStartHandler generates PKCE parameters, opens the NeboLoop
// authorize URL in the system browser, and returns the state for polling.
func NeboLoopOAuthStartHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !svcCtx.Config.IsNeboLoopEnabled() {
			httputil.Error(w, fmt.Errorf("NeboLoop integration is disabled"))
			return
		}

		state := generateState()
		verifier := generateCodeVerifier()
		challenge := computeCodeChallenge(verifier)

		pendingFlows.Store(state, &oauthFlowState{
			CodeVerifier: verifier,
			CreatedAt:    time.Now(),
		})

		redirectURI := fmt.Sprintf("http://localhost:%d/auth/neboloop/callback", svcCtx.Config.Port)

		params := url.Values{
			"response_type":         {"code"},
			"client_id":             {neboLoopOAuthClientID},
			"redirect_uri":          {redirectURI},
			"scope":                 {"openid profile email"},
			"state":                 {state},
			"code_challenge":        {challenge},
			"code_challenge_method": {"S256"},
		}

		authorizeURL := neboLoopFrontendURL(svcCtx.Config.NeboLoop.ApiURL) + "/oauth/authorize?" + params.Encode()

		// Open in the user's default system browser so existing Google/Apple
		// sessions work for one-click sign-in. Standard desktop OAuth pattern.
		openBrowser(authorizeURL)

		httputil.OkJSON(w, types.NeboLoopOAuthStartResponse{
			AuthorizeURL: authorizeURL,
			State:        state,
		})
	}
}

// NeboLoopOAuthCallbackHandler is the OAuth redirect URI handler. The browser
// navigates here after the user authenticates on NeboLoop. It exchanges the
// authorization code for tokens, connects the bot, and serves HTML that
// communicates back to the opener window via postMessage.
func NeboLoopOAuthCallbackHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		state := r.URL.Query().Get("state")
		code := r.URL.Query().Get("code")
		errParam := r.URL.Query().Get("error")

		flowVal, ok := pendingFlows.Load(state)
		if !ok {
			serveCallbackHTML(w, "", "Invalid or expired OAuth state")
			return
		}
		flow := flowVal.(*oauthFlowState)

		if errParam != "" {
			flow.Error = errParam
			flow.Completed = true
			serveCallbackHTML(w, "", "Authentication was denied or failed: "+errParam)
			return
		}

		if code == "" {
			flow.Error = "missing authorization code"
			flow.Completed = true
			serveCallbackHTML(w, "", "Missing authorization code")
			return
		}

		ctx := r.Context()
		apiURL := svcCtx.Config.NeboLoop.ApiURL
		redirectURI := fmt.Sprintf("http://localhost:%d/auth/neboloop/callback", svcCtx.Config.Port)

		// Exchange authorization code for tokens
		tokenResp, err := exchangeOAuthCode(ctx, apiURL, code, flow.CodeVerifier, redirectURI)
		if err != nil {
			flow.Error = err.Error()
			flow.Completed = true
			serveCallbackHTML(w, "", "Token exchange failed")
			return
		}

		// Get user info
		userInfo, err := fetchUserInfo(ctx, apiURL, tokenResp.AccessToken)
		if err != nil {
			flow.Error = err.Error()
			flow.Completed = true
			serveCallbackHTML(w, "", "Failed to get user info")
			return
		}

		// Ensure the owner has a bot, get connection token
		botID, connectionToken, err := ensureBot(ctx, apiURL, tokenResp.AccessToken)
		if err != nil {
			fmt.Printf("[NeboLoop OAuth] Warning: bot setup failed: %v\n", err)
			// Not fatal -- user is still authenticated, bot can be connected later
		}

		// Store owner profile (reuses existing helper)
		if err := storeNeboLoopProfile(ctx, svcCtx.DB, apiURL, userInfo.ID, userInfo.Email, tokenResp.AccessToken, tokenResp.RefreshToken); err != nil {
			fmt.Printf("[NeboLoop OAuth] Warning: failed to store profile: %v\n", err)
		}

		// Auto-connect bot (reuses existing helper)
		if connectionToken != "" {
			if err := autoConnectBot(ctx, svcCtx, apiURL, botID, connectionToken); err != nil {
				fmt.Printf("[NeboLoop OAuth] Warning: auto-connect failed: %v\n", err)
			} else {
				activateNeboLoopComm(svcCtx)
			}
		}

		// Mark flow as completed
		flow.Email = userInfo.Email
		flow.DisplayName = userInfo.DisplayName
		flow.Completed = true

		serveCallbackHTML(w, userInfo.Email, "")
	}
}

// NeboLoopOAuthStatusHandler is a polling endpoint so the frontend can detect
// when the OAuth flow completes (fallback for when postMessage doesn't work).
func NeboLoopOAuthStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		state := httputil.QueryString(r, "state", "")
		if state == "" {
			httputil.Error(w, fmt.Errorf("state parameter required"))
			return
		}

		flowVal, ok := pendingFlows.Load(state)
		if !ok {
			httputil.OkJSON(w, types.NeboLoopOAuthStatusResponse{Status: "expired"})
			return
		}

		flow := flowVal.(*oauthFlowState)
		if !flow.Completed {
			httputil.OkJSON(w, types.NeboLoopOAuthStatusResponse{Status: "pending"})
			return
		}

		if flow.Error != "" {
			httputil.OkJSON(w, types.NeboLoopOAuthStatusResponse{
				Status: "error",
				Error:  flow.Error,
			})
		} else {
			httputil.OkJSON(w, types.NeboLoopOAuthStatusResponse{
				Status:      "complete",
				Email:       flow.Email,
				DisplayName: flow.DisplayName,
			})
		}

		// Clean up after status is read
		pendingFlows.Delete(state)
	}
}

// --- HTTP helpers ---

type oauthTokenResponse struct {
	AccessToken  string `json:"access_token"`
	TokenType    string `json:"token_type"`
	ExpiresIn    int    `json:"expires_in"`
	RefreshToken string `json:"refresh_token"`
	Scope        string `json:"scope"`
	IDToken      string `json:"id_token,omitempty"`
}

type oauthUserInfo struct {
	ID          string `json:"sub"`
	Email       string `json:"email"`
	DisplayName string `json:"name"`
}

func exchangeOAuthCode(ctx context.Context, apiURL, code, codeVerifier, redirectURI string) (*oauthTokenResponse, error) {
	body, _ := json.Marshal(map[string]string{
		"grant_type":    "authorization_code",
		"code":          code,
		"redirect_uri":  redirectURI,
		"client_id":     neboLoopOAuthClientID,
		"code_verifier": codeVerifier,
	})

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, apiURL+"/oauth/token",
		strings.NewReader(string(body)))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("token request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("token endpoint returned %d: %s", resp.StatusCode, string(body))
	}

	var result oauthTokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("decode token response: %w", err)
	}
	return &result, nil
}

func fetchUserInfo(ctx context.Context, apiURL, accessToken string) (*oauthUserInfo, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL+"/oauth/userinfo", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("userinfo request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("userinfo endpoint returned %d: %s", resp.StatusCode, string(body))
	}

	var result oauthUserInfo
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, fmt.Errorf("decode userinfo response: %w", err)
	}
	return &result, nil
}

// ensureBot checks if the owner has a bot, creates one if not, and returns
// the bot ID and a connection token for MQTT credential exchange.
func ensureBot(ctx context.Context, apiURL, accessToken string) (botID, connectionToken string, err error) {
	// List owner's bots
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL+"/api/v1/owners/me/bots", nil)
	if err != nil {
		return "", "", err
	}
	req.Header.Set("Authorization", "Bearer "+accessToken)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", "", fmt.Errorf("list bots failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		var bots []struct {
			ID              string `json:"id"`
			Name            string `json:"name"`
			ConnectionToken string `json:"connection_token,omitempty"`
		}
		if err := json.NewDecoder(resp.Body).Decode(&bots); err == nil && len(bots) > 0 {
			bot := bots[0]
			if bot.ConnectionToken != "" {
				return bot.ID, bot.ConnectionToken, nil
			}
			// Bot exists but no connection token -- need to create a new one
			return bot.ID, "", nil
		}
	}

	// No bots found -- create one
	createBody, _ := json.Marshal(map[string]string{
		"name":    "My Nebo",
		"purpose": "Personal Desktop AI Companion",
	})
	createReq, err := http.NewRequestWithContext(ctx, http.MethodPost, apiURL+"/api/v1/bots",
		strings.NewReader(string(createBody)))
	if err != nil {
		return "", "", err
	}
	createReq.Header.Set("Authorization", "Bearer "+accessToken)
	createReq.Header.Set("Content-Type", "application/json")

	createResp, err := http.DefaultClient.Do(createReq)
	if err != nil {
		return "", "", fmt.Errorf("create bot failed: %w", err)
	}
	defer createResp.Body.Close()

	if createResp.StatusCode != http.StatusOK && createResp.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(createResp.Body)
		return "", "", fmt.Errorf("create bot returned %d: %s", createResp.StatusCode, string(body))
	}

	var created struct {
		ID              string `json:"id"`
		ConnectionToken string `json:"connection_token"`
	}
	if err := json.NewDecoder(createResp.Body).Decode(&created); err != nil {
		return "", "", fmt.Errorf("decode create bot response: %w", err)
	}

	return created.ID, created.ConnectionToken, nil
}

// serveCallbackHTML renders a minimal HTML page that closes the browser
// window/tab after the OAuth flow completes. The Nebo app detects completion
// via polling, so this page just needs to get out of the user's way.
func serveCallbackHTML(w http.ResponseWriter, email, errMsg string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")

	var message string
	if errMsg != "" {
		message = "Sign-in failed: " + errMsg
	} else {
		message = "Connected! You can close this window."
	}

	html := fmt.Sprintf(`<!DOCTYPE html>
<html><head><title>NeboLoop</title>
<style>
body { font-family: -apple-system, sans-serif; display: flex; align-items: center;
  justify-content: center; min-height: 100vh; margin: 0; background: #f5f5f5; }
p { font-size: 16px; color: #333; }
</style>
</head>
<body>
<p>%s</p>
<script>
// Try to close this window/tab automatically
setTimeout(function() { window.close(); }, 1500);
</script>
</body></html>`, message)

	w.Write([]byte(html))
}
