package tools

import (
	"testing"
	"time"
)

// =============================================================================
// SnapshotStore tests
// =============================================================================

func TestSnapshotStore_PutGet(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	snap := &Snapshot{
		ID:        "snap-001",
		CreatedAt: time.Now(),
		App:       "Safari",
		Elements:  map[string]*Element{"B1": {ID: "B1", Role: "button", Label: "Back"}},
	}

	store.Put(snap)

	got := store.Get("snap-001")
	if got == nil {
		t.Fatal("expected to retrieve snap-001")
	}
	if got.App != "Safari" {
		t.Errorf("got App=%q, want Safari", got.App)
	}
}

func TestSnapshotStore_GetMissing(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	if got := store.Get("nonexistent"); got != nil {
		t.Errorf("expected nil for missing snapshot, got %v", got)
	}
}

func TestSnapshotStore_Latest(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	if store.Latest() != nil {
		t.Error("expected nil when store is empty")
	}

	snap1 := &Snapshot{ID: "snap-001", CreatedAt: time.Now(), App: "Safari"}
	snap2 := &Snapshot{ID: "snap-002", CreatedAt: time.Now(), App: "Chrome"}

	store.Put(snap1)
	store.Put(snap2)

	latest := store.Latest()
	if latest == nil {
		t.Fatal("expected non-nil latest")
	}
	if latest.ID != "snap-002" {
		t.Errorf("got latest ID=%q, want snap-002", latest.ID)
	}
}

func TestSnapshotStore_LookupElement(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	snap := &Snapshot{
		ID:        "snap-001",
		CreatedAt: time.Now(),
		App:       "Safari",
		Elements: map[string]*Element{
			"B1": {ID: "B1", Role: "button", Label: "Back", Bounds: Rect{X: 10, Y: 20, Width: 80, Height: 30}},
			"T1": {ID: "T1", Role: "textfield", Label: "URL", Bounds: Rect{X: 100, Y: 20, Width: 400, Height: 30}},
		},
		ElementOrder: []string{"B1", "T1"},
	}
	store.Put(snap)

	// Lookup by element ID in specific snapshot
	elem, retSnap, err := store.LookupElement("B1", "snap-001")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if elem.Label != "Back" {
		t.Errorf("got label=%q, want Back", elem.Label)
	}
	if retSnap.ID != "snap-001" {
		t.Errorf("got snap ID=%q, want snap-001", retSnap.ID)
	}

	// Lookup in latest snapshot (empty snapshot ID)
	elem, _, err = store.LookupElement("T1", "")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if elem.Label != "URL" {
		t.Errorf("got label=%q, want URL", elem.Label)
	}
}

func TestSnapshotStore_LookupElement_NotFound(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	snap := &Snapshot{
		ID:           "snap-001",
		CreatedAt:    time.Now(),
		Elements:     map[string]*Element{"B1": {ID: "B1"}},
		ElementOrder: []string{"B1"},
	}
	store.Put(snap)

	// Element not in snapshot
	_, _, err := store.LookupElement("T99", "snap-001")
	if err == nil {
		t.Error("expected error for missing element")
	}

	// Snapshot not found
	_, _, err = store.LookupElement("B1", "snap-999")
	if err == nil {
		t.Error("expected error for missing snapshot")
	}

	// No snapshots at all (empty store)
	emptyStore := newSnapshotStore(1 * time.Hour)
	defer emptyStore.Close()
	_, _, err = emptyStore.LookupElement("B1", "")
	if err == nil {
		t.Error("expected error when no snapshots available")
	}
}

func TestSnapshotStore_RemoveExpired(t *testing.T) {
	store := newSnapshotStore(100 * time.Millisecond)
	defer store.Close()

	snap := &Snapshot{
		ID:        "snap-old",
		CreatedAt: time.Now().Add(-1 * time.Second), // already expired (TTL is 100ms)
		App:       "Finder",
		Elements:  map[string]*Element{},
	}
	store.Put(snap)

	// Should still be retrievable before cleanup
	if store.Get("snap-old") == nil {
		t.Fatal("snap should exist before cleanup")
	}

	// Force cleanup
	store.removeExpired()

	if store.Get("snap-old") != nil {
		t.Error("expired snapshot should be removed after cleanup")
	}
}

func TestSnapshotStore_RemoveExpired_KeepsFresh(t *testing.T) {
	store := newSnapshotStore(1 * time.Hour)
	defer store.Close()

	old := &Snapshot{
		ID:        "snap-old",
		CreatedAt: time.Now().Add(-2 * time.Hour),
		Elements:  map[string]*Element{},
	}
	fresh := &Snapshot{
		ID:        "snap-fresh",
		CreatedAt: time.Now(),
		Elements:  map[string]*Element{},
	}

	store.Put(old)
	store.Put(fresh)

	store.removeExpired()

	if store.Get("snap-old") != nil {
		t.Error("old snapshot should be removed")
	}
	if store.Get("snap-fresh") == nil {
		t.Error("fresh snapshot should be kept")
	}
	if store.Latest().ID != "snap-fresh" {
		t.Error("latest should be snap-fresh")
	}
}

// =============================================================================
// Rect tests
// =============================================================================

func TestRectCenter(t *testing.T) {
	tests := []struct {
		name        string
		r           Rect
		wantX, wantY int
	}{
		{"simple", Rect{X: 0, Y: 0, Width: 100, Height: 50}, 50, 25},
		{"offset", Rect{X: 100, Y: 200, Width: 40, Height: 20}, 120, 210},
		{"zero size", Rect{X: 10, Y: 10, Width: 0, Height: 0}, 10, 10},
		{"odd dimensions", Rect{X: 0, Y: 0, Width: 101, Height: 51}, 50, 25}, // integer division
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cx, cy := tt.r.Center()
			if cx != tt.wantX || cy != tt.wantY {
				t.Errorf("Rect%+v.Center() = (%d, %d), want (%d, %d)", tt.r, cx, cy, tt.wantX, tt.wantY)
			}
		})
	}
}

// =============================================================================
// Element ID Assignment (annotator) tests
// =============================================================================

func TestAssignElementIDs_Basic(t *testing.T) {
	tree := []RawElement{
		{Role: "button", Title: "Back", Position: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Forward", Position: Rect{X: 100, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "textfield", Title: "URL Bar", Position: Rect{X: 200, Y: 10, Width: 400, Height: 30}, Actionable: true},
		{Role: "link", Title: "Apple", Position: Rect{X: 50, Y: 200, Width: 60, Height: 20}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 4 {
		t.Fatalf("got %d elements, want 4", len(elements))
	}

	// Check role-prefixed IDs
	wantIDs := map[string]string{
		"B1": "button",
		"B2": "button",
		"T1": "textfield",
		"L1": "link",
	}

	for _, elem := range elements {
		wantRole, ok := wantIDs[elem.ID]
		if !ok {
			t.Errorf("unexpected element ID: %s", elem.ID)
			continue
		}
		if elem.Role != wantRole {
			t.Errorf("element %s: got role=%q, want %q", elem.ID, elem.Role, wantRole)
		}
		delete(wantIDs, elem.ID)
	}
	for id := range wantIDs {
		t.Errorf("missing expected element: %s", id)
	}
}

func TestAssignElementIDs_FiltersNonActionable(t *testing.T) {
	tree := []RawElement{
		{Role: "button", Title: "Click Me", Position: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "static text", Title: "Label", Position: Rect{X: 10, Y: 50, Width: 80, Height: 20}, Actionable: false},
		{Role: "image", Title: "Logo", Position: Rect{X: 10, Y: 80, Width: 80, Height: 80}, Actionable: false},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 1 {
		t.Fatalf("got %d elements, want 1 (only actionable)", len(elements))
	}
	if elements[0].ID != "B1" {
		t.Errorf("got ID=%q, want B1", elements[0].ID)
	}
}

func TestAssignElementIDs_FiltersZeroBounds(t *testing.T) {
	tree := []RawElement{
		{Role: "button", Title: "Visible", Position: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "ZeroWidth", Position: Rect{X: 10, Y: 50, Width: 0, Height: 30}, Actionable: true},
		{Role: "button", Title: "ZeroHeight", Position: Rect{X: 10, Y: 80, Width: 80, Height: 0}, Actionable: true},
		{Role: "button", Title: "Negative", Position: Rect{X: 10, Y: 110, Width: -1, Height: 30}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 1 {
		t.Fatalf("got %d elements, want 1 (only valid bounds)", len(elements))
	}
	if elements[0].Label != "Visible" {
		t.Errorf("got label=%q, want Visible", elements[0].Label)
	}
}

func TestAssignElementIDs_ScreenOrder(t *testing.T) {
	tree := []RawElement{
		{Role: "button", Title: "Bottom-Right", Position: Rect{X: 500, Y: 500, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Top-Left", Position: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Top-Right", Position: Rect{X: 500, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Bottom-Left", Position: Rect{X: 10, Y: 500, Width: 80, Height: 30}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 4 {
		t.Fatalf("got %d elements, want 4", len(elements))
	}

	// Should be ordered: top-left, top-right, bottom-left, bottom-right
	expectedOrder := []string{"Top-Left", "Top-Right", "Bottom-Left", "Bottom-Right"}
	for i, elem := range elements {
		if elem.Label != expectedOrder[i] {
			t.Errorf("position %d: got label=%q, want %q", i, elem.Label, expectedOrder[i])
		}
	}
}

func TestAssignElementIDs_SameRowGrouping(t *testing.T) {
	// Elements within 10px vertical band should be sorted left-to-right
	tree := []RawElement{
		{Role: "button", Title: "Right", Position: Rect{X: 200, Y: 15, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Left", Position: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{Role: "button", Title: "Middle", Position: Rect{X: 100, Y: 12, Width: 80, Height: 30}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 3 {
		t.Fatalf("got %d elements, want 3", len(elements))
	}

	expectedOrder := []string{"Left", "Middle", "Right"}
	for i, elem := range elements {
		if elem.Label != expectedOrder[i] {
			t.Errorf("position %d: got label=%q, want %q", i, elem.Label, expectedOrder[i])
		}
	}
}

func TestAssignElementIDs_FlattensChildren(t *testing.T) {
	tree := []RawElement{
		{
			Role: "group", Title: "Toolbar", Position: Rect{X: 0, Y: 0, Width: 600, Height: 50}, Actionable: false,
			Children: []RawElement{
				{Role: "button", Title: "Save", Position: Rect{X: 10, Y: 10, Width: 60, Height: 30}, Actionable: true},
				{Role: "button", Title: "Open", Position: Rect{X: 80, Y: 10, Width: 60, Height: 30}, Actionable: true},
			},
		},
		{Role: "link", Title: "Footer", Position: Rect{X: 10, Y: 500, Width: 100, Height: 20}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 3 {
		t.Fatalf("got %d elements, want 3 (2 buttons + 1 link)", len(elements))
	}
}

func TestAssignElementIDs_EmptyTree(t *testing.T) {
	elements := AssignElementIDs(nil)
	if len(elements) != 0 {
		t.Errorf("got %d elements, want 0 for nil tree", len(elements))
	}

	elements = AssignElementIDs([]RawElement{})
	if len(elements) != 0 {
		t.Errorf("got %d elements, want 0 for empty tree", len(elements))
	}
}

func TestAssignElementIDs_DescriptionFallback(t *testing.T) {
	tree := []RawElement{
		{Role: "button", Title: "", Description: "Close window", Position: Rect{X: 10, Y: 10, Width: 30, Height: 30}, Actionable: true},
	}

	elements := AssignElementIDs(tree)

	if len(elements) != 1 {
		t.Fatalf("got %d elements, want 1", len(elements))
	}
	if elements[0].Label != "Close window" {
		t.Errorf("got label=%q, want 'Close window' (from description)", elements[0].Label)
	}
}

// =============================================================================
// Role prefix mapping tests
// =============================================================================

func TestRolePrefix(t *testing.T) {
	tests := []struct {
		role string
		want string
	}{
		{"button", "B"},
		{"Button", "B"},
		{"BUTTON", "B"},
		{"textfield", "T"},
		{"text field", "T"},
		{"link", "L"},
		{"checkbox", "C"},
		{"check box", "C"},
		{"menu", "M"},
		{"menu item", "M"},
		{"slider", "S"},
		{"tab", "A"},
		{"radio", "R"},
		{"radio button", "R"},
		{"popup", "P"},
		{"pop up button", "P"},
		{"combobox", "P"},
		{"image", "G"},
		{"static text", "X"},
		{"toolbar", "O"},
		{"list", "I"},
		{"table", "W"},
		{"scroll bar", "Z"},
		{"group", "U"},
		{"window", "N"},
		{"toggle", "C"},
		{"unknownrole", "U"}, // default: first letter uppercased
		{"", "E"},            // empty role: default E
	}

	for _, tt := range tests {
		t.Run(tt.role, func(t *testing.T) {
			got := rolePrefix(tt.role)
			if got != tt.want {
				t.Errorf("rolePrefix(%q) = %q, want %q", tt.role, got, tt.want)
			}
		})
	}
}

// =============================================================================
// FormatElementList tests
// =============================================================================

func TestFormatElementList_Empty(t *testing.T) {
	result := FormatElementList(nil)
	if result != "No actionable elements found." {
		t.Errorf("got %q, want 'No actionable elements found.'", result)
	}
}

func TestFormatElementList_WithElements(t *testing.T) {
	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Back", Bounds: Rect{X: 10, Y: 20, Width: 80, Height: 30}},
		{ID: "T1", Role: "textfield", Label: "URL Bar", Bounds: Rect{X: 100, Y: 20, Width: 400, Height: 30}},
	}

	result := FormatElementList(elements)

	if result == "" {
		t.Fatal("expected non-empty result")
	}
	if !contains(result, "B1") || !contains(result, "T1") {
		t.Errorf("result should contain element IDs: %s", result)
	}
	if !contains(result, "Back") || !contains(result, "URL Bar") {
		t.Errorf("result should contain labels: %s", result)
	}
	if !contains(result, "2 actionable") {
		t.Errorf("result should contain count: %s", result)
	}
}

func TestFormatElementList_TruncatesLongLabel(t *testing.T) {
	elements := []*Element{
		{ID: "B1", Role: "button", Label: "This is a very long label that exceeds the maximum character limit for display", Bounds: Rect{X: 10, Y: 10, Width: 80, Height: 30}},
	}

	result := FormatElementList(elements)

	if !contains(result, "...") {
		t.Errorf("long label should be truncated with ...: %s", result)
	}
}

func TestFormatElementList_EmptyLabel(t *testing.T) {
	elements := []*Element{
		{ID: "B1", Role: "button", Label: "", Bounds: Rect{X: 10, Y: 10, Width: 80, Height: 30}},
	}

	result := FormatElementList(elements)

	if !contains(result, "(no label)") {
		t.Errorf("empty label should show '(no label)': %s", result)
	}
}

// =============================================================================
// abs helper test
// =============================================================================

func TestAbs(t *testing.T) {
	if abs(5) != 5 {
		t.Error("abs(5) should be 5")
	}
	if abs(-5) != 5 {
		t.Error("abs(-5) should be 5")
	}
	if abs(0) != 0 {
		t.Error("abs(0) should be 0")
	}
}

// contains is a test helper for substring check
func contains(s, substr string) bool {
	return len(s) >= len(substr) && searchSubstring(s, substr)
}

func searchSubstring(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
