package router

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// Binding represents a channel-to-agent binding
type Binding struct {
	// Identity
	ID string `json:"id"`

	// Channel info
	ChannelType string `json:"channel_type"` // telegram, discord, slack
	ChannelID   string `json:"channel_id"`   // Chat/channel/workspace ID

	// Agent info
	AgentID string `json:"agent_id,omitempty"` // Empty = any agent

	// Metadata
	Name      string    `json:"name,omitempty"` // Human-readable name
	CreatedAt time.Time `json:"created_at"`
	UpdatedAt time.Time `json:"updated_at"`

	// Settings
	Enabled  bool              `json:"enabled"`
	Settings map[string]string `json:"settings,omitempty"`
}

// BindingStore manages channel-to-agent bindings
type BindingStore struct {
	mu       sync.RWMutex
	bindings map[string]*Binding // id -> binding
	byChannel map[string]string   // "type:channelID" -> binding ID
	filePath string              // Optional persistence path
}

// NewBindingStore creates a new binding store
func NewBindingStore() *BindingStore {
	return &BindingStore{
		bindings:  make(map[string]*Binding),
		byChannel: make(map[string]string),
	}
}

// channelKey creates a lookup key for channel type + ID
func channelKey(channelType, channelID string) string {
	return fmt.Sprintf("%s:%s", channelType, channelID)
}

// Add creates a new binding
func (s *BindingStore) Add(b *Binding) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Check for existing binding
	key := channelKey(b.ChannelType, b.ChannelID)
	if existingID, exists := s.byChannel[key]; exists {
		return fmt.Errorf("channel already bound: %s (binding: %s)", key, existingID)
	}

	// Set timestamps
	now := time.Now()
	if b.CreatedAt.IsZero() {
		b.CreatedAt = now
	}
	b.UpdatedAt = now

	// Note: Enabled defaults to false (Go zero value), caller must set to true explicitly

	// Store
	s.bindings[b.ID] = b
	s.byChannel[key] = b.ID

	// Persist if path set
	if s.filePath != "" {
		go s.persist()
	}

	return nil
}

// Remove deletes a binding by ID
func (s *BindingStore) Remove(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	binding, ok := s.bindings[id]
	if !ok {
		return fmt.Errorf("binding not found: %s", id)
	}

	key := channelKey(binding.ChannelType, binding.ChannelID)
	delete(s.byChannel, key)
	delete(s.bindings, id)

	// Persist if path set
	if s.filePath != "" {
		go s.persist()
	}

	return nil
}

// Update modifies an existing binding
func (s *BindingStore) Update(b *Binding) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	existing, ok := s.bindings[b.ID]
	if !ok {
		return fmt.Errorf("binding not found: %s", b.ID)
	}

	// If channel changed, update lookup
	oldKey := channelKey(existing.ChannelType, existing.ChannelID)
	newKey := channelKey(b.ChannelType, b.ChannelID)
	if oldKey != newKey {
		delete(s.byChannel, oldKey)
		s.byChannel[newKey] = b.ID
	}

	b.UpdatedAt = time.Now()
	b.CreatedAt = existing.CreatedAt // Preserve original
	s.bindings[b.ID] = b

	// Persist if path set
	if s.filePath != "" {
		go s.persist()
	}

	return nil
}

// Get returns a binding by ID
func (s *BindingStore) Get(id string) (*Binding, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	b, ok := s.bindings[id]
	return b, ok
}

// GetByChannel returns a binding by channel type and ID
func (s *BindingStore) GetByChannel(channelType, channelID string) (*Binding, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	key := channelKey(channelType, channelID)
	id, ok := s.byChannel[key]
	if !ok {
		return nil, false
	}

	b, ok := s.bindings[id]
	if !ok || !b.Enabled {
		return nil, false
	}
	return b, true
}

// ListAll returns all bindings
func (s *BindingStore) ListAll() []*Binding {
	s.mu.RLock()
	defer s.mu.RUnlock()

	result := make([]*Binding, 0, len(s.bindings))
	for _, b := range s.bindings {
		result = append(result, b)
	}
	return result
}

// Count returns the number of bindings
func (s *BindingStore) Count() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.bindings)
}

// SetFilePath sets the persistence file path
func (s *BindingStore) SetFilePath(path string) {
	s.filePath = path
}

// Load loads bindings from the persistence file
func (s *BindingStore) Load() error {
	if s.filePath == "" {
		return nil
	}

	data, err := os.ReadFile(s.filePath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil // No file yet, that's okay
		}
		return fmt.Errorf("failed to read bindings file: %w", err)
	}

	var bindings []*Binding
	if err := json.Unmarshal(data, &bindings); err != nil {
		return fmt.Errorf("failed to parse bindings: %w", err)
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	s.bindings = make(map[string]*Binding)
	s.byChannel = make(map[string]string)

	for _, b := range bindings {
		s.bindings[b.ID] = b
		key := channelKey(b.ChannelType, b.ChannelID)
		s.byChannel[key] = b.ID
	}

	return nil
}

// persist saves bindings to the persistence file
func (s *BindingStore) persist() error {
	if s.filePath == "" {
		return nil
	}

	s.mu.RLock()
	bindings := make([]*Binding, 0, len(s.bindings))
	for _, b := range s.bindings {
		bindings = append(bindings, b)
	}
	s.mu.RUnlock()

	data, err := json.MarshalIndent(bindings, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal bindings: %w", err)
	}

	// Ensure directory exists
	dir := filepath.Dir(s.filePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create bindings directory: %w", err)
	}

	// Write atomically
	tmpPath := s.filePath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write bindings: %w", err)
	}

	if err := os.Rename(tmpPath, s.filePath); err != nil {
		return fmt.Errorf("failed to rename bindings file: %w", err)
	}

	return nil
}
