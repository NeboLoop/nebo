package plugins

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/neboloop"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// ListPluginsHandler returns all registered plugins with their settings.
func ListPluginsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		pluginType := r.URL.Query().Get("type")

		var rows []db.PluginRegistry
		var err error
		if pluginType != "" {
			rows, err = svcCtx.DB.ListPluginsByType(r.Context(), pluginType)
		} else {
			rows, err = svcCtx.DB.ListPlugins(r.Context())
		}
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.PluginItem, 0, len(rows))
		for _, p := range rows {
			item := toPluginItem(p)
			// Load settings for each plugin
			settings, _ := svcCtx.DB.ListPluginSettings(r.Context(), p.ID)
			if len(settings) > 0 {
				item.Settings = make(map[string]string)
				for _, s := range settings {
					if s.IsSecret != 0 {
						item.Settings[s.SettingKey] = "••••••••" // Mask secrets
					} else {
						item.Settings[s.SettingKey] = s.SettingValue
					}
				}
			}
			result = append(result, item)
		}

		httputil.OkJSON(w, types.ListPluginsResponse{Plugins: result})
	}
}

// GetPluginHandler returns a single plugin with its settings.
func GetPluginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		p, err := svcCtx.DB.GetPlugin(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		item := toPluginItem(p)
		// Load settings
		settings, _ := svcCtx.DB.ListPluginSettings(r.Context(), id)
		if len(settings) > 0 {
			item.Settings = make(map[string]string)
			for _, s := range settings {
				if s.IsSecret != 0 {
					item.Settings[s.SettingKey] = "••••••••"
				} else {
					item.Settings[s.SettingKey] = s.SettingValue
				}
			}
		}

		httputil.OkJSON(w, types.GetPluginResponse{Plugin: item})
	}
}

// UpdatePluginSettingsHandler updates settings for a plugin (upsert pattern).
func UpdatePluginSettingsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		var req types.UpdatePluginSettingsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Verify plugin exists
		p, err := svcCtx.DB.GetPlugin(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Use PluginStore for update (triggers OnSettingsChanged hot-reload)
		if svcCtx.PluginStore != nil {
			if err := svcCtx.PluginStore.UpdateSettings(r.Context(), id, req.Settings, req.Secrets); err != nil {
				httputil.Error(w, err)
				return
			}
		} else {
			// Fallback: raw DB upsert (no hot-reload)
			for key, value := range req.Settings {
				isSecret := int64(0)
				if req.Secrets != nil && req.Secrets[key] {
					isSecret = 1
				}
				_, err := svcCtx.DB.UpsertPluginSetting(r.Context(), db.UpsertPluginSettingParams{
					ID:           uuid.New().String(),
					PluginID:     id,
					SettingKey:   key,
					SettingValue: value,
					IsSecret:     isSecret,
				})
				if err != nil {
					httputil.Error(w, err)
					return
				}
			}
		}

		// Return updated plugin with settings
		item := toPluginItem(p)
		settings, _ := svcCtx.DB.ListPluginSettings(r.Context(), id)
		if len(settings) > 0 {
			item.Settings = make(map[string]string)
			for _, s := range settings {
				if s.IsSecret != 0 {
					item.Settings[s.SettingKey] = "••••••••"
				} else {
					item.Settings[s.SettingKey] = s.SettingValue
				}
			}
		}

		httputil.OkJSON(w, types.UpdatePluginSettingsResponse{Plugin: item})
	}
}

// TogglePluginHandler enables or disables a plugin.
func TogglePluginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		var req types.TogglePluginRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		val := int64(0)
		if req.IsEnabled {
			val = 1
		}
		if err := svcCtx.DB.TogglePlugin(r.Context(), db.TogglePluginParams{
			IsEnabled: val,
			ID:        id,
		}); err != nil {
			httputil.Error(w, err)
			return
		}

		// Return updated plugin
		p, err := svcCtx.DB.GetPlugin(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.GetPluginResponse{Plugin: toPluginItem(p)})
	}
}

func toPluginItem(p db.PluginRegistry) types.PluginItem {
	// Parse settings_manifest JSON, fallback to empty object
	var manifest json.RawMessage
	if p.SettingsManifest != "" && p.SettingsManifest != "{}" {
		manifest = json.RawMessage(p.SettingsManifest)
	} else {
		manifest = json.RawMessage(`{}`)
	}

	// Extract capabilities and permissions from metadata (stored by AppRegistry)
	var capabilities, permissions []string
	if p.Metadata != "" {
		var meta struct {
			Provides    []string `json:"provides"`
			Permissions []string `json:"permissions"`
		}
		if json.Unmarshal([]byte(p.Metadata), &meta) == nil {
			capabilities = meta.Provides
			permissions = meta.Permissions
		}
	}

	return types.PluginItem{
		Id:               p.ID,
		Name:             p.Name,
		PluginType:       p.PluginType,
		DisplayName:      p.DisplayName,
		Description:      p.Description,
		Icon:             p.Icon,
		Version:          p.Version,
		IsEnabled:        p.IsEnabled != 0,
		IsInstalled:      p.IsInstalled != 0,
		SettingsManifest: manifest,
		ConnectionStatus: p.ConnectionStatus,
		LastConnectedAt:  nullTimeString(p.LastConnectedAt),
		LastError:        nullString(p.LastError),
		Capabilities:     capabilities,
		Permissions:      permissions,
		CreatedAt:        time.Unix(p.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt:        time.Unix(p.UpdatedAt, 0).Format(time.RFC3339),
	}
}

func nullString(s sql.NullString) string {
	if s.Valid {
		return s.String
	}
	return ""
}

func nullTimeString(t sql.NullInt64) string {
	if t.Valid && t.Int64 > 0 {
		return time.Unix(t.Int64, 0).Format(time.RFC3339)
	}
	return ""
}

// --------------------------------------------------------------------------
// NeboLoop Store Handlers
// --------------------------------------------------------------------------

// neboLoopClient constructs a neboloop.Client from the stored plugin settings.
func neboLoopClient(ctx context.Context, svcCtx *svc.ServiceContext) (*neboloop.Client, error) {
	if svcCtx.PluginStore == nil {
		return nil, fmt.Errorf("plugin store not initialized")
	}
	settings, err := svcCtx.PluginStore.GetSettingsByName(ctx, "neboloop")
	if err != nil {
		return nil, fmt.Errorf("could not load neboloop settings: %w", err)
	}
	return neboloop.NewClient(settings)
}

// GetStoreAppHandler returns a single app's detail from NeboLoop.
func GetStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		detail, err := client.GetApp(r.Context(), id)
		if err != nil {
			httputil.InternalError(w, "NeboLoop: "+err.Error())
			return
		}

		installed := installedPluginNames(r.Context(), svcCtx)
		httputil.OkJSON(w, types.GetStoreAppResponse{
			App: toStoreAppDetail(detail, installed),
		})
	}
}

// GetStoreAppReviewsHandler returns reviews for an app from NeboLoop.
func GetStoreAppReviewsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		page, _ := strconv.Atoi(r.URL.Query().Get("page"))
		pageSize, _ := strconv.Atoi(r.URL.Query().Get("pageSize"))

		reviews, err := client.GetAppReviews(r.Context(), id, page, pageSize)
		if err != nil {
			httputil.InternalError(w, "NeboLoop: "+err.Error())
			return
		}

		storeReviews := make([]types.StoreReview, 0, len(reviews.Reviews))
		for _, rv := range reviews.Reviews {
			storeReviews = append(storeReviews, types.StoreReview{
				ID:        rv.ID,
				UserName:  rv.UserName,
				Rating:    rv.Rating,
				Title:     rv.Title,
				Body:      rv.Body,
				CreatedAt: rv.CreatedAt,
				Helpful:   rv.Helpful,
			})
		}

		httputil.OkJSON(w, types.GetStoreAppReviewsResponse{
			Reviews:      storeReviews,
			TotalCount:   reviews.TotalCount,
			Average:      reviews.Average,
			Distribution: reviews.Distribution,
		})
	}
}

// ListStoreAppsHandler lists apps from NeboLoop.
func ListStoreAppsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		query := r.URL.Query().Get("q")
		category := r.URL.Query().Get("category")
		page, _ := strconv.Atoi(r.URL.Query().Get("page"))
		pageSize, _ := strconv.Atoi(r.URL.Query().Get("pageSize"))

		upstream, err := client.ListApps(r.Context(), query, category, page, pageSize)
		if err != nil {
			httputil.InternalError(w, "NeboLoop: "+err.Error())
			return
		}

		// Mark locally installed apps
		installed := installedPluginNames(r.Context(), svcCtx)
		apps := make([]types.StoreApp, 0, len(upstream.Apps))
		for _, a := range upstream.Apps {
			apps = append(apps, toStoreApp(a, installed))
		}

		httputil.OkJSON(w, types.ListStoreAppsResponse{
			Apps:       apps,
			TotalCount: upstream.TotalCount,
			Page:       upstream.Page,
			PageSize:   upstream.PageSize,
		})
	}
}

// ListStoreSkillsHandler lists skills from NeboLoop.
func ListStoreSkillsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		query := r.URL.Query().Get("q")
		category := r.URL.Query().Get("category")
		page, _ := strconv.Atoi(r.URL.Query().Get("page"))
		pageSize, _ := strconv.Atoi(r.URL.Query().Get("pageSize"))

		upstream, err := client.ListSkills(r.Context(), query, category, page, pageSize)
		if err != nil {
			httputil.InternalError(w, "NeboLoop: "+err.Error())
			return
		}

		installed := installedPluginNames(r.Context(), svcCtx)
		skills := make([]types.StoreSkill, 0, len(upstream.Skills))
		for _, s := range upstream.Skills {
			skills = append(skills, toStoreSkill(s, installed))
		}

		httputil.OkJSON(w, types.ListStoreSkillsResponse{
			Skills:     skills,
			TotalCount: upstream.TotalCount,
			Page:       upstream.Page,
			PageSize:   upstream.PageSize,
		})
	}
}

// InstallStoreAppHandler installs an app from NeboLoop.
func InstallStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		result, err := client.InstallApp(r.Context(), id)
		if err != nil {
			httputil.InternalError(w, "install failed: "+err.Error())
			return
		}

		// Create local plugin_registry row from install response
		pluginID, err := createLocalPlugin(r.Context(), svcCtx, result, "app")
		if err != nil {
			httputil.InternalError(w, "failed to register locally: "+err.Error())
			return
		}

		// Trigger actual download and launch in background (don't rely solely on MQTT)
		if reg := svcCtx.AppRegistry(); reg != nil && result.App != nil {
			type installer interface {
				InstallFromURL(ctx context.Context, downloadURL string) error
			}
			if inst, ok := reg.(installer); ok {
				downloadURL := client.APIServer() + "/api/v1/apps/" + id + "/download"
				if result.App.Version != "" {
					downloadURL += "?version=" + result.App.Version
				}
				go func() {
					bgCtx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
					defer cancel()
					if err := inst.InstallFromURL(bgCtx, downloadURL); err != nil {
						fmt.Printf("[plugins] Background install failed for %s: %v\n", id, err)
					}
				}()
			}
		}

		httputil.OkJSON(w, types.InstallStoreAppResponse{
			PluginID: pluginID,
			Message:  "app installed",
		})
	}
}

// UninstallStoreAppHandler uninstalls an app.
func UninstallStoreAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		if err := client.UninstallApp(r.Context(), id); err != nil {
			httputil.InternalError(w, "uninstall failed: "+err.Error())
			return
		}

		// Remove local plugin_registry row by store_id
		removeLocalPluginByStoreID(r.Context(), svcCtx, id)

		httputil.OkJSON(w, map[string]string{"message": "app uninstalled"})
	}
}

// InstallStoreSkillHandler installs a skill from NeboLoop.
func InstallStoreSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		result, err := client.InstallSkill(r.Context(), id)
		if err != nil {
			httputil.InternalError(w, "install failed: "+err.Error())
			return
		}

		pluginID, err := createLocalPlugin(r.Context(), svcCtx, result, "skill")
		if err != nil {
			httputil.InternalError(w, "failed to register locally: "+err.Error())
			return
		}

		httputil.OkJSON(w, types.InstallStoreSkillResponse{
			PluginID: pluginID,
			Message:  "skill installed",
		})
	}
}

// UninstallStoreSkillHandler uninstalls a skill.
func UninstallStoreSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		client, err := neboLoopClient(r.Context(), svcCtx)
		if err != nil {
			httputil.BadRequest(w, "NeboLoop not configured: "+err.Error())
			return
		}

		if err := client.UninstallSkill(r.Context(), id); err != nil {
			httputil.InternalError(w, "uninstall failed: "+err.Error())
			return
		}

		removeLocalPluginByStoreID(r.Context(), svcCtx, id)

		httputil.OkJSON(w, map[string]string{"message": "skill uninstalled"})
	}
}

// --------------------------------------------------------------------------
// NeboLoop Connection Code Handlers
// --------------------------------------------------------------------------

// NeboLoopConnectHandler redeems a connection code and stores MQTT credentials.
// POST /api/v1/neboloop/connect
func NeboLoopConnectHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.NeboLoopConnectRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if req.Code == "" {
			httputil.BadRequest(w, "code is required")
			return
		}
		if req.Name == "" {
			httputil.BadRequest(w, "name is required")
			return
		}

		if svcCtx.PluginStore == nil {
			httputil.InternalError(w, "plugin store not initialized")
			return
		}

		// Resolve API server: existing setting → config default
		apiServer := svcCtx.Config.NeboLoop.ApiURL
		settings, err := svcCtx.PluginStore.GetSettingsByName(r.Context(), "neboloop")
		if err == nil && settings["api_server"] != "" {
			apiServer = settings["api_server"]
		}

		// Ensure we have an immutable bot_id before redeeming
		botID := settings["bot_id"]
		if botID == "" {
			botID = uuid.New().String()
		}

		// Step 1: Redeem code (pass our immutable bot_id so the server registers it)
		purpose := req.Purpose
		if purpose == "" {
			purpose = "AI companion"
		}
		redeemed, err := neboloop.RedeemCode(r.Context(), apiServer, req.Code, req.Name, purpose, botID)
		if err != nil {
			httputil.BadRequest(w, "redeem failed: "+err.Error())
			return
		}

		// Step 2: Store connection settings (bot_id is our local immutable UUID)
		plugin, err := svcCtx.PluginStore.GetPlugin(r.Context(), "neboloop")
		if err != nil {
			httputil.InternalError(w, "neboloop plugin not registered: "+err.Error())
			return
		}

		newSettings := map[string]string{
			"api_server": apiServer,
			"bot_id":     botID,
		}

		if err := svcCtx.PluginStore.UpdateSettings(r.Context(), plugin.ID, newSettings, nil); err != nil {
			httputil.InternalError(w, "failed to save bot_id: "+err.Error())
			return
		}

		httputil.OkJSON(w, types.NeboLoopConnectResponse{
			BotID:   botID,
			BotName: redeemed.Name,
			BotSlug: redeemed.Slug,
			Message: "Connected to NeboLoop",
		})
	}
}

// NeboLoopStatusHandler returns the current NeboLoop connection status.
// GET /api/v1/neboloop/status
func NeboLoopStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if svcCtx.PluginStore == nil {
			httputil.OkJSON(w, types.NeboLoopStatusResponse{Connected: false})
			return
		}

		settings, err := svcCtx.PluginStore.GetSettingsByName(r.Context(), "neboloop")
		if err != nil {
			httputil.OkJSON(w, types.NeboLoopStatusResponse{Connected: false})
			return
		}

		botID := settings["bot_id"]
		connected := botID != ""

		resp := types.NeboLoopStatusResponse{
			Connected: connected,
			APIServer: settings["api_server"],
		}
		if connected {
			resp.BotID = botID
			// Try to get bot name from plugin connection status
			plugin, err := svcCtx.PluginStore.GetPlugin(r.Context(), "neboloop")
			if err == nil {
				resp.BotName = plugin.DisplayName
			}
		}

		httputil.OkJSON(w, resp)
	}
}

// --------------------------------------------------------------------------
// NeboLoop helpers
// --------------------------------------------------------------------------

// createLocalPlugin creates a local plugin_registry row from a NeboLoop install response.
func createLocalPlugin(ctx context.Context, svcCtx *svc.ServiceContext, result *neboloop.InstallResponse, pluginType string) (string, error) {
	// Determine which item was installed (app or skill)
	item := result.App
	if item == nil {
		item = result.Skill
	}
	if item == nil {
		return "", fmt.Errorf("install response contained no app or skill data")
	}

	pluginID := uuid.New().String()
	manifestStr := "{}"
	if len(item.Manifest) > 0 {
		manifestStr = string(item.Manifest)
	}

	// Store NeboLoop IDs in metadata for uninstall matching and reconciliation
	meta, _ := json.Marshal(map[string]string{
		"store_install_id": result.ID,
		"store_app_id":     item.ID,
	})

	_, err := svcCtx.DB.CreatePlugin(ctx, db.CreatePluginParams{
		ID:               pluginID,
		Name:             item.Slug,
		PluginType:       pluginType,
		DisplayName:      item.Name,
		Description:      "",
		Icon:             "",
		Version:          item.Version,
		IsEnabled:        1,
		IsInstalled:      1,
		SettingsManifest: manifestStr,
		Metadata:         string(meta),
	})
	if err != nil {
		return "", err
	}
	return pluginID, nil
}

// removeLocalPluginByStoreID removes the local plugin_registry row matching a NeboLoop store app ID stored in metadata.
func removeLocalPluginByStoreID(ctx context.Context, svcCtx *svc.ServiceContext, storeID string) {
	rows, err := svcCtx.DB.ListPlugins(ctx)
	if err != nil {
		return
	}
	for _, p := range rows {
		var meta map[string]string
		if err := json.Unmarshal([]byte(p.Metadata), &meta); err == nil {
			if meta["store_app_id"] == storeID || meta["store_install_id"] == storeID {
				_ = svcCtx.DB.DeletePlugin(ctx, p.ID)
				return
			}
		}
	}
}

// installedPluginNames returns a set of locally installed plugin names.
func installedPluginNames(ctx context.Context, svcCtx *svc.ServiceContext) map[string]bool {
	rows, err := svcCtx.DB.ListPlugins(ctx)
	if err != nil {
		return nil
	}
	names := make(map[string]bool, len(rows))
	for _, p := range rows {
		if p.IsInstalled != 0 {
			names[p.Name] = true
		}
	}
	return names
}

// toStoreApp converts a NeboLoop AppItem to a types.StoreApp, marking installed status.
func toStoreApp(a neboloop.AppItem, installed map[string]bool) types.StoreApp {
	return types.StoreApp{
		ID:           a.ID,
		Name:         a.Name,
		Slug:         a.Slug,
		Description:  a.Description,
		Icon:         a.Icon,
		Category:     a.Category,
		Version:      a.Version,
		Author:       types.StoreAuthor{ID: a.Author.ID, Name: a.Author.Name, Verified: a.Author.Verified},
		InstallCount: a.InstallCount,
		Rating:       a.Rating,
		ReviewCount:  a.ReviewCount,
		IsInstalled:  a.IsInstalled || installed[a.Slug],
		Status:       a.Status,
	}
}

// toStoreAppDetail converts a NeboLoop AppDetail to a types.StoreAppDetail, marking installed status.
func toStoreAppDetail(d *neboloop.AppDetail, installed map[string]bool) types.StoreAppDetail {
	changelog := make([]types.StoreChangelog, 0, len(d.Changelog))
	for _, c := range d.Changelog {
		changelog = append(changelog, types.StoreChangelog{
			Version: c.Version,
			Date:    c.Date,
			Notes:   c.Notes,
		})
	}

	return types.StoreAppDetail{
		StoreApp:    toStoreApp(d.AppItem, installed),
		AgeRating:   d.AgeRating,
		Platforms:   d.Platforms,
		Size:        d.Size,
		Language:    d.Language,
		Screenshots: d.Screenshots,
		Changelog:   changelog,
		WebsiteURL:  d.WebsiteURL,
		PrivacyURL:  d.PrivacyURL,
		SupportURL:  d.SupportURL,
	}
}

// toStoreSkill converts a NeboLoop SkillItem to a types.StoreSkill, marking installed status.
func toStoreSkill(s neboloop.SkillItem, installed map[string]bool) types.StoreSkill {
	return types.StoreSkill{
		ID:           s.ID,
		Name:         s.Name,
		Slug:         s.Slug,
		Description:  s.Description,
		Icon:         s.Icon,
		Category:     s.Category,
		Version:      s.Version,
		Author:       types.StoreAuthor{ID: s.Author.ID, Name: s.Author.Name, Verified: s.Author.Verified},
		InstallCount: s.InstallCount,
		Rating:       s.Rating,
		ReviewCount:  s.ReviewCount,
		IsInstalled:  s.IsInstalled || installed[s.Slug],
		Status:       s.Status,
	}
}
