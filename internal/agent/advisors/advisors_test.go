package advisors

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/neboloop/nebo/internal/defaults"
)

func TestLoaderLoadAll(t *testing.T) {
	// Use the user's actual advisors directory
	dataDir, err := defaults.DataDir()
	if err != nil {
		t.Fatalf("failed to get data dir: %v", err)
	}

	advisorsDir := filepath.Join(dataDir, "advisors")
	if _, err := os.Stat(advisorsDir); os.IsNotExist(err) {
		t.Skip("advisors directory does not exist")
	}

	loader := NewLoader(advisorsDir)
	if err := loader.LoadAll(); err != nil {
		t.Fatalf("LoadAll failed: %v", err)
	}

	count := loader.Count()
	if count == 0 {
		t.Error("expected at least one advisor to be loaded")
	}
	t.Logf("Loaded %d advisors", count)

	// List all loaded advisors
	advisors := loader.ListAll()
	for _, adv := range advisors {
		t.Logf("  - %s (role: %s, priority: %d, enabled: %v)",
			adv.Name, adv.Role, adv.Priority, adv.Enabled)
		if adv.Persona == "" {
			t.Errorf("advisor %s has empty persona", adv.Name)
		}
	}
}

func TestLoaderGetByName(t *testing.T) {
	dataDir, _ := defaults.DataDir()
	advisorsDir := filepath.Join(dataDir, "advisors")
	if _, err := os.Stat(advisorsDir); os.IsNotExist(err) {
		t.Skip("advisors directory does not exist")
	}

	loader := NewLoader(advisorsDir)
	loader.LoadAll()

	// Test getting skeptic
	skeptic, found := loader.Get("skeptic")
	if !found {
		t.Fatal("expected to find 'skeptic' advisor")
	}

	if skeptic.Role != "critic" {
		t.Errorf("expected skeptic role to be 'critic', got %s", skeptic.Role)
	}

	t.Logf("Skeptic persona preview: %.100s...", skeptic.Persona)
}

func TestLoaderList(t *testing.T) {
	dataDir, _ := defaults.DataDir()
	advisorsDir := filepath.Join(dataDir, "advisors")
	if _, err := os.Stat(advisorsDir); os.IsNotExist(err) {
		t.Skip("advisors directory does not exist")
	}

	loader := NewLoader(advisorsDir)
	loader.LoadAll()

	// List returns only enabled advisors sorted by priority
	enabled := loader.List()
	t.Logf("Found %d enabled advisors (sorted by priority)", len(enabled))

	for _, adv := range enabled {
		t.Logf("  - %s (priority: %d)", adv.Name, adv.Priority)
	}
}
