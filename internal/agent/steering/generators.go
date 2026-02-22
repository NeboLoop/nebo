package steering

import (
	"fmt"
	"strings"
	"sync"
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

// --- Generator 7: Objective Task Nudge ---
// When an objective exists but the agent hasn't created work tasks yet,
// nudge it to break the objective into trackable steps.

type objectiveTaskNudge struct{}

func (g *objectiveTaskNudge) Name() string { return "objective_task_nudge" }

func (g *objectiveTaskNudge) Generate(ctx *Context) []Message {
	if ctx.ActiveTask == "" {
		return nil // no objective
	}
	if len(ctx.WorkTasks) > 0 {
		return nil // already has tasks
	}
	if countAssistantTurns(ctx.Messages) < 2 {
		return nil // too early
	}
	return []Message{{
		Content:  wrapSteering(g.Name(), tmplObjectiveTaskNudge),
		Position: PositionEnd,
	}}
}

// --- Generator 8: Pending Task Action ---
// When the model responds with text-only and there are pending tasks,
// strongly nudge it to take action rather than narrate intent.

type pendingTaskAction struct{}

func (g *pendingTaskAction) Name() string { return "pending_task_action" }

func (g *pendingTaskAction) Generate(ctx *Context) []Message {
	if len(ctx.WorkTasks) == 0 {
		return nil
	}
	// Count pending/in-progress tasks
	pending := 0
	for _, wt := range ctx.WorkTasks {
		if wt.Status == "pending" || wt.Status == "in_progress" {
			pending++
		}
	}
	if pending == 0 {
		return nil
	}
	// Only fire after the first iteration (text-only response despite pending tasks)
	if ctx.Iteration < 2 {
		return nil
	}
	// Don't fire if tools were used recently (model is actively working)
	if countTurnsSinceAnyToolUse(ctx.Messages) == 0 {
		return nil
	}

	list := formatTaskList(ctx.WorkTasks)
	content := fmt.Sprintf(tmplPendingTaskAction, list)
	return []Message{{
		Content:  wrapSteering(g.Name(), content),
		Position: PositionEnd,
	}}
}

// --- Generator 9: Task Progress ---
// Re-injects the work task list every 8 iterations to keep the agent on track.

type taskProgress struct{}

func (g *taskProgress) Name() string { return "task_progress" }

func (g *taskProgress) Generate(ctx *Context) []Message {
	if len(ctx.WorkTasks) == 0 {
		return nil
	}
	if ctx.Iteration < 4 || ctx.Iteration%8 != 0 {
		return nil
	}
	list := formatTaskList(ctx.WorkTasks)
	content := fmt.Sprintf(tmplTaskProgress, list)
	return []Message{{
		Content:  wrapSteering(g.Name(), content),
		Position: PositionEnd,
	}}
}

// formatTaskList renders work tasks as a checklist.
func formatTaskList(tasks []WorkTask) string {
	var sb strings.Builder
	for _, wt := range tasks {
		icon := "[ ]"
		switch wt.Status {
		case "in_progress":
			icon = "[→]"
		case "completed":
			icon = "[✓]"
		}
		sb.WriteString(fmt.Sprintf("  %s %s\n", icon, wt.Subject))
	}
	return sb.String()
}

// --- Generator 10: Janus Quota Warning ---
// Warns the user when their NeboLoop Janus token budget is running low.

// janusQuotaWarnedSessions tracks which sessions already received a warning.
// Once per session — we don't want to nag every iteration.
var janusQuotaWarnedSessions sync.Map

type janusQuotaWarning struct{}

func (g *janusQuotaWarning) Name() string { return "janus_quota_warning" }

func (g *janusQuotaWarning) Generate(ctx *Context) []Message {
	rl := ctx.JanusRateLimit
	if rl == nil || (rl.SessionLimitTokens <= 0 && rl.WeeklyLimitTokens <= 0) {
		return nil
	}
	sessionRatio := 1.0
	if rl.SessionLimitTokens > 0 {
		sessionRatio = float64(rl.SessionRemainingTokens) / float64(rl.SessionLimitTokens)
	}
	weeklyRatio := 1.0
	if rl.WeeklyLimitTokens > 0 {
		weeklyRatio = float64(rl.WeeklyRemainingTokens) / float64(rl.WeeklyLimitTokens)
	}
	if sessionRatio >= 0.20 && weeklyRatio >= 0.20 {
		return nil // Plenty of quota left in both windows
	}
	// Only warn once per session
	if _, warned := janusQuotaWarnedSessions.LoadOrStore(ctx.SessionID, true); warned {
		return nil
	}
	window := "weekly"
	ratio := weeklyRatio
	if sessionRatio < weeklyRatio {
		window = "session"
		ratio = sessionRatio
	}
	if sessionRatio < 0.20 && weeklyRatio < 0.20 {
		window = "both session and weekly"
	}
	pctUsed := int(100 - ratio*100)
	content := fmt.Sprintf(tmplJanusQuotaWarning, pctUsed, window)
	return []Message{{
		Content:  wrapSteering(g.Name(), content),
		Position: PositionEnd,
	}}
}
