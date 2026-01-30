package oauth

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"

	"gobot/internal/db"
	"gobot/internal/svc"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
)

// Handler handles OAuth callbacks directly
type Handler struct {
	svcCtx *svc.ServiceContext
}

// NewHandler creates a new OAuth handler
func NewHandler(svcCtx *svc.ServiceContext) *Handler {
	return &Handler{svcCtx: svcCtx}
}

// RegisterRoutes registers OAuth callback routes on the chi router
func (h *Handler) RegisterRoutes(r chi.Router) {
	r.Get("/oauth/google/callback", h.googleCallback)
	r.Get("/oauth/github/callback", h.githubCallback)
}

// googleCallback handles Google OAuth callback
func (h *Handler) googleCallback(w http.ResponseWriter, r *http.Request) {
	h.handleCallback(w, r, "google")
}

// githubCallback handles GitHub OAuth callback
func (h *Handler) githubCallback(w http.ResponseWriter, r *http.Request) {
	h.handleCallback(w, r, "github")
}

func (h *Handler) handleCallback(w http.ResponseWriter, r *http.Request, provider string) {
	// Check if OAuth is enabled
	if !h.svcCtx.Config.IsOAuthEnabled() {
		h.redirectWithError(w, r, "OAuth is not enabled")
		return
	}

	if !h.svcCtx.UseLocal() {
		h.redirectWithError(w, r, "OAuth not available in this mode")
		return
	}

	// Get code and state from query params
	code := r.URL.Query().Get("code")
	state := r.URL.Query().Get("state")
	errorParam := r.URL.Query().Get("error")

	if errorParam != "" {
		h.redirectWithError(w, r, "OAuth error: "+errorParam)
		return
	}

	if code == "" {
		h.redirectWithError(w, r, "Missing authorization code")
		return
	}

	// Exchange code for user info
	var userInfo *OAuthUserInfo
	var err error

	switch provider {
	case "google":
		if !h.svcCtx.Config.IsGoogleOAuthEnabled() {
			h.redirectWithError(w, r, "Google OAuth is not enabled")
			return
		}
		userInfo, err = h.exchangeGoogleCode(code)
	case "github":
		if !h.svcCtx.Config.IsGitHubOAuthEnabled() {
			h.redirectWithError(w, r, "GitHub OAuth is not enabled")
			return
		}
		userInfo, err = h.exchangeGitHubCode(code)
	default:
		h.redirectWithError(w, r, "Unknown provider")
		return
	}

	if err != nil {
		h.redirectWithError(w, r, "Failed to authenticate: "+err.Error())
		return
	}

	// Find or create user
	ctx := r.Context()
	var userID string
	isNewUser := false

	// Check if OAuth connection exists
	existingConn, err := h.svcCtx.DB.Queries.GetOAuthConnectionByProvider(ctx, db.GetOAuthConnectionByProviderParams{
		Provider:       provider,
		ProviderUserID: userInfo.ProviderUserID,
	})

	if err == nil {
		// Connection exists, use existing user
		userID = existingConn.UserID

		// Update connection info
		_ = h.svcCtx.DB.Queries.UpdateOAuthConnection(ctx, db.UpdateOAuthConnectionParams{
			ID:           existingConn.ID,
			Email:        sql.NullString{String: userInfo.Email, Valid: userInfo.Email != ""},
			Name:         sql.NullString{String: userInfo.Name, Valid: userInfo.Name != ""},
			AvatarUrl:    sql.NullString{String: userInfo.AvatarURL, Valid: userInfo.AvatarURL != ""},
			AccessToken:  sql.NullString{String: userInfo.AccessToken, Valid: userInfo.AccessToken != ""},
			RefreshToken: sql.NullString{String: userInfo.RefreshToken, Valid: userInfo.RefreshToken != ""},
		})
	} else {
		// No existing connection - check if user exists by email
		existingUser, err := h.svcCtx.Auth.GetUserByEmail(ctx, userInfo.Email)
		if err == nil {
			// User exists - link OAuth to existing account
			userID = existingUser.ID
		} else {
			// Create new user
			isNewUser = true
			newUser, err := h.svcCtx.DB.Queries.CreateUserFromOAuth(ctx, db.CreateUserFromOAuthParams{
				ID:        uuid.New().String(),
				Email:     userInfo.Email,
				Name:      userInfo.Name,
				AvatarUrl: sql.NullString{String: userInfo.AvatarURL, Valid: userInfo.AvatarURL != ""},
			})
			if err != nil {
				h.redirectWithError(w, r, "Failed to create user")
				return
			}
			userID = newUser.ID

			// Create user preferences
			_, _ = h.svcCtx.DB.Queries.CreateUserPreferences(ctx, userID)
		}

		// Create OAuth connection
		_, _ = h.svcCtx.DB.Queries.CreateOAuthConnection(ctx, db.CreateOAuthConnectionParams{
			ID:             uuid.New().String(),
			UserID:         userID,
			Provider:       provider,
			ProviderUserID: userInfo.ProviderUserID,
			Email:          sql.NullString{String: userInfo.Email, Valid: userInfo.Email != ""},
			Name:           sql.NullString{String: userInfo.Name, Valid: userInfo.Name != ""},
			AvatarUrl:      sql.NullString{String: userInfo.AvatarURL, Valid: userInfo.AvatarURL != ""},
			AccessToken:    sql.NullString{String: userInfo.AccessToken, Valid: userInfo.AccessToken != ""},
			RefreshToken:   sql.NullString{String: userInfo.RefreshToken, Valid: userInfo.RefreshToken != ""},
		})
	}

	// Generate tokens
	authResp, err := h.svcCtx.Auth.GenerateTokensForUser(ctx, userID, userInfo.Email)
	if err != nil {
		h.redirectWithError(w, r, "Failed to generate tokens")
		return
	}

	// Redirect to frontend with tokens
	redirectURL := fmt.Sprintf("/auth/callback?token=%s&refresh=%s&expires=%d&new=%t&state=%s",
		url.QueryEscape(authResp.Token),
		url.QueryEscape(authResp.RefreshToken),
		authResp.ExpiresAt.UnixMilli(),
		isNewUser,
		url.QueryEscape(state),
	)

	http.Redirect(w, r, redirectURL, http.StatusTemporaryRedirect)
}

func (h *Handler) redirectWithError(w http.ResponseWriter, r *http.Request, message string) {
	redirectURL := fmt.Sprintf("/login?error=%s", url.QueryEscape(message))
	http.Redirect(w, r, redirectURL, http.StatusTemporaryRedirect)
}

type OAuthUserInfo struct {
	ProviderUserID string
	Email          string
	Name           string
	AvatarURL      string
	AccessToken    string
	RefreshToken   string
}

func (h *Handler) exchangeGoogleCode(code string) (*OAuthUserInfo, error) {
	callbackBase := h.svcCtx.Config.OAuth.CallbackBaseURL
	if callbackBase == "" {
		callbackBase = h.svcCtx.Config.App.BaseURL
	}

	// Exchange code for tokens
	tokenResp, err := http.PostForm("https://oauth2.googleapis.com/token", url.Values{
		"client_id":     {h.svcCtx.Config.OAuth.GoogleClientID},
		"client_secret": {h.svcCtx.Config.OAuth.GoogleClientSecret},
		"code":          {code},
		"grant_type":    {"authorization_code"},
		"redirect_uri":  {callbackBase + "/oauth/google/callback"},
	})
	if err != nil {
		return nil, fmt.Errorf("failed to exchange code: %w", err)
	}
	defer tokenResp.Body.Close()

	body, _ := io.ReadAll(tokenResp.Body)
	if tokenResp.StatusCode != 200 {
		return nil, fmt.Errorf("token exchange failed: %s", string(body))
	}

	var tokenData struct {
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
	}
	if err := json.Unmarshal(body, &tokenData); err != nil {
		return nil, fmt.Errorf("failed to parse token response: %w", err)
	}

	// Get user info
	userReq, _ := http.NewRequest("GET", "https://www.googleapis.com/oauth2/v2/userinfo", nil)
	userReq.Header.Set("Authorization", "Bearer "+tokenData.AccessToken)
	userResp, err := http.DefaultClient.Do(userReq)
	if err != nil {
		return nil, fmt.Errorf("failed to get user info: %w", err)
	}
	defer userResp.Body.Close()

	body, _ = io.ReadAll(userResp.Body)
	var userData struct {
		ID      string `json:"id"`
		Email   string `json:"email"`
		Name    string `json:"name"`
		Picture string `json:"picture"`
	}
	if err := json.Unmarshal(body, &userData); err != nil {
		return nil, fmt.Errorf("failed to parse user info: %w", err)
	}

	return &OAuthUserInfo{
		ProviderUserID: userData.ID,
		Email:          userData.Email,
		Name:           userData.Name,
		AvatarURL:      userData.Picture,
		AccessToken:    tokenData.AccessToken,
		RefreshToken:   tokenData.RefreshToken,
	}, nil
}

func (h *Handler) exchangeGitHubCode(code string) (*OAuthUserInfo, error) {
	// Exchange code for token
	tokenReq, _ := http.NewRequest("POST", "https://github.com/login/oauth/access_token", strings.NewReader(url.Values{
		"client_id":     {h.svcCtx.Config.OAuth.GitHubClientID},
		"client_secret": {h.svcCtx.Config.OAuth.GitHubClientSecret},
		"code":          {code},
	}.Encode()))
	tokenReq.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	tokenReq.Header.Set("Accept", "application/json")

	tokenResp, err := http.DefaultClient.Do(tokenReq)
	if err != nil {
		return nil, fmt.Errorf("failed to exchange code: %w", err)
	}
	defer tokenResp.Body.Close()

	body, _ := io.ReadAll(tokenResp.Body)
	var tokenData struct {
		AccessToken string `json:"access_token"`
		Error       string `json:"error"`
	}
	if err := json.Unmarshal(body, &tokenData); err != nil {
		return nil, fmt.Errorf("failed to parse token response: %w", err)
	}
	if tokenData.Error != "" {
		return nil, fmt.Errorf("token exchange failed: %s", tokenData.Error)
	}

	// Get user info
	userReq, _ := http.NewRequest("GET", "https://api.github.com/user", nil)
	userReq.Header.Set("Authorization", "Bearer "+tokenData.AccessToken)
	userReq.Header.Set("Accept", "application/json")
	userResp, err := http.DefaultClient.Do(userReq)
	if err != nil {
		return nil, fmt.Errorf("failed to get user info: %w", err)
	}
	defer userResp.Body.Close()

	body, _ = io.ReadAll(userResp.Body)
	var userData struct {
		ID        int    `json:"id"`
		Login     string `json:"login"`
		Email     string `json:"email"`
		Name      string `json:"name"`
		AvatarURL string `json:"avatar_url"`
	}
	if err := json.Unmarshal(body, &userData); err != nil {
		return nil, fmt.Errorf("failed to parse user info: %w", err)
	}

	// GitHub may not return email, fetch from emails endpoint
	email := userData.Email
	if email == "" {
		email, _ = h.getGitHubPrimaryEmail(tokenData.AccessToken)
	}

	name := userData.Name
	if name == "" {
		name = userData.Login
	}

	return &OAuthUserInfo{
		ProviderUserID: fmt.Sprintf("%d", userData.ID),
		Email:          email,
		Name:           name,
		AvatarURL:      userData.AvatarURL,
		AccessToken:    tokenData.AccessToken,
	}, nil
}

func (h *Handler) getGitHubPrimaryEmail(accessToken string) (string, error) {
	req, _ := http.NewRequest("GET", "https://api.github.com/user/emails", nil)
	req.Header.Set("Authorization", "Bearer "+accessToken)
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var emails []struct {
		Email    string `json:"email"`
		Primary  bool   `json:"primary"`
		Verified bool   `json:"verified"`
	}
	body, _ := io.ReadAll(resp.Body)
	if err := json.Unmarshal(body, &emails); err != nil {
		return "", err
	}

	for _, e := range emails {
		if e.Primary && e.Verified {
			return e.Email, nil
		}
	}
	for _, e := range emails {
		if e.Verified {
			return e.Email, nil
		}
	}
	return "", fmt.Errorf("no verified email found")
}
