package apps

import (
	"os"
	"strings"
	"testing"
)

func TestPermissionDiff(t *testing.T) {
	tests := []struct {
		name     string
		old      []string
		new      []string
		wantAdds []string
	}{
		{
			name:     "no changes",
			old:      []string{"network:api.com:443", "settings:endpoint"},
			new:      []string{"network:api.com:443", "settings:endpoint"},
			wantAdds: nil,
		},
		{
			name:     "new permissions added",
			old:      []string{"network:api.com:443"},
			new:      []string{"network:api.com:443", "user:token", "shell:exec"},
			wantAdds: []string{"user:token", "shell:exec"},
		},
		{
			name:     "permissions removed (no adds)",
			old:      []string{"network:api.com:443", "shell:exec"},
			new:      []string{"network:api.com:443"},
			wantAdds: nil,
		},
		{
			name:     "from empty",
			old:      nil,
			new:      []string{"network:api.com:443"},
			wantAdds: []string{"network:api.com:443"},
		},
		{
			name:     "to empty",
			old:      []string{"network:api.com:443"},
			new:      nil,
			wantAdds: nil,
		},
		{
			name:     "both empty",
			old:      nil,
			new:      nil,
			wantAdds: nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			added := permissionDiff(tt.old, tt.new)
			if len(added) != len(tt.wantAdds) {
				t.Errorf("permissionDiff() = %v (len %d), want %v (len %d)",
					added, len(added), tt.wantAdds, len(tt.wantAdds))
				return
			}
			for i, a := range added {
				if a != tt.wantAdds[i] {
					t.Errorf("added[%d] = %q, want %q", i, a, tt.wantAdds[i])
				}
			}
		})
	}
}

func TestBrokerToInstallURL(t *testing.T) {
	tests := []struct {
		name    string
		broker  string
		want    string // expected scheme
		wantErr bool
	}{
		{
			name:   "tcp scheme converts to mqtt",
			broker: "tcp://localhost:1883",
			want:   "mqtt",
		},
		{
			name:   "mqtt scheme stays",
			broker: "mqtt://broker.example.com:1883",
			want:   "mqtt",
		},
		{
			name:   "mqtts scheme stays",
			broker: "mqtts://broker.example.com:8883",
			want:   "mqtts",
		},
		{
			name:   "ssl converts to mqtts",
			broker: "ssl://broker.example.com:8883",
			want:   "mqtts",
		},
		{
			name:   "tls converts to mqtts",
			broker: "tls://broker.example.com:8883",
			want:   "mqtts",
		},
		{
			name:   "ws scheme stays",
			broker: "ws://broker.example.com/mqtt",
			want:   "ws",
		},
		{
			name:   "wss scheme stays",
			broker: "wss://broker.example.com/mqtt",
			want:   "wss",
		},
		{
			name:   "no scheme defaults to mqtt",
			broker: "localhost:1883",
			want:   "mqtt",
		},
		{
			name:    "unsupported scheme",
			broker:  "http://localhost:1883",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			u, err := brokerToInstallURL(tt.broker)
			if tt.wantErr {
				if err == nil {
					t.Fatal("expected error")
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if u.Scheme != tt.want {
				t.Errorf("scheme = %q, want %q", u.Scheme, tt.want)
			}
		})
	}
}

func TestDownloadURL(t *testing.T) {
	il := &InstallListener{
		config: InstallListenerConfig{
			APIServer: "https://api.neboloop.com",
		},
	}

	t.Run("uses event download URL if provided", func(t *testing.T) {
		event := installEvent{
			AppID:       "com.test.app",
			Version:     "1.0.0",
			DownloadURL: "https://cdn.neboloop.com/apps/test.napp",
		}
		got := il.downloadURL(event)
		if got != "https://cdn.neboloop.com/apps/test.napp" {
			t.Errorf("downloadURL() = %q, want CDN URL", got)
		}
	})

	t.Run("constructs URL from API server", func(t *testing.T) {
		event := installEvent{
			AppID:   "com.test.app",
			Version: "2.0.0",
		}
		got := il.downloadURL(event)
		want := "https://api.neboloop.com/api/v1/apps/com.test.app/download?version=2.0.0"
		if got != want {
			t.Errorf("downloadURL() = %q, want %q", got, want)
		}
	})

	t.Run("strips trailing slash from API server", func(t *testing.T) {
		il2 := &InstallListener{
			config: InstallListenerConfig{
				APIServer: "https://api.neboloop.com/",
			},
		}
		event := installEvent{AppID: "test", Version: "1.0"}
		got := il2.downloadURL(event)
		if strings.Contains(got, "//api/") {
			t.Errorf("downloadURL() has double slash: %q", got)
		}
	})

	t.Run("no API server returns empty", func(t *testing.T) {
		il3 := &InstallListener{
			config: InstallListenerConfig{},
		}
		event := installEvent{AppID: "test", Version: "1.0"}
		got := il3.downloadURL(event)
		if got != "" {
			t.Errorf("downloadURL() = %q, want empty string", got)
		}
	})
}

func TestDirExists(t *testing.T) {
	dir := t.TempDir()

	if !dirExists(dir) {
		t.Error("dirExists should return true for existing directory")
	}

	if dirExists(dir + "/nonexistent") {
		t.Error("dirExists should return false for nonexistent path")
	}

	// Create a file (not a directory)
	filePath := dir + "/file.txt"
	os.WriteFile(filePath, []byte("test"), 0644)
	if dirExists(filePath) {
		t.Error("dirExists should return false for a file")
	}
}
