package afv

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/neboloop/nebo/internal/agent/tools"
)

func TestRandInt5Range(t *testing.T) {
	for i := 0; i < 100; i++ {
		v := randInt5()
		if v < 10000 || v > 99999 {
			t.Fatalf("randInt5() = %d, want [10000, 99999]", v)
		}
	}
}

func TestFenceGenerateUniqueness(t *testing.T) {
	store := NewFenceStore()
	seen := make(map[int]bool)
	for i := 0; i < 50; i++ {
		fp := store.Generate(fmt.Sprintf("test_%d", i))
		if seen[fp.A] && seen[fp.B] {
			t.Fatalf("duplicate fence pair at iteration %d", i)
		}
		seen[fp.A] = true
		seen[fp.B] = true
	}
}

func TestFenceChecksum(t *testing.T) {
	store := NewFenceStore()
	fp := store.Generate("test")
	if fp.Checksum != fp.A+fp.B {
		t.Fatalf("Checksum = %d, want %d (A=%d + B=%d)", fp.Checksum, fp.A+fp.B, fp.A, fp.B)
	}
}

func TestFenceWrap(t *testing.T) {
	store := NewFenceStore()
	fp := store.Generate("test")
	wrapped := fp.Wrap("hello world")
	expected := fmt.Sprintf("$$FENCE_A_%d$$ hello world $$FENCE_B_%d$$", fp.A, fp.B)
	if wrapped != expected {
		t.Fatalf("Wrap() = %q, want %q", wrapped, expected)
	}
}

func TestFenceCount(t *testing.T) {
	store := NewFenceStore()
	if store.Count() != 0 {
		t.Fatal("empty store should have count 0")
	}
	store.Generate("a")
	store.Generate("b")
	if store.Count() != 2 {
		t.Fatalf("Count() = %d, want 2", store.Count())
	}
}

func TestStripFenceMarkers(t *testing.T) {
	text := "before $$FENCE_A_12345$$ content $$FENCE_B_67890$$ after"
	stripped := StripFenceMarkers(text)
	if strings.Contains(stripped, "$$FENCE") {
		t.Fatalf("StripFenceMarkers() still contains markers: %q", stripped)
	}
	if !strings.Contains(stripped, "content") {
		t.Fatalf("StripFenceMarkers() lost content: %q", stripped)
	}
}

func TestVerifyAllPass(t *testing.T) {
	store := NewFenceStore()
	fp1 := store.Generate("tool_web")
	fp2 := store.Generate("guide_identity")

	context := fmt.Sprintf(
		"system prompt $$FENCE_A_%d$$ web content $$FENCE_B_%d$$ more text $$FENCE_A_%d$$ identity $$FENCE_B_%d$$",
		fp1.A, fp1.B, fp2.A, fp2.B,
	)

	vr := Verify(store, context)
	if !vr.OK {
		t.Fatalf("Verify() failed: %+v", vr.Violations)
	}
	if vr.Total != 2 || vr.Passed != 2 {
		t.Fatalf("Verify() Total=%d Passed=%d, want 2/2", vr.Total, vr.Passed)
	}
}

func TestVerifyMarkerMissing(t *testing.T) {
	store := NewFenceStore()
	fp := store.Generate("tool_web")

	// Only include A marker, not B
	context := fmt.Sprintf("$$FENCE_A_%d$$ some content", fp.A)

	vr := Verify(store, context)
	if vr.OK {
		t.Fatal("Verify() should fail with missing B marker")
	}
	if vr.Failed != 1 {
		t.Fatalf("Failed = %d, want 1", vr.Failed)
	}
	if !strings.Contains(vr.Violations[0].Reason, "closing marker missing") {
		t.Fatalf("unexpected reason: %s", vr.Violations[0].Reason)
	}
}

func TestVerifyBothMarkersMissing(t *testing.T) {
	store := NewFenceStore()
	store.Generate("tool_web")

	// No markers at all
	vr := Verify(store, "just plain text with no markers")
	if vr.OK {
		t.Fatal("Verify() should fail with both markers missing")
	}
	if !strings.Contains(vr.Violations[0].Reason, "both markers missing") {
		t.Fatalf("unexpected reason: %s", vr.Violations[0].Reason)
	}
}

func TestVerifyMarkerAltered(t *testing.T) {
	store := NewFenceStore()
	fp := store.Generate("tool_web")

	// Include A but alter B value
	context := fmt.Sprintf("$$FENCE_A_%d$$ content $$FENCE_B_%d$$", fp.A, fp.B+1)

	vr := Verify(store, context)
	if vr.OK {
		t.Fatal("Verify() should fail with altered B marker")
	}
}

func TestSystemGuideFormat(t *testing.T) {
	store := NewFenceStore()
	guides := BuildSystemGuides(store, "Nebo")

	if len(guides) != len(guideTemplates) {
		t.Fatalf("got %d guides, want %d", len(guides), len(guideTemplates))
	}

	for _, g := range guides {
		formatted := g.Format()
		if !strings.HasPrefix(formatted, `<system-guide name="`) {
			t.Fatalf("guide %q doesn't start with <system-guide>: %s", g.Name, formatted)
		}
		if !strings.HasSuffix(formatted, "</system-guide>") {
			t.Fatalf("guide %q doesn't end with </system-guide>: %s", g.Name, formatted)
		}
		if !strings.Contains(formatted, "$$FENCE_A_") || !strings.Contains(formatted, "$$FENCE_B_") {
			t.Fatalf("guide %q missing internal fences: %s", g.Name, formatted)
		}
	}
}

func TestSystemGuideAgentNameReplacement(t *testing.T) {
	store := NewFenceStore()
	guides := BuildSystemGuides(store, "TestBot")

	found := false
	for _, g := range guides {
		if g.Name == "identity" {
			if !strings.Contains(g.Content, "TestBot") {
				t.Fatalf("identity guide should contain agent name, got: %s", g.Content)
			}
			if strings.Contains(g.Content, "{agent_name}") {
				t.Fatal("identity guide still contains {agent_name} placeholder")
			}
			found = true
		}
	}
	if !found {
		t.Fatal("identity guide not found")
	}
}

func TestToolResultGuide(t *testing.T) {
	store := NewFenceStore()
	guide := BuildToolResultGuide(store, "web")

	if guide.Name != "tool-boundary-web" {
		t.Fatalf("Name = %q, want %q", guide.Name, "tool-boundary-web")
	}
	if !strings.Contains(guide.Content, "UNTRUSTED") {
		t.Fatal("tool result guide should mention UNTRUSTED")
	}

	formatted := guide.Format()
	if !strings.Contains(formatted, "$$FENCE_A_") {
		t.Fatal("tool result guide missing fence markers")
	}
}

func TestQuarantineAddAndRecent(t *testing.T) {
	q := NewQuarantineStore()

	for i := 0; i < 5; i++ {
		q.Add(QuarantinedResponse{
			SessionID: fmt.Sprintf("session-%d", i),
			Content:   fmt.Sprintf("content-%d", i),
			Timestamp: time.Now(),
		})
	}

	if q.Count() != 5 {
		t.Fatalf("Count() = %d, want 5", q.Count())
	}

	recent := q.Recent(3)
	if len(recent) != 3 {
		t.Fatalf("Recent(3) returned %d, want 3", len(recent))
	}
	// Most recent first
	if recent[0].SessionID != "session-4" {
		t.Fatalf("Recent[0] = %q, want session-4", recent[0].SessionID)
	}
}

func TestQuarantineRingBufferEviction(t *testing.T) {
	q := NewQuarantineStore()

	// Fill past capacity
	for i := 0; i < maxQuarantineEntries+10; i++ {
		q.Add(QuarantinedResponse{
			SessionID: fmt.Sprintf("session-%d", i),
			Content:   fmt.Sprintf("content-%d", i),
			Timestamp: time.Now(),
		})
	}

	if q.Count() != maxQuarantineEntries {
		t.Fatalf("Count() = %d, want %d", q.Count(), maxQuarantineEntries)
	}

	recent := q.Recent(1)
	expected := fmt.Sprintf("session-%d", maxQuarantineEntries+9)
	if recent[0].SessionID != expected {
		t.Fatalf("most recent = %q, want %q", recent[0].SessionID, expected)
	}
}

func TestShouldFence(t *testing.T) {
	tests := []struct {
		origin   tools.Origin
		tool     string
		expected bool
	}{
		{tools.OriginComm, "anything", true},
		{tools.OriginApp, "anything", true},
		{tools.OriginSkill, "anything", false},
		{tools.OriginUser, "web", true},
		{tools.OriginUser, "file", false},
		{tools.OriginUser, "shell", false},
		{tools.OriginUser, "skill", false},
		{tools.OriginUser, "agent", false},
		{tools.OriginUser, "screenshot", false},
		{tools.OriginSystem, "anything", false},
		{tools.OriginSystem, "web", false},
	}

	for _, tt := range tests {
		got := ShouldFence(tt.origin, tt.tool)
		if got != tt.expected {
			t.Errorf("ShouldFence(%q, %q) = %v, want %v", tt.origin, tt.tool, got, tt.expected)
		}
	}
}

