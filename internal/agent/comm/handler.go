package comm

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/runner"
	"github.com/nebolabs/nebo/internal/agent/tools"
	"github.com/nebolabs/nebo/internal/agenthub"
)

// activeTask tracks a running task so it can be canceled.
type activeTask struct {
	Cancel  context.CancelFunc
	Message CommMessage
}

// CommHandler processes incoming comm messages through the full agentic loop.
// It enqueues messages to the comm lane and uses Runner.Run() for processing —
// the same agentic loop used by main lane (same memories, tools, personality).
type CommHandler struct {
	manager *CommPluginManager
	runner  *runner.Runner
	lanes   *agenthub.LaneManager
	agentID string

	activeTasks   map[string]*activeTask
	activeTasksMu sync.Mutex
}

// NewCommHandler creates a new comm handler
func NewCommHandler(manager *CommPluginManager, agentID string) *CommHandler {
	return &CommHandler{
		manager:     manager,
		agentID:     agentID,
		activeTasks: make(map[string]*activeTask),
	}
}

// SetRunner sets the runner (called after runner creation during agent startup)
func (h *CommHandler) SetRunner(r *runner.Runner) {
	h.runner = r
}

// SetLanes sets the lane manager for enqueueing work
func (h *CommHandler) SetLanes(lanes *agenthub.LaneManager) {
	h.lanes = lanes
}

// GetManager returns the underlying plugin manager
func (h *CommHandler) GetManager() *CommPluginManager {
	return h.manager
}

// Handle processes an incoming comm message by enqueueing it to the comm lane.
// This is called by the plugin's message handler and returns immediately.
// Task messages get dedicated processing with lifecycle status updates.
func (h *CommHandler) Handle(msg CommMessage) {
	if h.runner == nil {
		fmt.Printf("[Comm] Warning: runner not set, dropping message from %s\n", msg.From)
		return
	}
	if h.lanes == nil {
		fmt.Printf("[Comm] Warning: lanes not set, dropping message from %s\n", msg.From)
		return
	}

	fmt.Printf("[Comm] Received message: from=%s topic=%s type=%s\n", msg.From, msg.Topic, msg.Type)

	switch msg.Type {
	case CommTypeTask:
		// Check for cancellation — spec says canceled tasks arrive on the tasks topic
		if msg.TaskStatus == TaskStatusCanceled {
			h.cancelTask(msg.TaskID)
			return
		}
		h.lanes.EnqueueAsync(context.Background(), agenthub.LaneComm, func(taskCtx context.Context) error {
			// Create a cancellable context so we can honor cancellations
			ctx, cancel := context.WithCancel(taskCtx)
			h.trackTask(msg.TaskID, cancel, msg)
			defer h.untrackTask(msg.TaskID)
			return h.processTask(ctx, msg)
		})
	case CommTypeTaskResult:
		h.lanes.EnqueueAsync(context.Background(), agenthub.LaneComm, func(taskCtx context.Context) error {
			return h.processTaskResult(taskCtx, msg)
		})
	default:
		h.lanes.EnqueueAsync(context.Background(), agenthub.LaneComm, func(taskCtx context.Context) error {
			return h.processMessage(taskCtx, msg)
		})
	}
}

// Shutdown cancels all in-progress tasks and sends failure status for each.
// Called during graceful shutdown.
func (h *CommHandler) Shutdown(ctx context.Context) {
	h.activeTasksMu.Lock()
	tasks := make(map[string]*activeTask, len(h.activeTasks))
	for id, at := range h.activeTasks {
		tasks[id] = at
	}
	h.activeTasks = make(map[string]*activeTask)
	h.activeTasksMu.Unlock()

	for _, at := range tasks {
		at.Cancel()
		h.sendTaskFailure(ctx, at.Message, "bot shutting down")
	}
}

// =============================================================================
// Task tracking for cancellation support
// =============================================================================

func (h *CommHandler) trackTask(taskID string, cancel context.CancelFunc, msg CommMessage) {
	h.activeTasksMu.Lock()
	h.activeTasks[taskID] = &activeTask{Cancel: cancel, Message: msg}
	h.activeTasksMu.Unlock()
}

func (h *CommHandler) untrackTask(taskID string) {
	h.activeTasksMu.Lock()
	delete(h.activeTasks, taskID)
	h.activeTasksMu.Unlock()
}

func (h *CommHandler) cancelTask(taskID string) {
	h.activeTasksMu.Lock()
	at, ok := h.activeTasks[taskID]
	if ok {
		delete(h.activeTasks, taskID)
	}
	h.activeTasksMu.Unlock()

	if ok {
		at.Cancel()
		fmt.Printf("[Comm] Canceled task %s\n", taskID)
		// Send canceled status back
		h.sendTaskStatus(context.Background(), at.Message, TaskStatusCanceled)
	}
}

// =============================================================================
// Message processing
// =============================================================================

// processMessage runs a comm message through the full agentic loop
func (h *CommHandler) processMessage(ctx context.Context, msg CommMessage) error {
	// Build session key for this conversation
	conversationID := msg.ConversationID
	if conversationID == "" {
		conversationID = msg.ID
	}
	sessionKey := fmt.Sprintf("comm-%s-%s", msg.Topic, conversationID)

	// Build prompt with comm context prefix
	prompt := h.buildPrompt(msg)

	// Run through the full agentic loop (same Runner.Run() as main lane)
	events, err := h.runner.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		Prompt:     prompt,
		Origin:     tools.OriginComm,
	})
	if err != nil {
		fmt.Printf("[Comm] Error running agentic loop for %s: %v\n", sessionKey, err)
		return err
	}

	// Collect response text from stream events
	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
	}

	// Send response back through the comm channel
	if result.Len() > 0 {
		h.sendResponse(ctx, msg, result.String())
	}

	return nil
}

// buildPrompt constructs a prompt with comm channel context
func (h *CommHandler) buildPrompt(msg CommMessage) string {
	return fmt.Sprintf("[Comm Channel: %s | From: %s | Type: %s]\n\n%s",
		msg.Topic, msg.From, msg.Type, msg.Content)
}

// sendResponse sends a reply back through the comm plugin
func (h *CommHandler) sendResponse(ctx context.Context, original CommMessage, response string) {
	reply := CommMessage{
		ID:             uuid.New().String(),
		From:           h.agentID,
		To:             original.From,
		Topic:          original.Topic,
		ConversationID: original.ConversationID,
		Type:           CommTypeMessage,
		Content:        response,
		Timestamp:      time.Now().Unix(),
	}

	if err := h.manager.Send(ctx, reply); err != nil {
		fmt.Printf("[Comm] Error sending response to %s: %v\n", original.From, err)
	}
}

// =============================================================================
// A2A task processing
// =============================================================================

// processTask handles an incoming A2A task request.
// Sends "working" status, runs the agentic loop, then sends the result.
func (h *CommHandler) processTask(ctx context.Context, msg CommMessage) error {
	// Acknowledge: send "working" status
	h.sendTaskStatus(ctx, msg, TaskStatusWorking)

	// Use task ID as session key for conversation persistence
	sessionKey := fmt.Sprintf("task-%s", msg.TaskID)

	// Build prompt with A2A task context
	prompt := fmt.Sprintf("[A2A Task %s from %s]\n\n%s", msg.TaskID, msg.From, msg.Content)

	// Run through the full agentic loop
	events, err := h.runner.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		Prompt:     prompt,
		Origin:     tools.OriginComm,
	})
	if err != nil {
		// Check if it was a cancellation
		if ctx.Err() == context.Canceled {
			fmt.Printf("[Comm] Task %s was canceled\n", msg.TaskID)
			return nil // Status already sent by cancelTask
		}
		fmt.Printf("[Comm] Error running task %s: %v\n", msg.TaskID, err)
		h.sendTaskFailure(ctx, msg, err.Error())
		return err
	}

	// Collect response text
	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
	}

	// Send completed result with artifacts
	if result.Len() > 0 {
		h.sendTaskResult(ctx, msg, result.String())
	} else {
		h.sendTaskFailure(ctx, msg, "no output produced")
	}

	return nil
}

// processTaskResult handles an incoming A2A task result (from a task we submitted).
// Feeds the result back through the agentic loop so the agent can use it.
func (h *CommHandler) processTaskResult(ctx context.Context, msg CommMessage) error {
	sessionKey := fmt.Sprintf("task-result-%s", msg.TaskID)
	prompt := fmt.Sprintf("[A2A Task Result %s | Status: %s]\n\n%s",
		msg.TaskID, msg.TaskStatus, msg.Content)

	events, err := h.runner.Run(ctx, &runner.RunRequest{
		SessionKey: sessionKey,
		Prompt:     prompt,
		Origin:     tools.OriginComm,
	})
	if err != nil {
		fmt.Printf("[Comm] Error processing task result %s: %v\n", msg.TaskID, err)
		return err
	}

	// Drain events (agent may take actions but we don't send a reply for results)
	for range events {
	}

	return nil
}

// sendTaskStatus sends a lifecycle status update for an A2A task.
func (h *CommHandler) sendTaskStatus(ctx context.Context, original CommMessage, status TaskStatus) {
	msg := CommMessage{
		ID:            uuid.New().String(),
		From:          h.agentID,
		To:            original.From,
		Topic:         original.Topic,
		Type:          CommTypeTaskStatus,
		TaskID:        original.TaskID,
		CorrelationID: original.CorrelationID,
		TaskStatus:    status,
		Timestamp:     time.Now().Unix(),
	}

	if err := h.manager.Send(ctx, msg); err != nil {
		fmt.Printf("[Comm] Error sending task status %s for %s: %v\n", status, original.TaskID, err)
	}
}

// sendTaskFailure sends a failed A2A task result with an error message.
func (h *CommHandler) sendTaskFailure(ctx context.Context, original CommMessage, errMsg string) {
	msg := CommMessage{
		ID:            uuid.New().String(),
		From:          h.agentID,
		To:            original.From,
		Topic:         original.Topic,
		Type:          CommTypeTaskResult,
		TaskID:        original.TaskID,
		CorrelationID: original.CorrelationID,
		TaskStatus:    TaskStatusFailed,
		Error:         errMsg,
		Timestamp:     time.Now().Unix(),
	}

	if err := h.manager.Send(ctx, msg); err != nil {
		fmt.Printf("[Comm] Error sending task failure for %s: %v\n", original.TaskID, err)
	}
}

// sendTaskResult sends a completed A2A task result with artifacts.
func (h *CommHandler) sendTaskResult(ctx context.Context, original CommMessage, response string) {
	msg := CommMessage{
		ID:            uuid.New().String(),
		From:          h.agentID,
		To:            original.From,
		Topic:         original.Topic,
		Type:          CommTypeTaskResult,
		TaskID:        original.TaskID,
		CorrelationID: original.CorrelationID,
		TaskStatus:    TaskStatusCompleted,
		Artifacts: []TaskArtifact{
			{
				Parts: []ArtifactPart{
					{Type: "text", Text: response},
				},
			},
		},
		Timestamp: time.Now().Unix(),
	}

	if err := h.manager.Send(ctx, msg); err != nil {
		fmt.Printf("[Comm] Error sending task result for %s: %v\n", original.TaskID, err)
	}
}

// =============================================================================
// CommService interface methods (used by agent tool to avoid import cycles)
// =============================================================================

// Send sends a message through the active comm plugin (CommService interface)
func (h *CommHandler) Send(ctx context.Context, to, topic, content string, msgType string) error {
	mt := CommMessageType(msgType)
	if mt == "" {
		mt = CommTypeMessage
	}

	msg := CommMessage{
		ID:        uuid.New().String(),
		From:      h.agentID,
		To:        to,
		Topic:     topic,
		Type:      mt,
		Content:   content,
		Timestamp: time.Now().Unix(),
	}

	return h.manager.Send(ctx, msg)
}

// Subscribe subscribes to a comm topic (CommService interface)
func (h *CommHandler) Subscribe(ctx context.Context, topic string) error {
	return h.manager.Subscribe(ctx, topic)
}

// Unsubscribe unsubscribes from a comm topic (CommService interface)
func (h *CommHandler) Unsubscribe(ctx context.Context, topic string) error {
	return h.manager.Unsubscribe(ctx, topic)
}

// ListTopics returns currently subscribed topics (CommService interface)
func (h *CommHandler) ListTopics() []string {
	return h.manager.ListTopics()
}

// PluginName returns the active plugin name (CommService interface)
func (h *CommHandler) PluginName() string {
	if p := h.manager.GetActive(); p != nil {
		return p.Name()
	}
	return ""
}

// IsConnected returns whether the active plugin is connected (CommService interface)
func (h *CommHandler) IsConnected() bool {
	if p := h.manager.GetActive(); p != nil {
		return p.IsConnected()
	}
	return false
}

// CommAgentID returns this agent's ID in the comm network (CommService interface)
func (h *CommHandler) CommAgentID() string {
	return h.agentID
}
