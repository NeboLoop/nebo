package defaults

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestListDefaults(t *testing.T) {
	files, err := ListDefaults()
	if err != nil {
		t.Fatalf("ListDefaults failed: %v", err)
	}

	expected := []string{"config.yaml", "models.yaml", "SOUL.md", "HEARTBEAT.md"}
	if len(files) != len(expected) {
		t.Errorf("Expected %d files, got %d: %v", len(expected), len(files), files)
	}

	for _, exp := range expected {
		if !slices.Contains(files, exp) {
			t.Errorf("Expected file %s not found in %v", exp, files)
		}
	}
}

func TestGetDefault(t *testing.T) {
	content, err := GetDefault("config.yaml")
	if err != nil {
		t.Fatalf("GetDefault failed: %v", err)
	}

	if len(content) == 0 {
		t.Error("config.yaml content is empty")
	}

	// Verify it's valid YAML-ish content
	if string(content[:1]) != "#" {
		t.Log("config.yaml doesn't start with comment, that's fine")
	}
}

func TestDataDir(t *testing.T) {
	dir, err := DataDir()
	if err != nil {
		t.Fatalf("DataDir failed: %v", err)
	}

	// Should use os.UserConfigDir() as base
	configDir, err := os.UserConfigDir()
	if err != nil {
		t.Fatalf("UserConfigDir failed: %v", err)
	}

	if !strings.HasPrefix(dir, configDir) {
		t.Errorf("Expected DataDir to be under %s, got %s", configDir, dir)
	}

	base := filepath.Base(dir)
	if base != "Nebo" && base != "nebo" {
		t.Errorf("Expected DataDir to end with Nebo or nebo, got %s", base)
	}
}

func TestEnsureDataDir(t *testing.T) {
	// Use temp directory for testing via NEBO_DATA_DIR override
	tmpDir := t.TempDir()
	dataDir := filepath.Join(tmpDir, "Nebo")
	t.Setenv("NEBO_DATA_DIR", dataDir)

	dir, err := EnsureDataDir()
	if err != nil {
		t.Fatalf("EnsureDataDir failed: %v", err)
	}

	// Check directory was created
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		t.Error("Data directory was not created")
	}

	// Check config files were copied
	configPath := filepath.Join(dir, "config.yaml")
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Error("config.yaml was not copied")
	}

	modelsPath := filepath.Join(dir, "models.yaml")
	if _, err := os.Stat(modelsPath); os.IsNotExist(err) {
		t.Error("models.yaml was not copied")
	}
}
