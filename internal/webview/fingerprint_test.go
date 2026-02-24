package webview

import (
	"strings"
	"testing"
)

func TestGenerateFingerprintNotNil(t *testing.T) {
	fp := GenerateFingerprint()
	if fp == nil {
		t.Fatal("GenerateFingerprint returned nil")
	}
	if fp.UserAgent == "" {
		t.Error("UserAgent should not be empty")
	}
	if fp.Platform == "" {
		t.Error("Platform should not be empty")
	}
	if fp.ScreenWidth == 0 || fp.ScreenHeight == 0 {
		t.Error("Screen dimensions should not be zero")
	}
	if fp.WebGLVendor == "" || fp.WebGLRenderer == "" {
		t.Error("WebGL vendor/renderer should not be empty")
	}
	if fp.CanvasNoise <= 0 {
		t.Error("CanvasNoise should be positive")
	}
}

func TestGenerateFingerprintUniqueness(t *testing.T) {
	// Generate multiple fingerprints and check they're not all identical
	fps := make([]*Fingerprint, 20)
	for i := range fps {
		fps[i] = GenerateFingerprint()
	}

	// Check that at least some fingerprints differ
	sameUA := 0
	sameTZ := 0
	sameScreen := 0
	for i := 1; i < len(fps); i++ {
		if fps[i].UserAgent == fps[0].UserAgent {
			sameUA++
		}
		if fps[i].Timezone == fps[0].Timezone {
			sameTZ++
		}
		if fps[i].ScreenWidth == fps[0].ScreenWidth {
			sameScreen++
		}
	}

	// With 20 samples and multiple options, it's extremely unlikely all match
	if sameUA == 19 && sameTZ == 19 && sameScreen == 19 {
		t.Error("All 20 fingerprints are identical — randomization likely broken")
	}
}

func TestFingerprintInjectJS(t *testing.T) {
	fp := GenerateFingerprint()
	js := fp.InjectJS()

	// Should be an IIFE
	if !strings.HasPrefix(js, "(function(){") {
		t.Error("InjectJS should be an IIFE")
	}

	// Should override navigator properties
	if !strings.Contains(js, "navigator") || !strings.Contains(js, "userAgent") {
		t.Error("InjectJS should override navigator.userAgent")
	}
	if !strings.Contains(js, fp.UserAgent) {
		t.Error("InjectJS should contain the configured user agent")
	}
	if !strings.Contains(js, fp.Platform) {
		t.Error("InjectJS should contain the configured platform")
	}

	// Should override screen
	if !strings.Contains(js, "screen") {
		t.Error("InjectJS should override screen properties")
	}

	// Should override timezone
	if !strings.Contains(js, "getTimezoneOffset") {
		t.Error("InjectJS should override Date.getTimezoneOffset")
	}
	if !strings.Contains(js, fp.Timezone) {
		t.Error("InjectJS should contain the configured timezone")
	}

	// Should override WebGL
	if !strings.Contains(js, "WEBGL_debug_renderer_info") {
		t.Error("InjectJS should override WebGL renderer info")
	}
	if !strings.Contains(js, fp.WebGLRenderer) {
		t.Error("InjectJS should contain the configured WebGL renderer")
	}

	// Should add canvas noise
	if !strings.Contains(js, "toDataURL") {
		t.Error("InjectJS should override canvas toDataURL for noise")
	}
}

func TestFingerprintInjectedOnWindowCreate(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}

	var capturedHandle *mockHandle
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		h := newMockHandle(opts.Name)
		capturedHandle = h
		return h
	})

	win, err := m.CreateWindow("https://example.com", "Test", "")
	if err != nil {
		t.Fatalf("CreateWindow failed: %v", err)
	}

	if win.Fingerprint == nil {
		t.Fatal("Window should have a fingerprint assigned")
	}

	// Verify ExecJS was called with fingerprint injection
	capturedHandle.mu.Lock()
	defer capturedHandle.mu.Unlock()
	if len(capturedHandle.jsLog) == 0 {
		t.Fatal("ExecJS should have been called to inject fingerprint")
	}

	injectedJS := capturedHandle.jsLog[0]
	if !strings.Contains(injectedJS, "userAgent") {
		t.Error("Injected JS should contain userAgent override")
	}
}
