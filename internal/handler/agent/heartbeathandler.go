package agent

import (
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"

	"nebo/internal/httputil"
	"nebo/internal/svc"
)

// GetHeartbeatHandler returns the contents of HEARTBEAT.md
func GetHeartbeatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		home, err := os.UserHomeDir()
		if err != nil {
			httputil.Error(w, err)
			return
		}

		path := filepath.Join(home, ".nebo", "HEARTBEAT.md")
		content, _ := os.ReadFile(path) // Ignore error - file may not exist yet

		httputil.OkJSON(w, map[string]string{"content": string(content)})
	}
}

// UpdateHeartbeatHandler updates the contents of HEARTBEAT.md
func UpdateHeartbeatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Content string `json:"content"`
		}

		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			httputil.Error(w, err)
			return
		}

		home, err := os.UserHomeDir()
		if err != nil {
			httputil.Error(w, err)
			return
		}

		dir := filepath.Join(home, ".nebo")
		if err := os.MkdirAll(dir, 0755); err != nil {
			httputil.Error(w, err)
			return
		}

		path := filepath.Join(dir, "HEARTBEAT.md")
		if err := os.WriteFile(path, []byte(req.Content), 0644); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, map[string]bool{"success": true})
	}
}
