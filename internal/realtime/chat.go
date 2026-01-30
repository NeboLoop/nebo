package realtime

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"nebo/internal/agenthub"
	"nebo/internal/db"
	"nebo/internal/svc"

	"github.com/google/uuid"
	"nebo/internal/logging"
)

// ChatContext holds the context needed for chat handling
type ChatContext struct {
	hub    *agenthub.Hub
	svcCtx *svc.ServiceContext

	// Pending requests: requestID -> client info
	pending   map[string]*pendingRequest
	pendingMu sync.RWMutex

	// Pending approvals: approvalID -> agentID
	pendingApprovals   map[string]string
	pendingApprovalsMu sync.RWMutex

	// Client hub for broadcasting
	clientHub *Hub
}

type pendingRequest struct {
	client           *Client
	sessionID        string
	userID           string
	prompt           string
	createdAt        time.Time
	streamedContent  string
	isNewChat        bool
}

// NewChatContext creates a new chat context with service context for DB access
func NewChatContext(svcCtx *svc.ServiceContext, clientHub *Hub) (*ChatContext, error) {
	return &ChatContext{
		svcCtx:           svcCtx,
		pending:          make(map[string]*pendingRequest),
		pendingApprovals: make(map[string]string),
		clientHub:        clientHub,
	}, nil
}

// SetHub sets the agent hub for routing messages
func (c *ChatContext) SetHub(hub *agenthub.Hub) {
	c.hub = hub
	// Register ourselves to receive agent responses
	hub.SetResponseHandler(c.handleAgentResponse)
	// Register to receive approval requests
	hub.SetApprovalHandler(c.handleApprovalRequest)
}

// RegisterChatHandler sets up the chat handler
func RegisterChatHandler(chatCtx *ChatContext) {
	SetChatHandler(func(c *Client, msg *Message) {
		go handleChatMessage(c, msg, chatCtx)
	})
	SetApprovalResponseHandler(func(c *Client, msg *Message) {
		go chatCtx.handleApprovalResponse(msg)
	})
}

// handleApprovalRequest forwards an approval request from agent to all connected clients
func (c *ChatContext) handleApprovalRequest(agentID string, requestID string, toolName string, input json.RawMessage) {
	logging.Infof("[Chat] Approval request from agent %s: tool=%s id=%s", agentID, toolName, requestID)

	// Track the pending approval
	c.pendingApprovalsMu.Lock()
	c.pendingApprovals[requestID] = agentID
	c.pendingApprovalsMu.Unlock()

	// Broadcast to all connected clients
	if c.clientHub != nil {
		msg := &Message{
			Type: "approval_request",
			Data: map[string]interface{}{
				"request_id": requestID,
				"tool":       toolName,
				"input":      json.RawMessage(input),
			},
			Timestamp: time.Now(),
		}
		c.clientHub.Broadcast(msg)
	}
}

// handleApprovalResponse processes an approval response from a client
func (c *ChatContext) handleApprovalResponse(msg *Message) {
	requestID, _ := msg.Data["request_id"].(string)
	approved, _ := msg.Data["approved"].(bool)
	always, _ := msg.Data["always"].(bool)

	logging.Infof("[Chat] Approval response: id=%s approved=%v always=%v", requestID, approved, always)

	// Find the agent that requested this approval
	c.pendingApprovalsMu.Lock()
	agentID, ok := c.pendingApprovals[requestID]
	if ok {
		delete(c.pendingApprovals, requestID)
	}
	c.pendingApprovalsMu.Unlock()

	if !ok {
		logging.Infof("[Chat] No pending approval for id=%s", requestID)
		return
	}

	// Send response back to agent via hub
	if c.hub != nil {
		if err := c.hub.SendApprovalResponseWithAlways(agentID, requestID, approved, always); err != nil {
			logging.Errorf("[Chat] Failed to send approval response: %v", err)
		}
	}
}

// handleAgentResponse processes responses and stream chunks from agents
func (c *ChatContext) handleAgentResponse(agentID string, frame *agenthub.Frame) {
	logging.Infof("[Chat] handleAgentResponse: type=%s id=%s payload=%+v", frame.Type, frame.ID, frame.Payload)

	// Handle streaming chunks (sent during processing)
	if frame.Type == "stream" {
		c.pendingMu.Lock()
		req, ok := c.pending[frame.ID]
		if !ok {
			c.pendingMu.Unlock()
			logging.Infof("[Chat] No pending request for stream frame ID: %s (pending count: %d)", frame.ID, len(c.pending))
			return // No pending request for this stream
		}

		// Extract and forward chunk immediately
		logging.Infof("[Chat] Stream payload type: %T, value: %+v", frame.Payload, frame.Payload)
		payload, ok := frame.Payload.(map[string]any)
		if !ok {
			c.pendingMu.Unlock()
			logging.Infof("[Chat] Unexpected payload type: %T", frame.Payload)
			return
		}

		if chunk, ok := payload["chunk"].(string); ok {
			// Accumulate for persistence
			req.streamedContent += chunk
			c.pendingMu.Unlock()
			logging.Infof("[Chat] Streaming %d bytes to client for session %s", len(chunk), req.sessionID)
			sendChatStream(req.client, req.sessionID, chunk)
		} else {
			c.pendingMu.Unlock()
			logging.Infof("[Chat] No chunk in payload: %+v", payload)
		}
		if tool, ok := payload["tool"].(string); ok {
			input, _ := payload["input"].(string)
			sendToolStart(req.client, req.sessionID, tool, input)
		}
		if toolResult, ok := payload["tool_result"].(string); ok {
			sendToolResult(req.client, req.sessionID, toolResult)
		}
		return
	}

	// Handle final response (sent when complete)
	if frame.Type != "res" {
		return
	}

	c.pendingMu.Lock()
	req, ok := c.pending[frame.ID]
	if ok {
		delete(c.pending, frame.ID)
	}
	c.pendingMu.Unlock()

	if !ok {
		logging.Infof("[Chat] No pending request for response %s", frame.ID)
		return
	}

	logging.Infof("[Chat] Received final response for request %s from agent %s", frame.ID, agentID)

	if !frame.OK {
		if req.client != nil {
			sendChatError(req.client, req.sessionID, frame.Error)
		}
		return
	}

	// Check if this is a title generation response (prompt is empty)
	if req.prompt == "" && req.streamedContent != "" {
		// This is a title response - update the chat title
		ctx := context.Background()
		title := strings.TrimSpace(req.streamedContent)
		// Clean up the title (remove quotes, limit length)
		title = strings.Trim(title, "\"'")
		if len(title) > 100 {
			title = title[:97] + "..."
		}
		if title != "" {
			err := c.svcCtx.DB.UpdateChatTitle(ctx, db.UpdateChatTitleParams{
				ID:    req.sessionID,
				Title: title,
			})
			if err != nil {
				logging.Errorf("[Chat] Failed to update chat title: %v", err)
			} else {
				logging.Infof("[Chat] Updated chat title to: %s", title)
			}
		}
		return
	}

	// Save assistant message to DB with day marker
	if req.streamedContent != "" && c.svcCtx != nil && c.svcCtx.DB != nil {
		ctx := context.Background()
		msgID := uuid.New().String()
		_, err := c.svcCtx.DB.CreateChatMessageWithDay(ctx, db.CreateChatMessageWithDayParams{
			ID:       msgID,
			ChatID:   req.sessionID,
			Role:     "assistant",
			Content:  req.streamedContent,
			Metadata: sql.NullString{},
		})
		if err != nil {
			logging.Errorf("[Chat] Failed to save assistant message: %v", err)
		} else {
			logging.Infof("[Chat] Saved assistant message %s to chat %s (len=%d)", msgID, req.sessionID, len(req.streamedContent))
			// Update chat timestamp
			_ = c.svcCtx.DB.UpdateChatTimestamp(ctx, req.sessionID)
		}

		// Generate title for new chats
		if req.isNewChat {
			go c.requestTitleGeneration(agentID, req.sessionID, req.prompt, req.streamedContent)
		}
	}

	// Send completion to client
	if req.client != nil {
		sendChatComplete(req.client, req.sessionID)
	}
}

// companionUserID is the fixed user ID for standalone/companion mode
const companionUserID = "companion-default"

// handleChatMessage processes a chat message by routing to connected agent
func handleChatMessage(c *Client, msg *Message, chatCtx *ChatContext) {
	sessionID, _ := msg.Data["session_id"].(string)
	prompt, _ := msg.Data["prompt"].(string)
	useCompanion, _ := msg.Data["companion"].(bool)

	logging.Infof("[Chat] Processing message for session %s: %s", sessionID, prompt)

	if chatCtx.hub == nil {
		sendChatError(c, sessionID, "Agent hub not initialized")
		return
	}

	// Find any connected agent
	agent := chatCtx.hub.GetAnyAgent()
	if agent == nil {
		sendChatError(c, sessionID, "No agent connected. Make sure gobot is running.")
		return
	}

	ctx := context.Background()
	isNewChat := false

	// Check if this is companion mode or a new chat (session_id is empty or doesn't exist)
	if sessionID == "" {
		if useCompanion {
			// Companion mode: get or create the single companion chat
			if chatCtx.svcCtx != nil && chatCtx.svcCtx.DB != nil {
				chat, err := chatCtx.svcCtx.DB.GetOrCreateCompanionChat(ctx, db.GetOrCreateCompanionChatParams{
					ID:     uuid.New().String(),
					UserID: sql.NullString{String: companionUserID, Valid: true},
				})
				if err != nil {
					logging.Errorf("[Chat] Failed to get/create companion chat: %v", err)
					sendChatError(c, sessionID, "Failed to get companion chat: "+err.Error())
					return
				}
				sessionID = chat.ID
				// Don't send chat_created for companion mode - the chat already exists
			}
		} else {
			// Create new chat (legacy multi-chat mode)
			sessionID = uuid.New().String()
			isNewChat = true

			// Generate title from first message
			title := prompt
			if len(title) > 50 {
				title = title[:47] + "..."
			}
			title = strings.TrimSpace(title)
			if title == "" {
				title = "New Chat"
			}

			if chatCtx.svcCtx != nil && chatCtx.svcCtx.DB != nil {
				_, err := chatCtx.svcCtx.DB.CreateChat(ctx, db.CreateChatParams{
					ID:    sessionID,
					Title: title,
				})
				if err != nil {
					logging.Errorf("[Chat] Failed to create chat: %v", err)
					sendChatError(c, sessionID, "Failed to create chat: "+err.Error())
					return
				}
			}

			// Send the new chat ID back to the client
			sendChatCreated(c, sessionID)
		}
	}

	// Save user message to DB with day marker
	if chatCtx.svcCtx != nil && chatCtx.svcCtx.DB != nil {
		msgID := uuid.New().String()
		_, err := chatCtx.svcCtx.DB.CreateChatMessageWithDay(ctx, db.CreateChatMessageWithDayParams{
			ID:       msgID,
			ChatID:   sessionID,
			Role:     "user",
			Content:  prompt,
			Metadata: sql.NullString{},
		})
		if err != nil {
			logging.Errorf("[Chat] Failed to save user message: %v", err)
		} else {
			logging.Infof("[Chat] Saved user message %s to chat %s", msgID, sessionID)
		}
	} else {
		logging.Errorf("[Chat] Cannot save message - svcCtx=%v DB=%v", chatCtx.svcCtx != nil, chatCtx.svcCtx != nil && chatCtx.svcCtx.DB != nil)
	}

	// Create request and track it
	requestID := fmt.Sprintf("chat-%d", time.Now().UnixNano())

	chatCtx.pendingMu.Lock()
	chatCtx.pending[requestID] = &pendingRequest{
		client:          c,
		sessionID:       sessionID,
		userID:          c.UserID,
		prompt:          prompt,
		createdAt:       time.Now(),
		streamedContent: "",
		isNewChat:       isNewChat,
	}
	logging.Infof("[Chat] Registered pending request: %s for session %s", requestID, sessionID)
	chatCtx.pendingMu.Unlock()

	// Send "run" request to the agent (agent handles "run" not "chat")
	frame := &agenthub.Frame{
		Type:   "req",
		ID:     requestID,
		Method: "run",
		Params: map[string]any{
			"session_key": sessionID,
			"prompt":      prompt,
		},
	}

	if err := chatCtx.hub.SendToAgent(agent.ID, frame); err != nil {
		// Remove from pending on error
		chatCtx.pendingMu.Lock()
		delete(chatCtx.pending, requestID)
		chatCtx.pendingMu.Unlock()

		logging.Errorf("[Chat] Failed to send to agent: %v", err)
		sendChatError(c, sessionID, "Failed to communicate with agent: "+err.Error())
		return
	}

	logging.Infof("[Chat] Routed message to agent %s (request: %s)", agent.ID, requestID)
}

// requestTitleGeneration asks the agent to generate a title for the chat
func (c *ChatContext) requestTitleGeneration(agentID, sessionID, userPrompt, assistantResponse string) {
	if c.hub == nil || c.svcCtx == nil {
		return
	}

	// Create a prompt for title generation
	titlePrompt := fmt.Sprintf(`Generate a short, descriptive title (3-6 words) for this conversation. Only respond with the title, nothing else.

User: %s
Assistant: %s`, userPrompt, assistantResponse)

	requestID := fmt.Sprintf("title-%d", time.Now().UnixNano())

	// Track this as a title generation request
	c.pendingMu.Lock()
	c.pending[requestID] = &pendingRequest{
		client:    nil, // No client to respond to
		sessionID: sessionID,
		createdAt: time.Now(),
		isNewChat: false, // Mark as title request by using empty prompt
		prompt:    "",    // Empty prompt signals this is a title request
	}
	c.pendingMu.Unlock()

	frame := &agenthub.Frame{
		Type:   "req",
		ID:     requestID,
		Method: "generate_title",
		Params: map[string]any{
			"session_key": sessionID,
			"prompt":      titlePrompt,
		},
	}

	if err := c.hub.SendToAgent(agentID, frame); err != nil {
		c.pendingMu.Lock()
		delete(c.pending, requestID)
		c.pendingMu.Unlock()
		logging.Errorf("[Chat] Failed to request title generation: %v", err)
	}
}

func sendChatCreated(c *Client, sessionID string) {
	msg := &Message{
		Type:      "chat_created",
		Data:      map[string]interface{}{"session_id": sessionID},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendChatStream(c *Client, sessionID, content string) {
	if c == nil {
		logging.Infof("[Chat] sendChatStream: client is nil!")
		return
	}
	if c.IsClosed() {
		logging.Infof("[Chat] sendChatStream: client is closed!")
		return
	}
	msg := &Message{
		Type:      "chat_stream",
		Data:      map[string]interface{}{"session_id": sessionID, "content": content},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendChatComplete(c *Client, sessionID string) {
	msg := &Message{
		Type:      "chat_complete",
		Data:      map[string]interface{}{"session_id": sessionID},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendToolStart(c *Client, sessionID, tool, input string) {
	msg := &Message{
		Type:      "tool_start",
		Data:      map[string]interface{}{"session_id": sessionID, "tool": tool, "input": input},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendToolResult(c *Client, sessionID, result string) {
	msg := &Message{
		Type:      "tool_result",
		Data:      map[string]interface{}{"session_id": sessionID, "result": result},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendChatError(c *Client, sessionID, errStr string) {
	msg := &Message{
		Type:      "error",
		Data:      map[string]interface{}{"session_id": sessionID, "error": errStr},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendToClient(c *Client, msg *Message) {
	if c == nil {
		return // No client to send to (e.g., title generation)
	}

	logging.Infof("[Chat] sendToClient: type=%s, data_len=%d", msg.Type, len(msg.Data))

	// Use the safe SendMessage method that handles closed channels
	if err := c.SendMessage(msg); err != nil {
		if err == ErrClientClosed {
			logging.Infof("[Chat] Client connection closed, dropping message")
		} else if err == ErrClientSendBufferFull {
			logging.Infof("[Chat] Client send buffer full, dropping message")
		} else {
			logging.Errorf("[Chat] Failed to send message: %v", err)
		}
	}
}
