package ai

import (
	"testing"
)

func TestDedupeCache(t *testing.T) {
	cache := NewDedupeCache(DedupeCacheOptions{
		TTLMs:   1000, // 1 second
		MaxSize: 3,
	})

	// First check should return false (new entry)
	if cache.CheckAt("key1", 1000) {
		t.Error("first check should return false")
	}

	// Second check within TTL should return true (duplicate)
	if !cache.CheckAt("key1", 1500) {
		t.Error("second check within TTL should return true")
	}

	// Check after TTL expires should return false
	if cache.CheckAt("key1", 3000) {
		t.Error("check after TTL should return false")
	}

	// Empty key should always return false
	if cache.Check("") {
		t.Error("empty key should return false")
	}
}

func TestDedupeCacheMaxSize(t *testing.T) {
	cache := NewDedupeCache(DedupeCacheOptions{
		TTLMs:   100000, // Long TTL
		MaxSize: 2,
	})

	cache.CheckAt("key1", 1000)
	cache.CheckAt("key2", 2000)
	cache.CheckAt("key3", 3000) // Should evict key1

	if cache.Size() > 2 {
		t.Errorf("cache size should be <= 2, got %d", cache.Size())
	}

	// key1 should have been evicted (oldest)
	// New check for key1 should return false
	if cache.CheckAt("key1", 4000) {
		t.Error("key1 should have been evicted")
	}
}

func TestDedupeCacheClear(t *testing.T) {
	cache := NewDedupeCache(DedupeCacheOptions{
		TTLMs:   100000,
		MaxSize: 100,
	})

	cache.Check("key1")
	cache.Check("key2")

	if cache.Size() != 2 {
		t.Errorf("expected size 2, got %d", cache.Size())
	}

	cache.Clear()

	if cache.Size() != 0 {
		t.Errorf("expected size 0 after clear, got %d", cache.Size())
	}
}

func TestStableStringify(t *testing.T) {
	tests := []struct {
		name     string
		input    any
		expected string
	}{
		{"nil", nil, "null"},
		{"string", "hello", `"hello"`},
		{"number", 42, "42"},
		{"bool", true, "true"},
		{"empty map", map[string]any{}, "{}"},
		{
			"map with sorted keys",
			map[string]any{"z": 1, "a": 2, "m": 3},
			`{"a":2,"m":3,"z":1}`,
		},
		{
			"nested map",
			map[string]any{"outer": map[string]any{"b": 1, "a": 2}},
			`{"outer":{"a":2,"b":1}}`,
		},
		{"array", []any{1, 2, 3}, "[1,2,3]"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := stableStringify(tt.input)
			if result != tt.expected {
				t.Errorf("stableStringify() = %s, want %s", result, tt.expected)
			}
		})
	}
}

func TestGetAPIErrorPayloadFingerprint(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		wantEmpty bool
	}{
		{"empty string", "", true},
		{"non-JSON", "just text", true},
		{
			"valid error payload",
			`{"type": "error", "error": {"message": "rate limited"}}`,
			false,
		},
		{
			"anthropic style",
			`{"type": "error", "error": {"type": "rate_limit_error", "message": "Too many requests"}}`,
			false,
		},
		{
			"with request_id",
			`{"request_id": "abc123", "error": {"message": "error"}}`,
			false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := GetAPIErrorPayloadFingerprint(tt.input)
			if tt.wantEmpty && result != "" {
				t.Errorf("expected empty fingerprint, got %s", result)
			}
			if !tt.wantEmpty && result == "" {
				t.Error("expected non-empty fingerprint")
			}
		})
	}

	// Test deterministic output - same payload should give same fingerprint
	payload1 := `{"type": "error", "error": {"type": "rate_limit_error", "message": "Too many requests"}}`
	payload2 := `{"error": {"message": "Too many requests", "type": "rate_limit_error"}, "type": "error"}`

	fp1 := GetAPIErrorPayloadFingerprint(payload1)
	fp2 := GetAPIErrorPayloadFingerprint(payload2)

	if fp1 != fp2 {
		t.Errorf("same payload different order should give same fingerprint\nfp1: %s\nfp2: %s", fp1, fp2)
	}
}

func TestParseAPIErrorInfo(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		wantNil  bool
		wantType string
		wantCode string
	}{
		{"empty", "", true, "", ""},
		{"non-error text", "hello world", true, "", ""},
		{
			"rate limit message",
			"Error: rate_limit exceeded",
			false, "rate_limit_error", "",
		},
		{
			"401 status",
			"HTTP 401 Unauthorized",
			false, "", "401",
		},
		{
			"429 status",
			"Status 429 Too Many Requests",
			false, "", "429",
		},
		{
			"JSON with type",
			`{"type": "error", "error": {"type": "authentication_error", "message": "Invalid key"}}`,
			false, "authentication_error", "",
		},
		{
			"billing error",
			"insufficient_quota: Your billing quota has been exceeded",
			false, "billing_error", "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			info := ParseAPIErrorInfo(tt.input)
			if tt.wantNil {
				if info != nil {
					t.Errorf("expected nil, got %+v", info)
				}
				return
			}
			if info == nil {
				t.Fatal("expected non-nil info")
			}
			if tt.wantType != "" && info.Type != tt.wantType {
				t.Errorf("type = %s, want %s", info.Type, tt.wantType)
			}
			if tt.wantCode != "" && info.HTTPCode != tt.wantCode {
				t.Errorf("http_code = %s, want %s", info.HTTPCode, tt.wantCode)
			}
		})
	}
}

func TestHashText(t *testing.T) {
	// Same input should give same hash
	h1 := HashText("hello")
	h2 := HashText("hello")
	if h1 != h2 {
		t.Error("same input should give same hash")
	}

	// Different input should give different hash
	h3 := HashText("world")
	if h1 == h3 {
		t.Error("different input should give different hash")
	}

	// Should be a hex string of appropriate length (SHA256 = 64 hex chars)
	if len(h1) != 64 {
		t.Errorf("hash should be 64 chars, got %d", len(h1))
	}
}

func TestIsRecentAPIError(t *testing.T) {
	// Reset cache before test
	ResetAPIErrorDedupe()

	fingerprint := "test-error-fp-123"

	// First occurrence should return false
	if IsRecentAPIError(fingerprint) {
		t.Error("first occurrence should return false")
	}

	// Second occurrence should return true (duplicate)
	if !IsRecentAPIError(fingerprint) {
		t.Error("second occurrence should return true")
	}

	// Different fingerprint should return false
	if IsRecentAPIError("different-fp") {
		t.Error("different fingerprint should return false")
	}

	// Reset and check again
	ResetAPIErrorDedupe()
	if IsRecentAPIError(fingerprint) {
		t.Error("after reset, should return false")
	}
}
