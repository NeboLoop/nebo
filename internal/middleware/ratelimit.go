package middleware

import (
	"net/http"
	"strconv"
	"sync"
	"time"
)

// RateLimiter implements a token bucket rate limiter
type RateLimiter struct {
	// Rate is the number of requests allowed per interval
	Rate int
	// Interval is the time window for rate limiting
	Interval time.Duration
	// Burst is the maximum number of requests allowed in a burst
	Burst int
	// clients tracks rate limit state per client
	clients sync.Map
	// KeyFunc extracts the client identifier from the request
	KeyFunc func(*http.Request) string
	// ExceededHandler is called when rate limit is exceeded
	ExceededHandler func(http.ResponseWriter, *http.Request)
	// SkipFunc determines if a request should skip rate limiting
	SkipFunc func(*http.Request) bool
}

// clientState tracks the rate limit state for a single client
type clientState struct {
	tokens    float64
	lastCheck time.Time
	mu        sync.Mutex
}

// RateLimitConfig holds configuration for rate limiting
type RateLimitConfig struct {
	// Rate is requests per interval
	Rate int
	// Interval is the time window
	Interval time.Duration
	// Burst is the maximum burst size
	Burst int
	// KeyFunc extracts client identifier
	KeyFunc func(*http.Request) string
	// ExceededHandler handles rate limit exceeded
	ExceededHandler func(http.ResponseWriter, *http.Request)
	// SkipFunc determines requests to skip
	SkipFunc func(*http.Request) bool
}

// DefaultRateLimitConfig returns a default rate limit configuration
func DefaultRateLimitConfig() *RateLimitConfig {
	return &RateLimitConfig{
		Rate:            100,
		Interval:        time.Minute,
		Burst:           20,
		KeyFunc:         DefaultKeyFunc,
		ExceededHandler: DefaultExceededHandler,
		SkipFunc:        nil,
	}
}

// AuthRateLimitConfig returns rate limit configuration for auth endpoints
func AuthRateLimitConfig() *RateLimitConfig {
	return &RateLimitConfig{
		Rate:            5,
		Interval:        time.Minute,
		Burst:           5,
		KeyFunc:         DefaultKeyFunc,
		ExceededHandler: DefaultExceededHandler,
		SkipFunc:        nil,
	}
}

// APIRateLimitConfig returns rate limit configuration for API endpoints
func APIRateLimitConfig() *RateLimitConfig {
	return &RateLimitConfig{
		Rate:            1000,
		Interval:        time.Minute,
		Burst:           100,
		KeyFunc:         DefaultKeyFunc,
		ExceededHandler: DefaultExceededHandler,
		SkipFunc:        nil,
	}
}

// NewRateLimiter creates a new rate limiter
func NewRateLimiter(config *RateLimitConfig) *RateLimiter {
	if config == nil {
		config = DefaultRateLimitConfig()
	}

	exceededHandler := config.ExceededHandler
	if exceededHandler == nil {
		exceededHandler = DefaultExceededHandler
	}

	rl := &RateLimiter{
		Rate:            config.Rate,
		Interval:        config.Interval,
		Burst:           config.Burst,
		KeyFunc:         config.KeyFunc,
		ExceededHandler: exceededHandler,
		SkipFunc:        config.SkipFunc,
	}

	// Start cleanup goroutine
	go rl.cleanup()

	return rl
}

// Allow checks if a request is allowed
func (rl *RateLimiter) Allow(key string) bool {
	now := time.Now()

	// Get or create client state
	stateI, _ := rl.clients.LoadOrStore(key, &clientState{
		tokens:    float64(rl.Burst),
		lastCheck: now,
	})
	state := stateI.(*clientState)

	state.mu.Lock()
	defer state.mu.Unlock()

	// Calculate tokens to add based on time elapsed
	elapsed := now.Sub(state.lastCheck)
	tokensToAdd := elapsed.Seconds() * (float64(rl.Rate) / rl.Interval.Seconds())
	state.tokens = min(float64(rl.Burst), state.tokens+tokensToAdd)
	state.lastCheck = now

	// Check if request is allowed
	if state.tokens >= 1 {
		state.tokens--
		return true
	}

	return false
}

// Remaining returns the number of remaining requests for a key
func (rl *RateLimiter) Remaining(key string) int {
	stateI, ok := rl.clients.Load(key)
	if !ok {
		return rl.Burst
	}
	state := stateI.(*clientState)
	state.mu.Lock()
	defer state.mu.Unlock()
	return int(state.tokens)
}

// Reset returns when the rate limit will reset for a key
func (rl *RateLimiter) Reset(key string) time.Time {
	stateI, ok := rl.clients.Load(key)
	if !ok {
		return time.Now()
	}
	state := stateI.(*clientState)
	state.mu.Lock()
	defer state.mu.Unlock()

	// Calculate time until fully replenished
	tokensNeeded := float64(rl.Burst) - state.tokens
	if tokensNeeded <= 0 {
		return time.Now()
	}

	refillRate := float64(rl.Rate) / rl.Interval.Seconds()
	timeNeeded := time.Duration(tokensNeeded/refillRate) * time.Second

	return time.Now().Add(timeNeeded)
}

// Middleware returns an HTTP middleware that enforces rate limiting
func (rl *RateLimiter) Middleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Check if should skip
			if rl.SkipFunc != nil && rl.SkipFunc(r) {
				next.ServeHTTP(w, r)
				return
			}

			// Get client key
			key := rl.KeyFunc(r)

			// Check rate limit
			if !rl.Allow(key) {
				// Set rate limit headers
				w.Header().Set("X-RateLimit-Limit", strconv.Itoa(rl.Rate))
				w.Header().Set("X-RateLimit-Remaining", strconv.Itoa(rl.Remaining(key)))
				w.Header().Set("X-RateLimit-Reset", strconv.FormatInt(rl.Reset(key).Unix(), 10))
				w.Header().Set("Retry-After", strconv.Itoa(int(rl.Interval.Seconds())))

				rl.ExceededHandler(w, r)
				return
			}

			// Set rate limit headers
			w.Header().Set("X-RateLimit-Limit", strconv.Itoa(rl.Rate))
			w.Header().Set("X-RateLimit-Remaining", strconv.Itoa(rl.Remaining(key)))
			w.Header().Set("X-RateLimit-Reset", strconv.FormatInt(rl.Reset(key).Unix(), 10))

			next.ServeHTTP(w, r)
		})
	}
}

// cleanup periodically removes expired client states
func (rl *RateLimiter) cleanup() {
	ticker := time.NewTicker(5 * time.Minute)
	defer ticker.Stop()

	for range ticker.C {
		threshold := time.Now().Add(-rl.Interval * 2)
		rl.clients.Range(func(key, value interface{}) bool {
			state := value.(*clientState)
			state.mu.Lock()
			if state.lastCheck.Before(threshold) {
				rl.clients.Delete(key)
			}
			state.mu.Unlock()
			return true
		})
	}
}

// DefaultKeyFunc extracts the client IP from RemoteAddr only.
// It intentionally ignores X-Forwarded-For and X-Real-IP headers because
// those are trivially spoofable by any client. For deployments behind a
// trusted reverse proxy, use TrustedProxyKeyFunc instead.
func DefaultKeyFunc(r *http.Request) string {
	return r.RemoteAddr
}

// TrustedProxyKeyFunc returns a key function that only trusts proxy headers
// (X-Forwarded-For, X-Real-IP) when the request comes from a known proxy IP.
// For all other requests, it falls back to RemoteAddr.
func TrustedProxyKeyFunc(trustedProxies []string) func(*http.Request) string {
	trusted := make(map[string]bool, len(trustedProxies))
	for _, ip := range trustedProxies {
		trusted[ip] = true
	}
	return func(r *http.Request) string {
		remoteIP := r.RemoteAddr
		if host, _, err := splitHostPort(remoteIP); err == nil {
			remoteIP = host
		}
		if trusted[remoteIP] {
			xff := r.Header.Get("X-Forwarded-For")
			if xff != "" {
				ips := splitAndTrim(xff, ",")
				if len(ips) > 0 {
					return ips[0]
				}
			}
			xri := r.Header.Get("X-Real-IP")
			if xri != "" {
				return xri
			}
		}
		return r.RemoteAddr
	}
}

// splitHostPort splits a host:port string, handling IPv6 bracket notation.
func splitHostPort(addr string) (string, string, error) {
	// Handle IPv6 [::1]:port
	if len(addr) > 0 && addr[0] == '[' {
		end := len(addr) - 1
		for i := 1; i < len(addr); i++ {
			if addr[i] == ']' {
				end = i
				break
			}
		}
		host := addr[1:end]
		port := ""
		if end+1 < len(addr) && addr[end+1] == ':' {
			port = addr[end+2:]
		}
		return host, port, nil
	}
	// Handle IPv4 host:port
	for i := len(addr) - 1; i >= 0; i-- {
		if addr[i] == ':' {
			return addr[:i], addr[i+1:], nil
		}
	}
	return addr, "", nil
}

// DefaultExceededHandler handles rate limit exceeded responses
func DefaultExceededHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusTooManyRequests)
	w.Write([]byte(`{"error":"rate limit exceeded","message":"too many requests, please try again later"}`))
}

// UserKeyFunc creates a key function that uses user ID from context
func UserKeyFunc(userIDKey interface{}) func(*http.Request) string {
	return func(r *http.Request) string {
		if userID := r.Context().Value(userIDKey); userID != nil {
			if id, ok := userID.(string); ok {
				return "user:" + id
			}
		}
		return DefaultKeyFunc(r)
	}
}

// PathKeyFunc creates a key function that includes the path
func PathKeyFunc(r *http.Request) string {
	return DefaultKeyFunc(r) + ":" + r.URL.Path
}

// splitAndTrim splits a string and trims whitespace from each part
func splitAndTrim(s, sep string) []string {
	parts := make([]string, 0)
	for _, p := range split(s, sep) {
		trimmed := trim(p)
		if trimmed != "" {
			parts = append(parts, trimmed)
		}
	}
	return parts
}

func split(s, sep string) []string {
	result := make([]string, 0)
	start := 0
	for i := 0; i <= len(s)-len(sep); i++ {
		if s[i:i+len(sep)] == sep {
			result = append(result, s[start:i])
			start = i + len(sep)
		}
	}
	result = append(result, s[start:])
	return result
}

func trim(s string) string {
	start := 0
	end := len(s)
	for start < end && (s[start] == ' ' || s[start] == '\t') {
		start++
	}
	for end > start && (s[end-1] == ' ' || s[end-1] == '\t') {
		end--
	}
	return s[start:end]
}

// min returns the smaller of two float64 values
func min(a, b float64) float64 {
	if a < b {
		return a
	}
	return b
}

// SlidingWindowLimiter implements a sliding window rate limiter
type SlidingWindowLimiter struct {
	// Window is the time window size
	Window time.Duration
	// Limit is the maximum requests per window
	Limit int
	// clients tracks request timestamps per client
	clients sync.Map
	// KeyFunc extracts client identifier
	KeyFunc func(*http.Request) string
	// ExceededHandler handles rate limit exceeded
	ExceededHandler func(http.ResponseWriter, *http.Request)
}

type windowState struct {
	requests []time.Time
	mu       sync.Mutex
}

// NewSlidingWindowLimiter creates a new sliding window rate limiter
func NewSlidingWindowLimiter(window time.Duration, limit int) *SlidingWindowLimiter {
	swl := &SlidingWindowLimiter{
		Window:          window,
		Limit:           limit,
		KeyFunc:         DefaultKeyFunc,
		ExceededHandler: DefaultExceededHandler,
	}
	go swl.cleanup()
	return swl
}

// Allow checks if a request is allowed
func (swl *SlidingWindowLimiter) Allow(key string) bool {
	now := time.Now()
	windowStart := now.Add(-swl.Window)

	stateI, _ := swl.clients.LoadOrStore(key, &windowState{
		requests: make([]time.Time, 0),
	})
	state := stateI.(*windowState)

	state.mu.Lock()
	defer state.mu.Unlock()

	// Remove expired requests
	validRequests := make([]time.Time, 0)
	for _, t := range state.requests {
		if t.After(windowStart) {
			validRequests = append(validRequests, t)
		}
	}
	state.requests = validRequests

	// Check limit
	if len(state.requests) >= swl.Limit {
		return false
	}

	// Add current request
	state.requests = append(state.requests, now)
	return true
}

// Middleware returns HTTP middleware
func (swl *SlidingWindowLimiter) Middleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			key := swl.KeyFunc(r)
			if !swl.Allow(key) {
				w.Header().Set("Retry-After", strconv.Itoa(int(swl.Window.Seconds())))
				swl.ExceededHandler(w, r)
				return
			}
			next.ServeHTTP(w, r)
		})
	}
}

func (swl *SlidingWindowLimiter) cleanup() {
	ticker := time.NewTicker(swl.Window)
	defer ticker.Stop()

	for range ticker.C {
		threshold := time.Now().Add(-swl.Window * 2)
		swl.clients.Range(func(key, value interface{}) bool {
			state := value.(*windowState)
			state.mu.Lock()
			hasValid := false
			for _, t := range state.requests {
				if t.After(threshold) {
					hasValid = true
					break
				}
			}
			if !hasValid {
				swl.clients.Delete(key)
			}
			state.mu.Unlock()
			return true
		})
	}
}
