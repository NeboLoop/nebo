package ai

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"sort"
	"strings"
	"sync"
	"time"
)

// DedupeCache provides time-based deduplication with size limits
type DedupeCache struct {
	mu      sync.Mutex
	cache   map[string]int64 // key -> timestamp
	ttlMs   int64
	maxSize int
}

// DedupeCacheOptions configures a DedupeCache
type DedupeCacheOptions struct {
	TTLMs   int64 // Time-to-live in milliseconds
	MaxSize int   // Maximum number of entries
}

// NewDedupeCache creates a new deduplication cache
func NewDedupeCache(opts DedupeCacheOptions) *DedupeCache {
	ttlMs := opts.TTLMs
	if ttlMs < 0 {
		ttlMs = 0
	}
	maxSize := opts.MaxSize
	if maxSize < 0 {
		maxSize = 0
	}
	return &DedupeCache{
		cache:   make(map[string]int64),
		ttlMs:   ttlMs,
		maxSize: maxSize,
	}
}

// Check returns true if the key is a duplicate (within TTL), false if new
// It also updates the cache entry timestamp (touch)
func (c *DedupeCache) Check(key string) bool {
	if key == "" {
		return false
	}
	return c.CheckAt(key, time.Now().UnixMilli())
}

// CheckAt is like Check but with a specific timestamp (for testing)
func (c *DedupeCache) CheckAt(key string, nowMs int64) bool {
	if key == "" {
		return false
	}

	c.mu.Lock()
	defer c.mu.Unlock()

	existing, found := c.cache[key]
	if found && (c.ttlMs <= 0 || nowMs-existing < c.ttlMs) {
		// Duplicate found, update timestamp (touch)
		c.cache[key] = nowMs
		return true
	}

	// New entry
	c.cache[key] = nowMs
	c.prune(nowMs)
	return false
}

// prune removes expired entries and enforces max size
func (c *DedupeCache) prune(nowMs int64) {
	// Remove expired entries
	if c.ttlMs > 0 {
		for k, v := range c.cache {
			if nowMs-v >= c.ttlMs {
				delete(c.cache, k)
			}
		}
	}

	// Enforce max size (LRU: remove oldest entries)
	if c.maxSize > 0 && len(c.cache) > c.maxSize {
		// Get entries sorted by timestamp
		type entry struct {
			key string
			ts  int64
		}
		entries := make([]entry, 0, len(c.cache))
		for k, v := range c.cache {
			entries = append(entries, entry{k, v})
		}
		sort.Slice(entries, func(i, j int) bool {
			return entries[i].ts < entries[j].ts
		})

		// Remove oldest until under max size
		toRemove := len(c.cache) - c.maxSize
		for i := 0; i < toRemove; i++ {
			delete(c.cache, entries[i].key)
		}
	}
}

// Clear removes all entries
func (c *DedupeCache) Clear() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.cache = make(map[string]int64)
}

// Size returns the current number of entries
func (c *DedupeCache) Size() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.cache)
}

// APIErrorInfo contains parsed information from an API error
type APIErrorInfo struct {
	HTTPCode  string `json:"http_code,omitempty"`
	Type      string `json:"type,omitempty"`
	Message   string `json:"message,omitempty"`
	RequestID string `json:"request_id,omitempty"`
}

// GetAPIErrorPayloadFingerprint creates a deterministic fingerprint of an error payload
func GetAPIErrorPayloadFingerprint(raw string) string {
	if raw == "" {
		return ""
	}

	payload := parseAPIErrorPayload(raw)
	if payload == nil {
		return ""
	}

	return stableStringify(payload)
}

// HashText creates a SHA256 hash of the given text
func HashText(value string) string {
	h := sha256.Sum256([]byte(value))
	return hex.EncodeToString(h[:])
}

// parseAPIErrorPayload attempts to parse JSON error payload from various formats
func parseAPIErrorPayload(raw string) map[string]any {
	// Try direct JSON parse
	var payload map[string]any
	if err := json.Unmarshal([]byte(raw), &payload); err == nil {
		if isErrorPayloadObject(payload) {
			return payload
		}
	}

	// Try to extract JSON from error message
	// Format: "Error: {...}" or "API Error: {...}"
	if idx := strings.Index(raw, "{"); idx >= 0 {
		jsonPart := raw[idx:]
		// Find matching closing brace
		depth := 0
		endIdx := -1
		for i, ch := range jsonPart {
			if ch == '{' {
				depth++
			} else if ch == '}' {
				depth--
				if depth == 0 {
					endIdx = i + 1
					break
				}
			}
		}
		if endIdx > 0 {
			if err := json.Unmarshal([]byte(jsonPart[:endIdx]), &payload); err == nil {
				if isErrorPayloadObject(payload) {
					return payload
				}
			}
		}
	}

	return nil
}

// isErrorPayloadObject checks if a parsed JSON looks like an API error
func isErrorPayloadObject(payload map[string]any) bool {
	if payload == nil {
		return false
	}

	// Check for common error indicators
	if t, ok := payload["type"].(string); ok && t == "error" {
		return true
	}
	if _, ok := payload["request_id"].(string); ok {
		return true
	}
	if _, ok := payload["requestId"].(string); ok {
		return true
	}
	if errObj, ok := payload["error"].(map[string]any); ok {
		if _, hasMsg := errObj["message"]; hasMsg {
			return true
		}
		if _, hasType := errObj["type"]; hasType {
			return true
		}
		if _, hasCode := errObj["code"]; hasCode {
			return true
		}
	}
	return false
}

// stableStringify creates a deterministic JSON string representation
func stableStringify(value any) string {
	if value == nil {
		return "null"
	}

	switch v := value.(type) {
	case map[string]any:
		// Sort keys for stable output
		keys := make([]string, 0, len(v))
		for k := range v {
			keys = append(keys, k)
		}
		sort.Strings(keys)

		parts := make([]string, len(keys))
		for i, k := range keys {
			parts[i] = `"` + k + `":` + stableStringify(v[k])
		}
		return "{" + strings.Join(parts, ",") + "}"

	case []any:
		parts := make([]string, len(v))
		for i, elem := range v {
			parts[i] = stableStringify(elem)
		}
		return "[" + strings.Join(parts, ",") + "]"

	default:
		// Use standard JSON encoding for primitives
		b, err := json.Marshal(v)
		if err != nil {
			return "null"
		}
		return string(b)
	}
}

// ParseAPIErrorInfo extracts structured error info from raw error text
func ParseAPIErrorInfo(raw string) *APIErrorInfo {
	if raw == "" {
		return nil
	}

	info := &APIErrorInfo{}
	lower := strings.ToLower(raw)

	// Extract HTTP status code
	httpPatterns := []struct {
		prefix string
		code   string
	}{
		{"401", "401"},
		{"402", "402"},
		{"403", "403"},
		{"404", "404"},
		{"429", "429"},
		{"500", "500"},
		{"502", "502"},
		{"503", "503"},
	}
	for _, p := range httpPatterns {
		if strings.Contains(raw, p.prefix) {
			info.HTTPCode = p.code
			break
		}
	}

	// Try to parse JSON payload
	payload := parseAPIErrorPayload(raw)
	if payload != nil {
		if reqID, ok := payload["request_id"].(string); ok {
			info.RequestID = reqID
		}
		if reqID, ok := payload["requestId"].(string); ok {
			info.RequestID = reqID
		}
		// Prefer nested error.type over outer type (outer "type": "error" is just a marker)
		if errObj, ok := payload["error"].(map[string]any); ok {
			if msg, ok := errObj["message"].(string); ok {
				info.Message = msg
			}
			if t, ok := errObj["type"].(string); ok {
				info.Type = t
			}
		}
		// Fall back to outer type if no nested error type
		if info.Type == "" {
			if t, ok := payload["type"].(string); ok && t != "error" {
				// "error" is a generic marker, not useful as error type
				info.Type = t
			}
		}
		if msg, ok := payload["message"].(string); ok && info.Message == "" {
			info.Message = msg
		}
	}

	// Extract type from common patterns
	if info.Type == "" {
		typePatterns := []struct {
			pattern string
			errType string
		}{
			{"rate_limit", "rate_limit_error"},
			{"authentication", "authentication_error"},
			{"invalid_api_key", "authentication_error"},
			{"insufficient_quota", "billing_error"},
			{"billing", "billing_error"},
			{"overloaded", "overloaded_error"},
		}
		for _, p := range typePatterns {
			if strings.Contains(lower, p.pattern) {
				info.Type = p.errType
				break
			}
		}
	}

	// If we found anything, return the info
	if info.HTTPCode != "" || info.Type != "" || info.Message != "" || info.RequestID != "" {
		return info
	}

	return nil
}

// Global error deduplication cache for API errors
var apiErrorDedupeCache = NewDedupeCache(DedupeCacheOptions{
	TTLMs:   20 * 60 * 1000, // 20 minutes
	MaxSize: 5000,
})

// IsRecentAPIError checks if this error fingerprint was recently seen
// Returns true if duplicate (should potentially suppress), false if new
func IsRecentAPIError(errFingerprint string) bool {
	return apiErrorDedupeCache.Check(errFingerprint)
}

// ResetAPIErrorDedupe clears the API error deduplication cache
func ResetAPIErrorDedupe() {
	apiErrorDedupeCache.Clear()
}
