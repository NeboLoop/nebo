package agent

import (
	"net/http"
	"os"
	"os/user"
	"runtime"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// GetSystemInfoHandler returns auto-discovered system information
func GetSystemInfoHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		osName := runtime.GOOS
		switch osName {
		case "darwin":
			osName = "macOS"
		case "linux":
			osName = "Linux"
		case "windows":
			osName = "Windows"
		}

		hostname, _ := os.Hostname()
		homeDir, _ := os.UserHomeDir()
		username := ""
		if u, err := user.Current(); err == nil {
			username = u.Username
		}

		httputil.OkJSON(w, &types.SystemInfoResponse{
			OS:       osName,
			Arch:     runtime.GOARCH,
			Hostname: hostname,
			HomeDir:  homeDir,
			Username: username,
		})
	}
}
