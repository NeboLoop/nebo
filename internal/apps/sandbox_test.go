package apps

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDefaultSandboxConfig(t *testing.T) {
	cfg := DefaultSandboxConfig()
	if cfg.MaxBinarySizeMB != 500 {
		t.Errorf("MaxBinarySizeMB = %d, want 500", cfg.MaxBinarySizeMB)
	}
	if !cfg.LogToFile {
		t.Error("LogToFile should be true by default")
	}
}

func TestSanitizeEnv(t *testing.T) {
	manifest := &AppManifest{
		ID:      "com.test.app",
		Name:    "Test",
		Version: "1.0.0",
	}
	appDir := "/tmp/apps/test"
	sockPath := "/tmp/apps/test/app.sock"

	env := sanitizeEnv(manifest, appDir, sockPath)

	// Check required NEBO_APP_* vars are present
	required := map[string]string{
		"NEBO_APP_DIR":     appDir,
		"NEBO_APP_SOCK":    sockPath,
		"NEBO_APP_ID":      "com.test.app",
		"NEBO_APP_NAME":    "Test",
		"NEBO_APP_VERSION": "1.0.0",
		"NEBO_APP_DATA":    filepath.Join(appDir, "data"),
	}

	envMap := make(map[string]string)
	for _, e := range env {
		k, v, _ := strings.Cut(e, "=")
		envMap[k] = v
	}

	for k, want := range required {
		got, ok := envMap[k]
		if !ok {
			t.Errorf("missing required env var: %s", k)
			continue
		}
		if got != want {
			t.Errorf("%s = %q, want %q", k, got, want)
		}
	}

	// Check that dangerous vars are NOT present
	dangerous := []string{
		"ANTHROPIC_API_KEY",
		"OPENAI_API_KEY",
		"JWT_SECRET",
		"AWS_SECRET_ACCESS_KEY",
		"DATABASE_URL",
	}
	for _, k := range dangerous {
		if _, ok := envMap[k]; ok {
			t.Errorf("dangerous env var %s should not be passed to app", k)
		}
	}
}

func TestSanitizeEnv_AllowlistedVars(t *testing.T) {
	manifest := &AppManifest{ID: "test", Name: "T", Version: "1"}

	// Set a known allowlisted var
	os.Setenv("TZ", "UTC")
	defer os.Unsetenv("TZ")

	env := sanitizeEnv(manifest, "/tmp/test", "/tmp/test/app.sock")

	found := false
	for _, e := range env {
		if strings.HasPrefix(e, "TZ=") {
			found = true
			break
		}
	}
	if !found {
		t.Error("allowlisted TZ var should be passed through")
	}
}

func TestValidateBinary(t *testing.T) {
	cfg := SandboxConfig{MaxBinarySizeMB: 1}

	t.Run("valid binary", func(t *testing.T) {
		dir := t.TempDir()
		binPath := filepath.Join(dir, "binary")
		// Write a fake Mach-O 64-bit header (magic bytes + padding)
		fakeNative := append([]byte{0xcf, 0xfa, 0xed, 0xfe}, make([]byte, 100)...)
		os.WriteFile(binPath, fakeNative, 0755)

		if err := validateBinary(binPath, cfg); err != nil {
			t.Errorf("unexpected error: %v", err)
		}
	})

	t.Run("non-executable", func(t *testing.T) {
		dir := t.TempDir()
		binPath := filepath.Join(dir, "binary")
		os.WriteFile(binPath, []byte("data"), 0644)

		err := validateBinary(binPath, cfg)
		if err == nil {
			t.Fatal("expected error for non-executable binary")
		}
		if !strings.Contains(err.Error(), "not executable") {
			t.Errorf("error = %q, want containing 'not executable'", err.Error())
		}
	})

	t.Run("symlink rejected", func(t *testing.T) {
		dir := t.TempDir()
		realBin := filepath.Join(dir, "real")
		fakeNative := append([]byte{0xcf, 0xfa, 0xed, 0xfe}, make([]byte, 100)...)
		os.WriteFile(realBin, fakeNative, 0755)

		linkBin := filepath.Join(dir, "link")
		os.Symlink(realBin, linkBin)

		err := validateBinary(linkBin, cfg)
		if err == nil {
			t.Fatal("expected error for symlink binary")
		}
		if !strings.Contains(err.Error(), "symlink") {
			t.Errorf("error = %q, want containing 'symlink'", err.Error())
		}
	})

	t.Run("oversized binary", func(t *testing.T) {
		dir := t.TempDir()
		binPath := filepath.Join(dir, "binary")
		// Create a file larger than 1MB limit
		data := make([]byte, 2*1024*1024)
		os.WriteFile(binPath, data, 0755)

		err := validateBinary(binPath, SandboxConfig{MaxBinarySizeMB: 1})
		if err == nil {
			t.Fatal("expected error for oversized binary")
		}
		if !strings.Contains(err.Error(), "too large") {
			t.Errorf("error = %q, want containing 'too large'", err.Error())
		}
	})

	t.Run("no size limit", func(t *testing.T) {
		dir := t.TempDir()
		binPath := filepath.Join(dir, "binary")
		// Start with Mach-O magic so format validation passes
		data := make([]byte, 2*1024*1024)
		copy(data, []byte{0xcf, 0xfa, 0xed, 0xfe})
		os.WriteFile(binPath, data, 0755)

		// MaxBinarySizeMB = 0 means no limit
		if err := validateBinary(binPath, SandboxConfig{MaxBinarySizeMB: 0}); err != nil {
			t.Errorf("unexpected error with no size limit: %v", err)
		}
	})

	t.Run("missing binary", func(t *testing.T) {
		err := validateBinary("/nonexistent/binary", cfg)
		if err == nil {
			t.Fatal("expected error for missing binary")
		}
	})
}

func TestAppLogWriter_NoLog(t *testing.T) {
	dir := t.TempDir()
	stdout, stderr, cleanup, err := appLogWriter(dir, SandboxConfig{LogToFile: false})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer cleanup()

	if stdout != os.Stdout {
		t.Error("stdout should be os.Stdout when LogToFile is false")
	}
	if stderr != os.Stderr {
		t.Error("stderr should be os.Stderr when LogToFile is false")
	}
}

func TestAppLogWriter_ToFile(t *testing.T) {
	dir := t.TempDir()
	stdout, stderr, cleanup, err := appLogWriter(dir, SandboxConfig{LogToFile: true})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer cleanup()

	if stdout == os.Stdout {
		t.Error("stdout should not be os.Stdout when LogToFile is true")
	}
	if stderr == os.Stderr {
		t.Error("stderr should not be os.Stderr when LogToFile is true")
	}

	// Verify log directory was created
	logDir := filepath.Join(dir, "logs")
	if _, err := os.Stat(logDir); err != nil {
		t.Errorf("log directory should exist: %v", err)
	}

	// Verify log files exist
	if _, err := os.Stat(filepath.Join(logDir, "stdout.log")); err != nil {
		t.Errorf("stdout.log should exist: %v", err)
	}
	if _, err := os.Stat(filepath.Join(logDir, "stderr.log")); err != nil {
		t.Errorf("stderr.log should exist: %v", err)
	}
}
