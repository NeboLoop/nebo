package steering

import (
	"fmt"
	"time"
)

// --- Generator 1: Identity Guard ---
// Prevents identity drift in long conversations.

type identityGuard struct{}

func (g *identityGuard) Name() string { return "identity_guard" }

func (g *identityGuard) Generate(ctx *Context) []Message {
	turns := countAssistantTurns(ctx.Messages)
	if turns < 8 || turns%8 != 0 {
		return nil
	}
	return []Message{{
		Content:  wrapSteering(g.Name(), tmplIdentityGuard),
		Position: PositionEnd,
	}}
}

// --- Generator 2: Channel Adapter ---
// Injects channel-specific behavior guidelines.

type channelAdapter struct{}

func (g *channelAdapter) Name() string { return "channel_adapter" }

func (g *channelAdapter) Generate(ctx *Context) []Message {
	// Web is the default — no special steering needed
	if ctx.Channel == "" || ctx.Channel == "web" {
		return nil
	}
	tmpl, ok := channelTemplates[ctx.Channel]
	if !ok {
		return nil
	}
	return []Message{{
		Content:  wrapSteering(g.Name(), tmpl),
		Position: PositionEnd,
	}}
}

// --- Generator 3: Tool Nudge ---
// Reminds the agent to use tools when it's been chatting without action.

type toolNudge struct{}

func (g *toolNudge) Name() string { return "tool_nudge" }

func (g *toolNudge) Generate(ctx *Context) []Message {
	// Only nudge when there's an active task
	if ctx.ActiveTask == "" {
		return nil
	}
	turnsSince := countTurnsSinceAnyToolUse(ctx.Messages)
	// -1 means no tool calls at all; 5+ means it's been a while
	if turnsSince != -1 && turnsSince < 5 {
		return nil
	}
	// Also require at least 5 total assistant turns
	if countAssistantTurns(ctx.Messages) < 5 {
		return nil
	}
	return []Message{{
		Content:  wrapSteering(g.Name(), tmplToolNudge),
		Position: PositionEnd,
	}}
}

// --- Generator 4: Compaction Recovery ---
// Prevents the agent from asking "what were we doing?" after context compaction.

type compactionRecovery struct{}

func (g *compactionRecovery) Name() string { return "compaction_recovery" }

func (g *compactionRecovery) Generate(ctx *Context) []Message {
	if !ctx.JustCompacted {
		return nil
	}
	return []Message{{
		Content:  wrapSteering(g.Name(), tmplCompactionRecovery),
		Position: PositionEnd,
	}}
}

// --- Generator 5: DateTime Refresh ---
// Refreshes stale date/time in long-running sessions.

type dateTimeRefresh struct{}

func (g *dateTimeRefresh) Name() string { return "datetime_refresh" }

func (g *dateTimeRefresh) Generate(ctx *Context) []Message {
	// Only fire after 30+ minutes and not on first iteration
	if ctx.Iteration <= 1 || time.Since(ctx.RunStartTime) < 30*time.Minute {
		return nil
	}
	// Only fire every 5th iteration after the threshold to avoid spamming
	if ctx.Iteration%5 != 0 {
		return nil
	}
	now := time.Now()
	content := fmt.Sprintf(tmplDateTimeRefresh, now.Format("January 2, 2006 3:04 PM MST"))
	return []Message{{
		Content:  wrapSteering(g.Name(), content),
		Position: PositionEnd,
	}}
}

// --- Generator 6: Memory Nudge ---
// Reminds the agent to store important user facts.

type memoryNudge struct{}

func (g *memoryNudge) Name() string { return "memory_nudge" }

func (g *memoryNudge) Generate(ctx *Context) []Message {
	// Need at least 10 assistant turns
	if countAssistantTurns(ctx.Messages) < 10 {
		return nil
	}

	// Check turns since last agent tool use (memory ops go through the agent tool)
	turnsSince := countTurnsSinceToolUse(ctx.Messages, "agent")
	// If agent tool was used recently (within 10 turns), skip
	if turnsSince >= 0 && turnsSince < 10 {
		return nil
	}

	// Check if recent user messages contain self-disclosure patterns
	if !lastNUserMessagesContain(ctx.Messages, 10, selfDisclosurePatterns) {
		return nil
	}

	return []Message{{
		Content:  wrapSteering(g.Name(), tmplMemoryNudge),
		Position: PositionEnd,
	}}
}
