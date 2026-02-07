package middleware

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestRateLimiter_Allow(t *testing.T) {
	config := &RateLimitConfig{
		Rate:     5,
		Interval: time.Minute,
		Burst:    5,
		KeyFunc:  DefaultKeyFunc,
	}
	limiter := NewRateLimiter(config)

	key := "test-client"

	// Should allow first 5 requests (burst)
	for i := 0; i < 5; i++ {
		if !limiter.Allow(key) {
			t.Errorf("Request %d should be allowed", i+1)
		}
	}

	// 6th request should be denied
	if limiter.Allow(key) {
		t.Error("Request 6 should be denied")
	}
}

func TestRateLimiter_TokenRefill(t *testing.T) {
	config := &RateLimitConfig{
		Rate:     10,
		Interval: time.Second,
		Burst:    10,
		KeyFunc:  DefaultKeyFunc,
	}
	limiter := NewRateLimiter(config)

	key := "test-client"

	// Use all tokens
	for i := 0; i < 10; i++ {
		limiter.Allow(key)
	}

	// Should be denied
	if limiter.Allow(key) {
		t.Error("Should be denied after burst")
	}

	// Wait for token refill (should refill 1 token per 100ms with rate of 10/second)
	time.Sleep(200 * time.Millisecond)

	// Should now have at least 1 token
	if !limiter.Allow(key) {
		t.Error("Should be allowed after token refill")
	}
}

func TestRateLimiter_Remaining(t *testing.T) {
	config := &RateLimitConfig{
		Rate:     5,
		Interval: time.Minute,
		Burst:    5,
		KeyFunc:  DefaultKeyFunc,
	}
	limiter := NewRateLimiter(config)

	key := "test-client"

	// Initially should have burst tokens
	if limiter.Remaining(key) != 5 {
		t.Errorf("Expected 5 remaining, got %d", limiter.Remaining(key))
	}

	// Use one token
	limiter.Allow(key)

	// Should have 4 remaining
	if limiter.Remaining(key) != 4 {
		t.Errorf("Expected 4 remaining, got %d", limiter.Remaining(key))
	}
}

func TestRateLimiter_Middleware(t *testing.T) {
	config := &RateLimitConfig{
		Rate:            2,
		Interval:        time.Minute,
		Burst:           2,
		KeyFunc:         DefaultKeyFunc,
		ExceededHandler: DefaultExceededHandler,
	}
	limiter := NewRateLimiter(config)

	handler := limiter.Middleware()(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))

	// First two requests should succeed
	for i := 0; i < 2; i++ {
		req := httptest.NewRequest(http.MethodGet, "/", nil)
		req.RemoteAddr = "192.168.1.1:12345"
		rec := httptest.NewRecorder()

		handler.ServeHTTP(rec, req)

		if rec.Code != http.StatusOK {
			t.Errorf("Request %d should succeed, got status %d", i+1, rec.Code)
		}

		// Check rate limit headers
		if rec.Header().Get("X-RateLimit-Limit") == "" {
			t.Error("Expected X-RateLimit-Limit header")
		}
	}

	// Third request should be rate limited
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.RemoteAddr = "192.168.1.1:12345"
	rec := httptest.NewRecorder()

	handler.ServeHTTP(rec, req)

	if rec.Code != http.StatusTooManyRequests {
		t.Errorf("Expected status 429, got %d", rec.Code)
	}

	// Check Retry-After header
	if rec.Header().Get("Retry-After") == "" {
		t.Error("Expected Retry-After header")
	}
}

func TestRateLimiter_DifferentClients(t *testing.T) {
	config := &RateLimitConfig{
		Rate:     2,
		Interval: time.Minute,
		Burst:    2,
		KeyFunc:  DefaultKeyFunc,
	}
	limiter := NewRateLimiter(config)

	// Client 1 uses tokens
	limiter.Allow("client1")
	limiter.Allow("client1")

	// Client 1 should be denied
	if limiter.Allow("client1") {
		t.Error("Client 1 should be denied")
	}

	// Client 2 should still be allowed
	if !limiter.Allow("client2") {
		t.Error("Client 2 should be allowed")
	}
}

func TestDefaultKeyFunc(t *testing.T) {
	// DefaultKeyFunc should ONLY use RemoteAddr, ignoring spoofable headers
	tests := []struct {
		name        string
		remoteAddr  string
		xForwarded  string
		xRealIP     string
		expectedKey string
	}{
		{
			name:        "remote addr only",
			remoteAddr:  "192.168.1.1:12345",
			expectedKey: "192.168.1.1:12345",
		},
		{
			name:        "ignores X-Forwarded-For",
			remoteAddr:  "10.0.0.1:12345",
			xForwarded:  "203.0.113.195, 70.41.3.18",
			expectedKey: "10.0.0.1:12345",
		},
		{
			name:        "ignores X-Real-IP",
			remoteAddr:  "10.0.0.1:12345",
			xRealIP:     "203.0.113.195",
			expectedKey: "10.0.0.1:12345",
		},
		{
			name:        "ignores both proxy headers",
			remoteAddr:  "10.0.0.1:12345",
			xForwarded:  "203.0.113.195",
			xRealIP:     "192.168.1.1",
			expectedKey: "10.0.0.1:12345",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodGet, "/", nil)
			req.RemoteAddr = tt.remoteAddr
			if tt.xForwarded != "" {
				req.Header.Set("X-Forwarded-For", tt.xForwarded)
			}
			if tt.xRealIP != "" {
				req.Header.Set("X-Real-IP", tt.xRealIP)
			}

			key := DefaultKeyFunc(req)
			if key != tt.expectedKey {
				t.Errorf("DefaultKeyFunc() = %q, want %q", key, tt.expectedKey)
			}
		})
	}
}

func TestTrustedProxyKeyFunc(t *testing.T) {
	keyFunc := TrustedProxyKeyFunc([]string{"10.0.0.1"})

	tests := []struct {
		name        string
		remoteAddr  string
		xForwarded  string
		xRealIP     string
		expectedKey string
	}{
		{
			name:        "trusted proxy with X-Forwarded-For",
			remoteAddr:  "10.0.0.1:12345",
			xForwarded:  "203.0.113.195, 70.41.3.18",
			expectedKey: "203.0.113.195",
		},
		{
			name:        "trusted proxy with X-Real-IP",
			remoteAddr:  "10.0.0.1:12345",
			xRealIP:     "203.0.113.195",
			expectedKey: "203.0.113.195",
		},
		{
			name:        "trusted proxy no headers falls back to RemoteAddr",
			remoteAddr:  "10.0.0.1:12345",
			expectedKey: "10.0.0.1:12345",
		},
		{
			name:        "untrusted source ignores X-Forwarded-For",
			remoteAddr:  "192.168.1.1:12345",
			xForwarded:  "203.0.113.195",
			expectedKey: "192.168.1.1:12345",
		},
		{
			name:        "untrusted source ignores X-Real-IP",
			remoteAddr:  "192.168.1.1:12345",
			xRealIP:     "203.0.113.195",
			expectedKey: "192.168.1.1:12345",
		},
		{
			name:        "trusted proxy X-Forwarded-For takes precedence over X-Real-IP",
			remoteAddr:  "10.0.0.1:12345",
			xForwarded:  "198.51.100.1",
			xRealIP:     "203.0.113.195",
			expectedKey: "198.51.100.1",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodGet, "/", nil)
			req.RemoteAddr = tt.remoteAddr
			if tt.xForwarded != "" {
				req.Header.Set("X-Forwarded-For", tt.xForwarded)
			}
			if tt.xRealIP != "" {
				req.Header.Set("X-Real-IP", tt.xRealIP)
			}

			key := keyFunc(req)
			if key != tt.expectedKey {
				t.Errorf("TrustedProxyKeyFunc() = %q, want %q", key, tt.expectedKey)
			}
		})
	}
}

func TestAuthRateLimitConfig(t *testing.T) {
	config := AuthRateLimitConfig()

	// Auth rate limit should be stricter
	if config.Rate != 5 {
		t.Errorf("Auth rate should be 5, got %d", config.Rate)
	}

	if config.Interval != time.Minute {
		t.Errorf("Auth interval should be 1 minute, got %v", config.Interval)
	}
}

func TestRateLimiter_SkipFunc(t *testing.T) {
	config := &RateLimitConfig{
		Rate:     1,
		Interval: time.Minute,
		Burst:    1,
		KeyFunc:  DefaultKeyFunc,
		SkipFunc: func(r *http.Request) bool {
			return r.URL.Path == "/health"
		},
	}
	limiter := NewRateLimiter(config)

	handler := limiter.Middleware()(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))

	// Health endpoint should skip rate limiting
	for i := 0; i < 10; i++ {
		req := httptest.NewRequest(http.MethodGet, "/health", nil)
		req.RemoteAddr = "192.168.1.1:12345"
		rec := httptest.NewRecorder()

		handler.ServeHTTP(rec, req)

		if rec.Code != http.StatusOK {
			t.Errorf("Health request %d should succeed, got %d", i+1, rec.Code)
		}
	}

	// Other endpoints should be rate limited
	req := httptest.NewRequest(http.MethodGet, "/api", nil)
	req.RemoteAddr = "192.168.1.1:12345"
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Error("First API request should succeed")
	}

	// Second request should be rate limited
	req = httptest.NewRequest(http.MethodGet, "/api", nil)
	req.RemoteAddr = "192.168.1.1:12345"
	rec = httptest.NewRecorder()
	handler.ServeHTTP(rec, req)

	if rec.Code != http.StatusTooManyRequests {
		t.Error("Second API request should be rate limited")
	}
}

func TestSlidingWindowLimiter(t *testing.T) {
	limiter := NewSlidingWindowLimiter(time.Second, 5)

	key := "test-client"

	// Should allow first 5 requests
	for i := 0; i < 5; i++ {
		if !limiter.Allow(key) {
			t.Errorf("Request %d should be allowed", i+1)
		}
	}

	// 6th request should be denied
	if limiter.Allow(key) {
		t.Error("Request 6 should be denied")
	}

	// Wait for window to expire
	time.Sleep(1100 * time.Millisecond)

	// Should be allowed again
	if !limiter.Allow(key) {
		t.Error("Should be allowed after window expires")
	}
}
