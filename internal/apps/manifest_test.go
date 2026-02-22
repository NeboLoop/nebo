package apps

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadManifest(t *testing.T) {
	dir := t.TempDir()

	manifest := AppManifest{
		ID:       "com.test.app",
		Name:     "Test App",
		Version:  "1.0.0",
		Runtime:  "local",
		Protocol: "grpc",
		Provides: []string{"gateway"},
		Permissions: []string{
			"network:api.example.com:443",
			"settings:read",
		},
	}

	data, _ := json.Marshal(manifest)
	os.WriteFile(filepath.Join(dir, "manifest.json"), data, 0644)

	m, err := LoadManifest(dir)
	if err != nil {
		t.Fatalf("LoadManifest() error = %v", err)
	}
	if m.ID != "com.test.app" {
		t.Errorf("ID = %q, want %q", m.ID, "com.test.app")
	}
	if m.Name != "Test App" {
		t.Errorf("Name = %q, want %q", m.Name, "Test App")
	}
	if len(m.Provides) != 1 || m.Provides[0] != "gateway" {
		t.Errorf("Provides = %v, want [gateway]", m.Provides)
	}
}

func TestLoadManifest_MissingFile(t *testing.T) {
	dir := t.TempDir()
	_, err := LoadManifest(dir)
	if err == nil {
		t.Fatal("expected error for missing manifest.json")
	}
}

func TestLoadManifest_InvalidJSON(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "manifest.json"), []byte("not json"), 0644)
	_, err := LoadManifest(dir)
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

func TestValidateManifest(t *testing.T) {
	tests := []struct {
		name    string
		modify  func(*AppManifest)
		wantErr string
	}{
		{
			name:   "valid manifest",
			modify: func(m *AppManifest) {},
		},
		{
			name:    "missing id",
			modify:  func(m *AppManifest) { m.ID = "" },
			wantErr: "missing required field: id",
		},
		{
			name:    "missing name",
			modify:  func(m *AppManifest) { m.Name = "" },
			wantErr: "missing required field: name",
		},
		{
			name:    "missing version",
			modify:  func(m *AppManifest) { m.Version = "" },
			wantErr: "missing required field: version",
		},
		{
			name:    "no provides",
			modify:  func(m *AppManifest) { m.Provides = nil },
			wantErr: "must declare at least one capability",
		},
		{
			name:    "invalid protocol",
			modify:  func(m *AppManifest) { m.Protocol = "rest" },
			wantErr: "unsupported protocol",
		},
		{
			name:    "invalid runtime",
			modify:  func(m *AppManifest) { m.Runtime = "cloud" },
			wantErr: "unsupported runtime",
		},
		{
			name:    "invalid capability",
			modify:  func(m *AppManifest) { m.Provides = []string{"unknown"} },
			wantErr: "invalid capability",
		},
		{
			name:    "invalid permission",
			modify:  func(m *AppManifest) { m.Permissions = []string{"invalid:foo"} },
			wantErr: "invalid permission",
		},
		{
			name:   "empty protocol is valid",
			modify: func(m *AppManifest) { m.Protocol = "" },
		},
		{
			name:   "empty runtime is valid",
			modify: func(m *AppManifest) { m.Runtime = "" },
		},
		{
			name:   "startup_timeout zero is valid (default)",
			modify: func(m *AppManifest) { m.StartupTimeout = 0 },
		},
		{
			name:   "startup_timeout 60 is valid",
			modify: func(m *AppManifest) { m.StartupTimeout = 60 },
		},
		{
			name:   "startup_timeout 120 is valid (max)",
			modify: func(m *AppManifest) { m.StartupTimeout = 120 },
		},
		{
			name:    "startup_timeout too high",
			modify:  func(m *AppManifest) { m.StartupTimeout = 200 },
			wantErr: "startup_timeout must be between 0 and 120",
		},
		{
			name:    "startup_timeout negative",
			modify:  func(m *AppManifest) { m.StartupTimeout = -1 },
			wantErr: "startup_timeout must be between 0 and 120",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &AppManifest{
				ID:          "com.test.app",
				Name:        "Test",
				Version:     "1.0.0",
				Provides:    []string{"gateway"},
				Permissions: []string{"network:example.com:443"},
			}
			tt.modify(m)

			err := ValidateManifest(m)
			if tt.wantErr == "" {
				if err != nil {
					t.Errorf("unexpected error: %v", err)
				}
			} else {
				if err == nil {
					t.Errorf("expected error containing %q, got nil", tt.wantErr)
				} else if !contains(err.Error(), tt.wantErr) {
					t.Errorf("error = %q, want containing %q", err.Error(), tt.wantErr)
				}
			}
		})
	}
}

func TestHasCapability(t *testing.T) {
	m := &AppManifest{Provides: []string{"gateway", "tool:search", "comm"}}

	if !HasCapability(m, "gateway") {
		t.Error("should have gateway capability")
	}
	if !HasCapability(m, "tool:search") {
		t.Error("should have tool:search capability")
	}
	if HasCapability(m, "vision") {
		t.Error("should not have vision capability")
	}
}

func TestHasCapabilityPrefix(t *testing.T) {
	m := &AppManifest{Provides: []string{"gateway", "tool:search", "channel:discord"}}

	if !HasCapabilityPrefix(m, "tool:") {
		t.Error("should match tool: prefix")
	}
	if !HasCapabilityPrefix(m, "channel:") {
		t.Error("should match channel: prefix")
	}
	if HasCapabilityPrefix(m, "comm:") {
		t.Error("should not match comm: prefix")
	}
}

func TestCheckPermission(t *testing.T) {
	m := &AppManifest{
		Permissions: []string{
			"network:api.example.com:443",
			"settings:read",
			"user:*",
		},
	}

	// Exact match
	if !CheckPermission(m, "network:api.example.com:443") {
		t.Error("should match exact permission")
	}
	if !CheckPermission(m, "settings:read") {
		t.Error("should match exact permission")
	}

	// Wildcard match
	if !CheckPermission(m, "user:token") {
		t.Error("user:* should match user:token")
	}
	if !CheckPermission(m, "user:id") {
		t.Error("user:* should match user:id")
	}

	// No match
	if CheckPermission(m, "network:other.com:443") {
		t.Error("should not match different network permission")
	}
	if CheckPermission(m, "shell:exec") {
		t.Error("should not match unlisted permission")
	}
}

func TestHasPermissionPrefix(t *testing.T) {
	m := &AppManifest{
		Permissions: []string{
			"network:api.example.com:443",
			"settings:read",
			"comm:send",
		},
	}

	if !HasPermissionPrefix(m, "network:") {
		t.Error("should match network: prefix")
	}
	if !HasPermissionPrefix(m, "comm:") {
		t.Error("should match comm: prefix")
	}
	if HasPermissionPrefix(m, "shell:") {
		t.Error("should not match shell: prefix")
	}
}

func TestIsValidCapability(t *testing.T) {
	valid := []string{"gateway", "vision", "browser", "comm", "tool:search", "channel:discord"}
	for _, cap := range valid {
		if !isValidCapability(cap) {
			t.Errorf("isValidCapability(%q) = false, want true", cap)
		}
	}

	invalid := []string{"unknown", "tool", "channel", ""}
	for _, cap := range invalid {
		if isValidCapability(cap) {
			t.Errorf("isValidCapability(%q) = true, want false", cap)
		}
	}
}

func TestIsValidPermission(t *testing.T) {
	valid := []string{
		"network:example.com:443",
		"network:*",
		"network:outbound",
		"settings:read",
		"memory:read",
		"session:create",
		"tool:shell",
		"shell:exec",
		"channel:send",
		"comm:send",
		"model:chat",
		"user:token",
		"database:read",
		"schedule:create",
		"oauth:google",
	}
	for _, perm := range valid {
		if !isValidPermission(perm) {
			t.Errorf("isValidPermission(%q) = false, want true", perm)
		}
	}

	invalid := []string{
		"bogus:foo",
		"admin:root",
		"",
		"noprefixhere",
		"memory:banana",
		"shell:rm",
		"network:",
		"settings:",
	}
	for _, perm := range invalid {
		if isValidPermission(perm) {
			t.Errorf("isValidPermission(%q) = true, want false", perm)
		}
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || containsStr(s, substr))
}

func containsStr(s, sub string) bool {
	for i := 0; i <= len(s)-len(sub); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}
