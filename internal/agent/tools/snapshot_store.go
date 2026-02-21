package tools

import (
	"fmt"
	"sync"
	"time"
)

// Rect represents a screen rectangle with position and size.
type Rect struct {
	X, Y, Width, Height int
}

// Center returns the center point of the rectangle.
func (r Rect) Center() (int, int) {
	return r.X + r.Width/2, r.Y + r.Height/2
}

// Element represents a single UI element from an accessibility snapshot.
type Element struct {
	ID         string `json:"id"`         // Role-prefixed: B1, T2, L3, C1, etc.
	Role       string `json:"role"`       // button, textfield, link, checkbox, etc.
	Label      string `json:"label"`      // Accessibility title/description
	Bounds     Rect   `json:"bounds"`     // Absolute screen coordinates
	Value      string `json:"value"`      // Current value (text fields, etc.)
	Actionable bool   `json:"actionable"` // Whether the element can be interacted with
}

// Snapshot represents a captured UI state with annotated elements.
type Snapshot struct {
	ID           string
	CreatedAt    time.Time
	App          string
	WindowTitle  string
	RawPNG       []byte              // Unannotated screenshot
	AnnotatedPNG []byte              // Screenshot with element overlays
	Elements     map[string]*Element // Keyed by element ID
	ElementOrder []string            // Ordered by screen position
}

// SnapshotStore is an in-memory store for UI snapshots with automatic TTL expiration.
type SnapshotStore struct {
	mu        sync.RWMutex
	snapshots map[string]*Snapshot
	order     []string // Most recent last
	ttl       time.Duration
	stopCh    chan struct{}
}

const defaultSnapshotTTL = 1 * time.Hour

var (
	snapshotStoreOnce sync.Once
	globalSnapshots   *SnapshotStore
)

// GetSnapshotStore returns the package-level singleton snapshot store.
func GetSnapshotStore() *SnapshotStore {
	snapshotStoreOnce.Do(func() {
		globalSnapshots = newSnapshotStore(defaultSnapshotTTL)
	})
	return globalSnapshots
}

func newSnapshotStore(ttl time.Duration) *SnapshotStore {
	s := &SnapshotStore{
		snapshots: make(map[string]*Snapshot),
		ttl:       ttl,
		stopCh:    make(chan struct{}),
	}
	go s.cleanup()
	return s
}

// Put stores a snapshot and returns its ID.
func (s *SnapshotStore) Put(snap *Snapshot) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.snapshots[snap.ID] = snap
	s.order = append(s.order, snap.ID)
}

// Get retrieves a snapshot by ID.
func (s *SnapshotStore) Get(id string) *Snapshot {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.snapshots[id]
}

// Latest returns the most recently stored snapshot, or nil if empty.
func (s *SnapshotStore) Latest() *Snapshot {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if len(s.order) == 0 {
		return nil
	}
	return s.snapshots[s.order[len(s.order)-1]]
}

// LookupElement finds an element by its ID across all snapshots.
// If snapshotID is empty, searches the latest snapshot.
func (s *SnapshotStore) LookupElement(elementID string, snapshotID string) (*Element, *Snapshot, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var snap *Snapshot
	if snapshotID != "" {
		snap = s.snapshots[snapshotID]
		if snap == nil {
			return nil, nil, fmt.Errorf("snapshot %q not found", snapshotID)
		}
	} else {
		if len(s.order) == 0 {
			return nil, nil, fmt.Errorf("no snapshots available — use screenshot(action: \"see\") first")
		}
		snap = s.snapshots[s.order[len(s.order)-1]]
	}

	elem, ok := snap.Elements[elementID]
	if !ok {
		return nil, nil, fmt.Errorf("element %q not found in snapshot %s", elementID, snap.ID)
	}
	return elem, snap, nil
}

// cleanup periodically removes expired snapshots.
func (s *SnapshotStore) cleanup() {
	ticker := time.NewTicker(5 * time.Minute)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			s.removeExpired()
		case <-s.stopCh:
			return
		}
	}
}

func (s *SnapshotStore) removeExpired() {
	s.mu.Lock()
	defer s.mu.Unlock()

	cutoff := time.Now().Add(-s.ttl)
	var kept []string
	for _, id := range s.order {
		snap := s.snapshots[id]
		if snap.CreatedAt.Before(cutoff) {
			delete(s.snapshots, id)
		} else {
			kept = append(kept, id)
		}
	}
	s.order = kept
}

// Close stops the cleanup goroutine.
func (s *SnapshotStore) Close() {
	close(s.stopCh)
}
