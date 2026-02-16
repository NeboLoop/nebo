package agent

import (
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"

	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
)

// GetHeartbeatHandler returns the contents of HEARTBEAT.md
func GetHeartbeatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		dataDir, err := defaults.DataDir()
		if err != nil {
			httputil.Error(w, err)
			return
		}

		path := filepath.Join(dataDir, "HEARTBEAT.md")
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

		dataDir, err := defaults.DataDir()
		if err != nil {
			httputil.Error(w, err)
			return
		}

		if err := os.MkdirAll(dataDir, 0755); err != nil {
			httputil.Error(w, err)
			return
		}

		path := filepath.Join(dataDir, "HEARTBEAT.md")
		if err := os.WriteFile(path, []byte(req.Content), 0644); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, map[string]bool{"success": true})
	}
}
