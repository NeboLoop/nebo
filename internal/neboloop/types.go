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
	InstallCount int     `json:"install_count"`
	Rating       float64 `json:"rating_avg"`
	ReviewCount  int     `json:"rating_count"`
	IsInstalled  bool    `json:"isInstalled"`
	Status       string  `json:"status"`
}

// SkillDetail extends SkillItem with manifest (returned by GET /skills/{id}).
type SkillDetail struct {
	SkillItem
	ManifestURL string          `json:"manifest_url,omitempty"`
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
	BotID   string `json:"bot_id,omitempty"` // Nebo-generated, immutable — server uses this instead of generating one
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

// --------------------------------------------------------------------------
// Skill Install Code Types
// --------------------------------------------------------------------------

// RedeemSkillCodeRequest is sent to POST /api/v1/skills/redeem.
type RedeemSkillCodeRequest struct {
	Code  string `json:"code"`
	BotID string `json:"bot_id"`
}

// --------------------------------------------------------------------------
// Loop Channel Types
// --------------------------------------------------------------------------

// LoopChannel describes a channel the bot belongs to within a Loop.
type LoopChannel struct {
	ChannelID      string `json:"channel_id"`
	ChannelName    string `json:"channel_name"`
	LoopID         string `json:"loop_id"`
	LoopName       string `json:"loop_name"`
	ConversationID string `json:"conversation_id"`
}

// LoopChannelsResponse is the wrapper returned by GET /api/v1/bots/{id}/channels.
type LoopChannelsResponse struct {
	Channels []LoopChannel `json:"channels"`
}

// --------------------------------------------------------------------------
// Loop Query Types (Bot Query System)
// --------------------------------------------------------------------------

// Loop describes a loop the bot belongs to.
type Loop struct {
	ID          string `json:"loop_id"`
	Name        string `json:"loop_name"`
	Description string `json:"description,omitempty"`
	MemberCount int    `json:"member_count,omitempty"`
}

// LoopsResponse is returned by GET /api/v1/bots/{id}/loops.
type LoopsResponse struct {
	Loops []Loop `json:"loops"`
}

// LoopMember describes a member in a loop with online presence.
type LoopMember struct {
	BotID    string `json:"bot_id"`
	BotName  string `json:"bot_name,omitempty"`
	Role     string `json:"role,omitempty"`
	IsOnline bool   `json:"is_online"`
}

// LoopMembersResponse is returned by GET /api/v1/bots/{id}/loops/{loopID}/members.
type LoopMembersResponse struct {
	Members []LoopMember `json:"members"`
}

// ChannelMember describes a member in a channel with online presence.
type ChannelMember struct {
	BotID    string `json:"bot_id"`
	BotName  string `json:"bot_name,omitempty"`
	Role     string `json:"role,omitempty"`
	IsOnline bool   `json:"is_online"`
}

// ChannelMembersResponse is returned by GET /api/v1/bots/{id}/channels/{channelID}/members.
type ChannelMembersResponse struct {
	Members []ChannelMember `json:"members"`
}

// channelMessageRaw is the raw wire format from the NeboLoop channel messages API.
type channelMessageRaw struct {
	MsgID     string `json:"msg_id"`
	SenderID  string `json:"sender_id"`
	Payload   string `json:"payload"` // JSON string containing sender_id, stream, content.text
	CreatedAt string `json:"created_at"`
	Seq       int    `json:"seq"`
	Stream    string `json:"stream"`
}

// channelPayload is the nested JSON inside channelMessageRaw.Payload.
type channelPayload struct {
	SenderID string `json:"sender_id"`
	Content  struct {
		Text string `json:"text"`
	} `json:"content"`
}

// ChannelMessageItem is a normalized message from channel history.
type ChannelMessageItem struct {
	ID        string `json:"id"`
	From      string `json:"from"`
	Content   string `json:"content"`
	CreatedAt string `json:"created_at"`
}

// ChannelMessagesResponse is returned by GET /api/v1/bots/{id}/channels/{channelID}/messages.
type ChannelMessagesResponse struct {
	Messages []channelMessageRaw `json:"messages"`
}

// Normalize converts raw API messages into clean ChannelMessageItems.
func (r *ChannelMessagesResponse) Normalize() []ChannelMessageItem {
	result := make([]ChannelMessageItem, len(r.Messages))
	for i, raw := range r.Messages {
		item := ChannelMessageItem{
			ID:        raw.MsgID,
			From:      raw.SenderID,
			CreatedAt: raw.CreatedAt,
		}
		// Extract text from the nested payload JSON
		var p channelPayload
		if json.Unmarshal([]byte(raw.Payload), &p) == nil {
			item.Content = p.Content.Text
		}
		result[i] = item
	}
	return result
}

// --------------------------------------------------------------------------
// Loop Invite Types
// --------------------------------------------------------------------------

// JoinLoopRequest is sent to POST /api/v1/loops/join.
type JoinLoopRequest struct {
	Code  string `json:"code"`
	BotID string `json:"bot_id"`
}

// JoinLoopResponse is returned by POST /api/v1/loops/join.
type JoinLoopResponse struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}
