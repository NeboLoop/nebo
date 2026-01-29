package router

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestBindingStore(t *testing.T) {
	store := NewBindingStore()

	// Test Add
	binding := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		AgentID:     "agent-1",
		Name:        "Test Binding",
		Enabled:     true,
	}

	err := store.Add(binding)
	if err != nil {
		t.Fatalf("Add() error = %v", err)
	}

	// Test Get by ID
	got, ok := store.Get("binding-1")
	if !ok {
		t.Fatal("Get() failed to find binding")
	}
	if got.Name != "Test Binding" {
		t.Errorf("Get() Name = %q, want %q", got.Name, "Test Binding")
	}

	// Test GetByChannel
	got, ok = store.GetByChannel("telegram", "12345")
	if !ok {
		t.Fatal("GetByChannel() failed to find binding")
	}
	if got.ID != "binding-1" {
		t.Errorf("GetByChannel() ID = %q, want %q", got.ID, "binding-1")
	}

	// Test Count
	if store.Count() != 1 {
		t.Errorf("Count() = %d, want 1", store.Count())
	}
}

func TestBindingStoreDuplicateChannel(t *testing.T) {
	store := NewBindingStore()

	binding1 := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Enabled:     true,
	}
	store.Add(binding1)

	// Try to add another binding with same channel
	binding2 := &Binding{
		ID:          "binding-2",
		ChannelType: "telegram",
		ChannelID:   "12345", // Same channel
		Enabled:     true,
	}

	err := store.Add(binding2)
	if err == nil {
		t.Error("Add() should error for duplicate channel")
	}
}

func TestBindingStoreRemove(t *testing.T) {
	store := NewBindingStore()

	binding := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Enabled:     true,
	}
	store.Add(binding)

	err := store.Remove("binding-1")
	if err != nil {
		t.Fatalf("Remove() error = %v", err)
	}

	// Should no longer be found
	_, ok := store.Get("binding-1")
	if ok {
		t.Error("Get() should not find removed binding")
	}

	_, ok = store.GetByChannel("telegram", "12345")
	if ok {
		t.Error("GetByChannel() should not find removed binding")
	}
}

func TestBindingStoreUpdate(t *testing.T) {
	store := NewBindingStore()

	binding := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Name:        "Original",
		Enabled:     true,
	}
	store.Add(binding)

	// Update
	updated := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Name:        "Updated",
		Enabled:     true,
	}
	err := store.Update(updated)
	if err != nil {
		t.Fatalf("Update() error = %v", err)
	}

	got, _ := store.Get("binding-1")
	if got.Name != "Updated" {
		t.Errorf("Get() Name = %q, want %q", got.Name, "Updated")
	}
}

func TestBindingStoreListAll(t *testing.T) {
	store := NewBindingStore()

	store.Add(&Binding{ID: "1", ChannelType: "telegram", ChannelID: "a", Enabled: true})
	store.Add(&Binding{ID: "2", ChannelType: "telegram", ChannelID: "b", Enabled: true})
	store.Add(&Binding{ID: "3", ChannelType: "telegram", ChannelID: "c", Enabled: true})

	allBindings := store.ListAll()
	if len(allBindings) != 3 {
		t.Errorf("ListAll() = %d bindings, want 3", len(allBindings))
	}
}

func TestBindingStoreDisabled(t *testing.T) {
	store := NewBindingStore()

	binding := &Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Enabled:     false, // Disabled
	}
	store.Add(binding)

	// GetByChannel should not return disabled bindings
	_, ok := store.GetByChannel("telegram", "12345")
	if ok {
		t.Error("GetByChannel() should not return disabled binding")
	}
}

func TestBindingStorePersistence(t *testing.T) {
	dir := t.TempDir()
	filePath := filepath.Join(dir, "bindings.json")

	// Create store and add bindings
	store := NewBindingStore()
	store.SetFilePath(filePath)

	store.Add(&Binding{
		ID:          "binding-1",
		ChannelType: "telegram",
		ChannelID:   "12345",
		Name:        "Test",
		Enabled:     true,
	})

	// Wait for async persist
	time.Sleep(100 * time.Millisecond)

	// Check file was created
	if _, err := os.Stat(filePath); os.IsNotExist(err) {
		t.Fatal("Persistence file was not created")
	}

	// Create new store and load
	store2 := NewBindingStore()
	store2.SetFilePath(filePath)
	if err := store2.Load(); err != nil {
		t.Fatalf("Load() error = %v", err)
	}

	if store2.Count() != 1 {
		t.Errorf("Loaded store Count() = %d, want 1", store2.Count())
	}

	got, ok := store2.Get("binding-1")
	if !ok {
		t.Fatal("Get() failed to find loaded binding")
	}
	if got.Name != "Test" {
		t.Errorf("Loaded binding Name = %q, want %q", got.Name, "Test")
	}
}
