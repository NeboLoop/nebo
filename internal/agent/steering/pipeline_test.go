package steering

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/neboloop/nebo/internal/agent/session"
)

// --- Helper to build test messages ---

func makeMessages(roles ...string) []session.Message {
	msgs := make([]session.Message, len(roles))
	for i, role := range roles {
		msgs[i] = session.Message{
			Role:    role,
			Content: "test message " + role,
		}
	}
	return msgs
}

func makeAssistantTurns(n int) []session.Message {
	msgs := make([]session.Message, 0, n*2)
	for i := 0; i < n; i++ {
		msgs = append(msgs, session.Message{Role: "user", Content: "hello"})
		msgs = append(msgs, session.Message{Role: "assistant", Content: "hi there"})
	}
	return msgs
}

func makeAssistantWithToolCall(toolName string) session.Message {
	calls, _ := json.Marshal([]struct {
		Name string `json:"name"`
	}{{Name: toolName}})
	return session.Message{
		Role:      "assistant",
		Content:   "using tool",
		ToolCalls: calls,
	}
}

// --- Pipeline Tests ---

func TestPipelineNew(t *testing.T) {
	p := New()
	if p == nil {
		t.Fatal("New() returned nil")
	}
	if len(p.generators) != 10 {
		t.Errorf("expected 10 generators, got %d", len(p.generators))
	}
}

func TestPipelineGenerate_ReplacesAgentName(t *testing.T) {
	p := &Pipeline{
		generators: []Generator{&identityGuard{}},
	}
	// Need 8 assistant turns to trigger identity guard
	ctx := &Context{
		Messages:  makeAssistantTurns(8),
		AgentName: "TestBot",
	}
	msgs := p.Generate(ctx)
	if len(msgs) == 0 {
		t.Fatal("expected steering messages")
	}
	if !strings.Contains(msgs[0].Content, "TestBot") {
		t.Error("expected agent name replacement")
	}
	if strings.Contains(msgs[0].Content, "{agent_name}") {
		t.Error("placeholder was not replaced")
	}
}

func TestPipelineGenerate_PanicRecovery(t *testing.T) {
	p := &Pipeline{
		generators: []Generator{&panicGenerator{}},
	}
	ctx := &Context{Messages: makeAssistantTurns(1)}
	// Should not panic
	msgs := p.Generate(ctx)
	if len(msgs) != 0 {
		t.Error("expected no messages from panicking generator")
	}
}

type panicGenerator struct{}

func (g *panicGenerator) Name() string                { return "panic" }
func (g *panicGenerator) Generate(_ *Context) []Message { panic("test panic") }

// --- Inject Tests ---

func TestInject_PositionEnd(t *testing.T) {
	msgs := makeMessages("user", "assistant")
	steering := []Message{{Content: "steer", Position: PositionEnd}}
	result := Inject(msgs, steering)
	if len(result) != 3 {
		t.Fatalf("expected 3 messages, got %d", len(result))
	}
	if result[2].Content != "steer" {
		t.Error("steering message should be at end")
	}
	if result[2].Role != "user" {
		t.Error("steering message should have user role")
	}
}

func TestInject_PositionAfterUser(t *testing.T) {
	msgs := makeMessages("user", "assistant", "user", "assistant")
	steering := []Message{{Content: "steer", Position: PositionAfterUser}}
	result := Inject(msgs, steering)
	if len(result) != 5 {
		t.Fatalf("expected 5 messages, got %d", len(result))
	}
	// Should be after the last user message (index 2), so at index 3
	if result[3].Content != "steer" {
		t.Errorf("steering message at wrong position, got content: %s", result[3].Content)
	}
}

func TestInject_Empty(t *testing.T) {
	msgs := makeMessages("user", "assistant")
	result := Inject(msgs, nil)
	if len(result) != 2 {
		t.Error("empty steering should return original messages")
	}
}

// --- Helper Tests ---

func TestCountAssistantTurns(t *testing.T) {
	msgs := makeAssistantTurns(5)
	if got := countAssistantTurns(msgs); got != 5 {
		t.Errorf("expected 5 turns, got %d", got)
	}
}

func TestCountTurnsSinceAnyToolUse(t *testing.T) {
	msgs := []session.Message{
		{Role: "user", Content: "do something"},
		makeAssistantWithToolCall("shell"),
		{Role: "tool", Content: "result"},
		{Role: "assistant", Content: "done with tool"},
		{Role: "user", Content: "now chat"},
		{Role: "assistant", Content: "chatting 1"},
		{Role: "user", Content: "more chat"},
		{Role: "assistant", Content: "chatting 2"},
	}
	if got := countTurnsSinceAnyToolUse(msgs); got != 3 {
		t.Errorf("expected 3 turns since tool use, got %d", got)
	}
}

func TestCountTurnsSinceToolUse_NeverUsed(t *testing.T) {
	msgs := makeAssistantTurns(5)
	if got := countTurnsSinceAnyToolUse(msgs); got != -1 {
		t.Errorf("expected -1 for never used, got %d", got)
	}
}

func TestLastNUserMessagesContain(t *testing.T) {
	msgs := []session.Message{
		{Role: "user", Content: "I work at Acme Corp"},
		{Role: "assistant", Content: "nice"},
		{Role: "user", Content: "thanks"},
	}
	if !lastNUserMessagesContain(msgs, 5, []string{"i work"}) {
		t.Error("should find 'i work' pattern")
	}
	if lastNUserMessagesContain(msgs, 5, []string{"banana"}) {
		t.Error("should not find 'banana' pattern")
	}
}

// --- Generator Tests ---

func TestIdentityGuard_Cadence(t *testing.T) {
	g := &identityGuard{}

	// 7 turns — should NOT fire
	ctx := &Context{Messages: makeAssistantTurns(7), AgentName: "Nebo"}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not fire at 7 turns")
	}

	// 8 turns — should fire
	ctx.Messages = makeAssistantTurns(8)
	if msgs := g.Generate(ctx); len(msgs) == 0 {
		t.Error("should fire at 8 turns")
	}

	// 16 turns — should fire again
	ctx.Messages = makeAssistantTurns(16)
	if msgs := g.Generate(ctx); len(msgs) == 0 {
		t.Error("should fire at 16 turns")
	}

	// 9 turns — should NOT fire
	ctx.Messages = makeAssistantTurns(9)
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not fire at 9 turns")
	}
}

func TestChannelAdapter_WebSkipped(t *testing.T) {
	g := &channelAdapter{}
	ctx := &Context{Channel: "web"}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should skip for web channel")
	}
}

func TestChannelAdapter_TelegramFires(t *testing.T) {
	g := &channelAdapter{}
	ctx := &Context{Channel: "telegram"}
	msgs := g.Generate(ctx)
	if len(msgs) == 0 {
		t.Fatal("should fire for telegram")
	}
	if !strings.Contains(msgs[0].Content, "Telegram") {
		t.Error("should contain Telegram-specific content")
	}
}

func TestChannelAdapter_EmptyIsWeb(t *testing.T) {
	g := &channelAdapter{}
	ctx := &Context{Channel: ""}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("empty channel should be treated as web (skipped)")
	}
}

func TestToolNudge_NoActiveTask(t *testing.T) {
	g := &toolNudge{}
	ctx := &Context{
		Messages:   makeAssistantTurns(10),
		ActiveTask: "",
	}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not nudge without active task")
	}
}

func TestToolNudge_RecentToolUse(t *testing.T) {
	g := &toolNudge{}
	msgs := []session.Message{
		{Role: "user", Content: "do it"},
		makeAssistantWithToolCall("file"),
		{Role: "tool", Content: "result"},
		{Role: "assistant", Content: "done"},
		{Role: "user", Content: "more"},
		{Role: "assistant", Content: "ok"},
	}
	ctx := &Context{
		Messages:   msgs,
		ActiveTask: "build something",
	}
	if result := g.Generate(ctx); len(result) != 0 {
		t.Error("should not nudge with recent tool use")
	}
}

func TestToolNudge_NoToolUseWithTask(t *testing.T) {
	g := &toolNudge{}
	ctx := &Context{
		Messages:   makeAssistantTurns(6), // 6 turns, no tool calls
		ActiveTask: "build something",
	}
	if result := g.Generate(ctx); len(result) == 0 {
		t.Error("should nudge after 5+ turns without tools and active task")
	}
}

func TestCompactionRecovery_Fires(t *testing.T) {
	g := &compactionRecovery{}
	ctx := &Context{JustCompacted: true}
	if msgs := g.Generate(ctx); len(msgs) == 0 {
		t.Error("should fire when just compacted")
	}
}

func TestCompactionRecovery_Skips(t *testing.T) {
	g := &compactionRecovery{}
	ctx := &Context{JustCompacted: false}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should skip when not compacted")
	}
}

func TestDateTimeRefresh_TooEarly(t *testing.T) {
	g := &dateTimeRefresh{}
	ctx := &Context{
		Iteration:    2,
		RunStartTime: time.Now(), // Just started
	}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not fire before 30 minutes")
	}
}

func TestDateTimeRefresh_Fires(t *testing.T) {
	g := &dateTimeRefresh{}
	ctx := &Context{
		Iteration:    5, // Divisible by 5
		RunStartTime: time.Now().Add(-45 * time.Minute), // 45 minutes ago
	}
	msgs := g.Generate(ctx)
	if len(msgs) == 0 {
		t.Error("should fire after 30+ minutes on 5th iteration")
	}
}

func TestDateTimeRefresh_FirstIteration(t *testing.T) {
	g := &dateTimeRefresh{}
	ctx := &Context{
		Iteration:    1,
		RunStartTime: time.Now().Add(-45 * time.Minute),
	}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not fire on first iteration")
	}
}

func TestMemoryNudge_TooFewTurns(t *testing.T) {
	g := &memoryNudge{}
	ctx := &Context{Messages: makeAssistantTurns(5)}
	if msgs := g.Generate(ctx); len(msgs) != 0 {
		t.Error("should not nudge with < 10 turns")
	}
}

func TestMemoryNudge_RecentMemoryStore(t *testing.T) {
	g := &memoryNudge{}
	msgs := makeAssistantTurns(8)
	// Add a memory store call 3 turns ago
	msgs = append(msgs,
		session.Message{Role: "user", Content: "I am a lawyer"},
		makeAssistantWithToolCall("agent"),
		session.Message{Role: "user", Content: "more chat"},
		session.Message{Role: "assistant", Content: "ok"},
	)
	ctx := &Context{Messages: msgs}
	if result := g.Generate(ctx); len(result) != 0 {
		t.Error("should not nudge when memory was recently used")
	}
}

func TestMemoryNudge_Fires(t *testing.T) {
	g := &memoryNudge{}
	// 12 turns without memory, with self-disclosure
	msgs := makeAssistantTurns(11)
	msgs = append(msgs,
		session.Message{Role: "user", Content: "I work at a law firm downtown"},
		session.Message{Role: "assistant", Content: "That sounds interesting"},
	)
	ctx := &Context{Messages: msgs}
	result := g.Generate(ctx)
	if len(result) == 0 {
		t.Error("should nudge: 12 turns, no memory store, self-disclosure present")
	}
}

func TestMemoryNudge_NoSelfDisclosure(t *testing.T) {
	g := &memoryNudge{}
	// 12 turns without memory, no self-disclosure
	msgs := makeAssistantTurns(11)
	msgs = append(msgs,
		session.Message{Role: "user", Content: "what is the weather today?"},
		session.Message{Role: "assistant", Content: "I can check that"},
	)
	ctx := &Context{Messages: msgs}
	if result := g.Generate(ctx); len(result) != 0 {
		t.Error("should not nudge without self-disclosure patterns")
	}
}
