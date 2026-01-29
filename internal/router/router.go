// Package router handles routing messages from channels to agents and back.
// It manages the channel → agent bindings and coordinates message flow.
package router

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"time"

	"gobot/internal/agenthub"
	"gobot/internal/channels"

	"github.com/google/uuid"
	"github.com/zeromicro/go-zero/core/logx"
)

// Router routes messages between channels and agents
type Router struct {
	mu       sync.RWMutex
	channels *channels.Manager
	agents   *agenthub.Hub
	bindings *BindingStore

	// Pending requests waiting for agent responses
	pending sync.Map // map[requestID]chan *agenthub.Frame

	// Request timeout
	timeout time.Duration
}

// NewRouter creates a new message router
func NewRouter(channelMgr *channels.Manager, agentHub *agenthub.Hub) *Router {
	return &Router{
		channels: channelMgr,
		agents:   agentHub,
		bindings: NewBindingStore(),
		timeout:  2 * time.Minute, // Default 2 minute timeout for agent responses
	}
}

// SetTimeout sets the timeout for agent responses
func (r *Router) SetTimeout(d time.Duration) {
	r.timeout = d
}

// Route handles an inbound message from a channel
func (r *Router) Route(ctx context.Context, msg channels.InboundMessage) error {
	// Find binding for this channel
	binding, ok := r.bindings.GetByChannel(msg.ChannelType, msg.ChannelID)
	if !ok {
		return fmt.Errorf("no agent bound to channel %s:%s", msg.ChannelType, msg.ChannelID)
	}

	// Check if any agent is connected
	agents := r.agents.GetAllAgents()
	if len(agents) == 0 {
		return fmt.Errorf("no agents connected")
	}

	// Create request frame
	requestID := uuid.New().String()
	frame := &agenthub.Frame{
		Type:   "req",
		ID:     requestID,
		Method: "chat",
		Params: map[string]any{
			"message":      msg.Text,
			"channel_type": msg.ChannelType,
			"channel_id":   msg.ChannelID,
			"sender_id":    msg.SenderID,
			"sender_name":  msg.SenderName,
			"message_id":   msg.MessageID,
			"reply_to_id":  msg.ReplyToID,
			"thread_id":    msg.ThreadID,
		},
	}

	// Create response channel
	respCh := make(chan *agenthub.Frame, 1)
	r.pending.Store(requestID, respCh)
	defer r.pending.Delete(requestID)

	// Send to first available agent (could implement load balancing)
	targetAgent := agents[0]
	if binding.AgentID != "" {
		// Specific agent requested
		if agent := r.agents.GetAgent(binding.AgentID); agent != nil {
			targetAgent = agent
		}
	}

	if err := r.agents.SendToAgent(targetAgent.ID, frame); err != nil {
		return fmt.Errorf("failed to send to agent: %w", err)
	}

	logx.Debugf("[router] Routed message to agent %s: %s", targetAgent.ID, truncate(msg.Text, 50))

	// Wait for response with timeout
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-time.After(r.timeout):
		return fmt.Errorf("agent response timeout")
	case resp := <-respCh:
		return r.handleAgentResponse(ctx, msg, resp)
	}
}

// HandleAgentResponse processes a response from an agent (called by agenthub)
func (r *Router) HandleAgentResponse(requestID string, frame *agenthub.Frame) {
	if respChI, ok := r.pending.Load(requestID); ok {
		respCh := respChI.(chan *agenthub.Frame)
		select {
		case respCh <- frame:
		default:
			// Channel full or closed
		}
	}
}

// handleAgentResponse sends the agent's response back to the channel
func (r *Router) handleAgentResponse(ctx context.Context, original channels.InboundMessage, resp *agenthub.Frame) error {
	if !resp.OK {
		logx.Errorf("[router] Agent error: %s", resp.Error)
		// Optionally send error message to channel
	}

	// Extract response text
	var responseText string
	switch payload := resp.Payload.(type) {
	case string:
		responseText = payload
	case map[string]any:
		if text, ok := payload["text"].(string); ok {
			responseText = text
		} else if content, ok := payload["content"].(string); ok {
			responseText = content
		} else {
			// Try to serialize
			data, _ := json.Marshal(payload)
			responseText = string(data)
		}
	default:
		return fmt.Errorf("unexpected response payload type: %T", resp.Payload)
	}

	if responseText == "" {
		return nil // No response to send
	}

	// Get the channel
	channel, ok := r.channels.Get(original.ChannelType)
	if !ok {
		return fmt.Errorf("channel not found: %s", original.ChannelType)
	}

	// Send response
	outMsg := channels.OutboundMessage{
		ChannelID: original.ChannelID,
		Text:      responseText,
		ReplyToID: original.MessageID,
		ThreadID:  original.ThreadID,
		ParseMode: "markdown",
	}

	return channel.Send(ctx, outMsg)
}

// SetupChannelHandlers sets up message handlers for all channels
func (r *Router) SetupChannelHandlers(ctx context.Context) {
	for _, channelID := range r.channels.List() {
		channel, _ := r.channels.Get(channelID)
		channel.SetHandler(func(msg channels.InboundMessage) {
			if err := r.Route(ctx, msg); err != nil {
				logx.Errorf("[router] Route error: %v", err)
			}
		})
	}
}

// GetBindings returns the binding store for management
func (r *Router) GetBindings() *BindingStore {
	return r.bindings
}

// truncate shortens a string for logging
func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
