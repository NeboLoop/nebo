package apps

import (
	"archive/tar"
	"compress/gzip"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestIsAllowedNappFile(t *testing.T) {
	allowed := []string{
		"manifest.json",
		"binary",
		"app",
		"signatures.json",
		"SKILL.md",
		"ui/index.html",
		"ui/style.css",
		"ui/js/app.js",
	}
	for _, f := range allowed {
		if !isAllowedNappFile(f) {
			t.Errorf("isAllowedNappFile(%q) = false, want true", f)
		}
	}

	disallowed := []string{
		".env",
		"config.yaml",
		"hack.sh",
		"data/db.sqlite",
		"logs/stdout.log",
	}
	for _, f := range disallowed {
		if isAllowedNappFile(f) {
			t.Errorf("isAllowedNappFile(%q) = true, want false", f)
		}
	}
}

func TestMaxSizeForFile(t *testing.T) {
	if maxSizeForFile("binary") != maxNappBinarySize {
		t.Errorf("binary max size = %d, want %d", maxSizeForFile("binary"), maxNappBinarySize)
	}
	if maxSizeForFile("app") != maxNappBinarySize {
		t.Errorf("app max size = %d, want %d", maxSizeForFile("app"), maxNappBinarySize)
	}
	if maxSizeForFile("ui/index.html") != maxNappUIFileSize {
		t.Errorf("ui file max size = %d, want %d", maxSizeForFile("ui/index.html"), maxNappUIFileSize)
	}
	if maxSizeForFile("manifest.json") != maxNappMetaFileSize {
		t.Errorf("manifest max size = %d, want %d", maxSizeForFile("manifest.json"), maxNappMetaFileSize)
	}
	if maxSizeForFile("signatures.json") != maxNappMetaFileSize {
		t.Errorf("signatures max size = %d, want %d", maxSizeForFile("signatures.json"), maxNappMetaFileSize)
	}
}

func TestExtractNapp_Valid(t *testing.T) {
	// Use Mach-O 64-bit magic bytes so binary format validation passes
	fakeBinary := string([]byte{0xcf, 0xfa, 0xed, 0xfe}) + string(make([]byte, 100))
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json":   {content: `{"id":"test","name":"Test","version":"1.0.0","provides":["gateway"]}`},
		"binary":          {content: fakeBinary, mode: 0755},
		"signatures.json": {content: `{"key_id":"abc","algorithm":"ed25519","binary_sha256":"x","binary_signature":"y","manifest_signature":"z"}`},
		"SKILL.md":        {content: "# Test Skill\nA test skill definition."},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err != nil {
		t.Fatalf("ExtractNapp() error = %v", err)
	}

	// Verify files exist
	for _, name := range []string{"manifest.json", "binary", "signatures.json"} {
		if _, err := os.Stat(filepath.Join(destDir, name)); err != nil {
			t.Errorf("expected %s to exist: %v", name, err)
		}
	}

	// Verify binary is executable
	info, _ := os.Stat(filepath.Join(destDir, "binary"))
	if info.Mode().Perm()&0100 == 0 {
		t.Error("binary should be executable")
	}
}

func TestExtractNapp_WithUIFiles(t *testing.T) {
	fakeBinary := string([]byte{0xcf, 0xfa, 0xed, 0xfe}) + string(make([]byte, 100))
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json":   {content: `{"id":"test"}`},
		"binary":          {content: fakeBinary, mode: 0755},
		"signatures.json": {content: `{"key_id":"k","algorithm":"ed25519","binary_sha256":"x","binary_signature":"y","manifest_signature":"z"}`},
		"SKILL.md":        {content: "# Test Skill"},
		"ui/index.html":   {content: "<html></html>"},
		"ui/style.css":    {content: "body{}"},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err != nil {
		t.Fatalf("ExtractNapp() error = %v", err)
	}

	if _, err := os.Stat(filepath.Join(destDir, "ui", "index.html")); err != nil {
		t.Error("ui/index.html should exist")
	}
}

func TestExtractNapp_MissingManifest(t *testing.T) {
	nappPath := createTestNapp(t, map[string]testEntry{
		"binary":          {content: "bin", mode: 0755},
		"signatures.json": {content: `{}`},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil || !strings.Contains(err.Error(), "missing manifest.json") {
		t.Errorf("expected 'missing manifest.json' error, got %v", err)
	}
}

func TestExtractNapp_MissingBinary(t *testing.T) {
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json":   {content: `{}`},
		"signatures.json": {content: `{}`},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil || !strings.Contains(err.Error(), "missing binary") {
		t.Errorf("expected 'missing binary' error, got %v", err)
	}
}

func TestExtractNapp_MissingSignatures(t *testing.T) {
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json": {content: `{}`},
		"binary":        {content: "bin", mode: 0755},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil || !strings.Contains(err.Error(), "missing signatures.json") {
		t.Errorf("expected 'missing signatures.json' error, got %v", err)
	}
}

func TestExtractNapp_PathTraversal(t *testing.T) {
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json":   {content: `{}`},
		"binary":          {content: "bin", mode: 0755},
		"signatures.json": {content: `{}`},
		"../../../etc/passwd": {content: "pwned"},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil {
		t.Fatal("expected error for path traversal")
	}
	errMsg := err.Error()
	if !strings.Contains(errMsg, "path traversal") && !strings.Contains(errMsg, "path escape") {
		t.Errorf("expected path traversal error, got %q", errMsg)
	}
}

func TestExtractNapp_SymlinkRejected(t *testing.T) {
	// Create a napp with a symlink entry
	nappPath := createTestNappWithSymlink(t)

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil {
		t.Fatal("expected error for symlink in napp")
	}
	if !strings.Contains(err.Error(), "symlinks not allowed") {
		t.Errorf("expected symlink error, got %q", err.Error())
	}
}

func TestExtractNapp_UnexpectedFile(t *testing.T) {
	nappPath := createTestNapp(t, map[string]testEntry{
		"manifest.json":   {content: `{}`},
		"binary":          {content: "bin", mode: 0755},
		"signatures.json": {content: `{}`},
		".env":            {content: "SECRET=hack"},
	})

	destDir := filepath.Join(t.TempDir(), "extracted")
	err := ExtractNapp(nappPath, destDir)
	if err == nil {
		t.Fatal("expected error for unexpected file")
	}
	if !strings.Contains(err.Error(), "unexpected file") {
		t.Errorf("expected unexpected file error, got %q", err.Error())
	}
}

// --- Test helpers ---

type testEntry struct {
	content string
	mode    os.FileMode
}

func createTestNapp(t *testing.T, files map[string]testEntry) string {
	t.Helper()
	nappPath := filepath.Join(t.TempDir(), "test.napp")
	f, err := os.Create(nappPath)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()

	gw := gzip.NewWriter(f)
	defer gw.Close()

	tw := tar.NewWriter(gw)
	defer tw.Close()

	for name, entry := range files {
		mode := entry.mode
		if mode == 0 {
			mode = 0644
		}
		hdr := &tar.Header{
			Name:     name,
			Size:     int64(len(entry.content)),
			Mode:     int64(mode),
			Typeflag: tar.TypeReg,
		}
		if err := tw.WriteHeader(hdr); err != nil {
			t.Fatal(err)
		}
		if _, err := tw.Write([]byte(entry.content)); err != nil {
			t.Fatal(err)
		}
	}

	return nappPath
}

func createTestNappWithSymlink(t *testing.T) string {
	t.Helper()
	nappPath := filepath.Join(t.TempDir(), "symlink.napp")
	f, err := os.Create(nappPath)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()

	gw := gzip.NewWriter(f)
	defer gw.Close()

	tw := tar.NewWriter(gw)
	defer tw.Close()

	// Add a symlink entry
	hdr := &tar.Header{
		Name:     "evil_link",
		Linkname: "/etc/passwd",
		Typeflag: tar.TypeSymlink,
	}
	if err := tw.WriteHeader(hdr); err != nil {
		t.Fatal(err)
	}

	return nappPath
}
