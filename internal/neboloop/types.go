package neboloop

import "encoding/json"

// Author represents the developer who published an app or skill.
type Author struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Verified bool   `json:"verified"`
}

// AppItem is the compact representation returned in list responses.
type AppItem struct {
	ID           string  `json:"id"`
	Name         string  `json:"name"`
	Slug         string  `json:"slug"`
	Description  string  `json:"description"`
	Icon         string  `json:"icon"`
	Category     string  `json:"category"`
	Version      string  `json:"version"`
	Author       Author  `json:"author"`
	InstallCount int     `json:"installCount"`
	Rating       float64 `json:"rating"`
	ReviewCount  int     `json:"reviewCount"`
	IsInstalled  bool    `json:"isInstalled"`
	Status       string  `json:"status"`
}

// AppDetail extends AppItem with manifest (returned by GET /apps/{id}).
type AppDetail struct {
	AppItem
	ManifestURL  string           `json:"manifestUrl,omitempty"`
	Manifest     json.RawMessage  `json:"manifest,omitempty"`
	AgeRating    string           `json:"ageRating,omitempty"`
	Platforms    []string         `json:"platforms,omitempty"`
	Size         map[string]int   `json:"size,omitempty"`
	Language     string           `json:"language,omitempty"`
	Screenshots  []string         `json:"screenshots,omitempty"`
	Changelog    []ChangelogEntry `json:"changelog,omitempty"`
	WebsiteURL   string           `json:"websiteUrl,omitempty"`
	PrivacyURL   string           `json:"privacyUrl,omitempty"`
	SupportURL   string           `json:"supportUrl,omitempty"`
}

// ChangelogEntry represents a single version entry in an app's changelog.
type ChangelogEntry struct {
	Version string `json:"version"`
	Date    string `json:"date"`
	Notes   string `json:"notes"`
}

// ReviewsResponse is the paginated response for GET /api/v1/apps/{id}/reviews.
type ReviewsResponse struct {
	Reviews      []Review `json:"reviews"`
	TotalCount   int      `json:"totalCount"`
	Average      float64  `json:"average"`
	Distribution [5]int   `json:"distribution"`
}

// Review represents a single user review of an app.
type Review struct {
	ID        string `json:"id"`
	UserName  string `json:"userName"`
	Rating    int    `json:"rating"`
	Title     string `json:"title"`
	Body      string `json:"body"`
	CreatedAt string `json:"createdAt"`
	Helpful   int    `json:"helpful"`
}

// SkillItem is the compact representation returned in list responses.
type SkillItem struct {
	ID           string  `json:"id"`
	Name         string  `json:"name"`
	Slug         string  `json:"slug"`
	Description  string  `json:"description"`
	Icon         string  `json:"icon"`
	Category     string  `json:"category"`
	Version      string  `json:"version"`
	Author       Author  `json:"author"`
	InstallCount int     `json:"installCount"`
	Rating       float64 `json:"rating"`
	ReviewCount  int     `json:"reviewCount"`
	IsInstalled  bool    `json:"isInstalled"`
	Status       string  `json:"status"`
}

// SkillDetail extends SkillItem with manifest (returned by GET /skills/{id}).
type SkillDetail struct {
	SkillItem
	ManifestURL string          `json:"manifestUrl,omitempty"`
	Manifest    json.RawMessage `json:"manifest,omitempty"`
}

// AppsResponse is the paginated list response for GET /api/v1/apps.
type AppsResponse struct {
	Apps       []AppItem `json:"apps"`
	TotalCount int       `json:"totalCount"`
	Page       int       `json:"page"`
	PageSize   int       `json:"pageSize"`
}

// SkillsResponse is the paginated list response for GET /api/v1/skills.
type SkillsResponse struct {
	Skills     []SkillItem `json:"skills"`
	TotalCount int         `json:"totalCount"`
	Page       int         `json:"page"`
	PageSize   int         `json:"pageSize"`
}

// InstallResponseApp is the app data returned in an install response.
type InstallResponseApp struct {
	ID       string          `json:"id"`
	Name     string          `json:"name"`
	Slug     string          `json:"slug"`
	Version  string          `json:"version"`
	Manifest json.RawMessage `json:"manifest,omitempty"`
}

// InstallResponse is returned by POST /api/v1/apps/{id}/install
// and POST /api/v1/skills/{id}/install.
type InstallResponse struct {
	ID              string              `json:"id"`
	App             *InstallResponseApp `json:"app,omitempty"`
	Skill           *InstallResponseApp `json:"skill,omitempty"`
	InstalledAt     string              `json:"installedAt"`
	UpdateAvailable bool                `json:"updateAvailable"`
}

// --------------------------------------------------------------------------
// Bot Identity Types
// --------------------------------------------------------------------------

// UpdateBotIdentityRequest is sent to PUT /api/v1/bots/{id}.
type UpdateBotIdentityRequest struct {
	Name string `json:"name,omitempty"`
	Role string `json:"role,omitempty"`
}

// --------------------------------------------------------------------------
// Connection Code Types
// --------------------------------------------------------------------------

// RedeemCodeRequest is sent to POST /api/v1/bots/connect/redeem.
type RedeemCodeRequest struct {
	Code    string `json:"code"`
	Name    string `json:"name"`
	Purpose string `json:"purpose"`
}

// RedeemCodeResponse is returned by POST /api/v1/bots/connect/redeem.
type RedeemCodeResponse struct {
	ID              string `json:"id"`
	Name            string `json:"name"`
	Slug            string `json:"slug"`
	Purpose         string `json:"purpose"`
	Visibility      string `json:"visibility"`
	ConnectionToken string `json:"connection_token"`
}

// ExchangeTokenRequest is sent to POST /api/v1/bots/exchange-token.
type ExchangeTokenRequest struct {
	Token string `json:"token"`
}

// ExchangeTokenResponse is returned by POST /api/v1/bots/exchange-token.
type ExchangeTokenResponse struct {
	MQTTUsername string `json:"mqtt_username"`
	MQTTPassword string `json:"mqtt_password"`
	MQTTBroker   string `json:"mqtt_broker,omitempty"`
}
