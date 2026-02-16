package afv

import (
	"crypto/rand"
	"fmt"
	"math/big"
	"regexp"
	"sync"

	"github.com/neboloop/nebo/internal/agent/tools"
)

// FencePair holds a pair of random values and their arithmetic checksum.
// A and B appear in the context window; Checksum (A+B) stays in volatile memory only.
type FencePair struct {
	ID       string
	A        int
	B        int
	Checksum int // A + B, never sent to the LLM
}

// Wrap returns the content wrapped in fence markers.
func (f *FencePair) Wrap(content string) string {
	return fmt.Sprintf("$$FENCE_A_%d$$ %s $$FENCE_B_%d$$", f.A, content, f.B)
}

// FenceStore is a per-run volatile store of fence pairs.
// Created fresh each runLoop() invocation and never persisted.
type FenceStore struct {
	mu     sync.RWMutex
	fences map[string]*FencePair
}

// NewFenceStore creates a new volatile fence store.
func NewFenceStore() *FenceStore {
	return &FenceStore{
		fences: make(map[string]*FencePair),
	}
}

// Generate creates a new FencePair with cryptographically random 5-digit values.
func (s *FenceStore) Generate(label string) *FencePair {
	a := randInt5()
	b := randInt5()
	pair := &FencePair{
		ID:       label,
		A:        a,
		B:        b,
		Checksum: a + b,
	}
	s.mu.Lock()
	s.fences[label] = pair
	s.mu.Unlock()
	return pair
}

// Get retrieves a fence pair by label.
func (s *FenceStore) Get(label string) *FencePair {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.fences[label]
}

// Count returns the number of fence pairs in the store.
func (s *FenceStore) Count() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.fences)
}

// All returns a snapshot of all fence pairs.
func (s *FenceStore) All() []*FencePair {
	s.mu.RLock()
	defer s.mu.RUnlock()
	result := make([]*FencePair, 0, len(s.fences))
	for _, fp := range s.fences {
		result = append(result, fp)
	}
	return result
}

var fenceMarkerRe = regexp.MustCompile(`\$\$FENCE_[AB]_\d+\$\$`)

// StripFenceMarkers removes all $$FENCE_*$$ markers from text.
// Use this when cleaning content before persistence to avoid leaking fence values.
func StripFenceMarkers(text string) string {
	return fenceMarkerRe.ReplaceAllString(text, "")
}

// userFencedTools are tool names that get fenced even for OriginUser.
var userFencedTools = map[string]bool{
	"web":   true,
	"file":  true,
	"shell": true,
	"skill": true,
}

// ShouldFence determines whether a tool result should be wrapped in fences
// based on the origin and tool name.
func ShouldFence(origin tools.Origin, toolName string) bool {
	switch origin {
	case tools.OriginComm, tools.OriginApp, tools.OriginSkill:
		return true
	case tools.OriginUser:
		return userFencedTools[toolName]
	case tools.OriginSystem:
		return false
	default:
		return true // Unknown origins get fenced
	}
}

// randInt5 generates a cryptographically random 5-digit integer [10000, 99999].
func randInt5() int {
	n, err := rand.Int(rand.Reader, big.NewInt(90000))
	if err != nil {
		// Fallback should never happen with crypto/rand, but don't panic
		return 10000
	}
	return int(n.Int64()) + 10000
}
