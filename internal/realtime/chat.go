package realtime

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/svc"

	"github.com/google/uuid"
	"github.com/neboloop/nebo/internal/logging"
)

// ChatContext holds the context needed for chat handling
type ChatContext struct {
	hub    *agenthub.Hub
	svcCtx *svc.ServiceContext

	// Pending requests: requestID -> client info
	pending   map[string]*pendingRequest
	pendingMu sync.RWMutex

	// Active sessions: sessionID -> requestID (for stream resumption)
	activeSessions   map[string]string
	activeSessionsMu sync.RWMutex

	// Pending approvals: approvalID -> agentID
	pendingApprovals   map[string]string
	pendingApprovalsMu sync.RWMutex

	// Pending ask requests: requestID -> agentID
	pendingAsks   map[string]string
	pendingAsksMu sync.RWMutex

	// Client hub for broadcasting
	clientHub *Hub
}

type toolCallInfo struct {
	ID     string `json:"id,omitempty"`
	Name   string `json:"name"`
	Input  string `json:"input"`
	Output string `json:"output,omitempty"`
	Status string `json:"status"`
}

type contentBlock struct {
	Type          string          `json:"type"`                    // "text", "tool", "image", or "ask"
	Text          string          `json:"text,omitempty"`          // accumulated text for text blocks
	ToolCallIndex *int            `json:"toolCallIndex,omitempty"` // index into toolCalls for tool blocks
	ImageURL      string          `json:"imageURL,omitempty"`      // URL for image blocks
	AskRequestID  string          `json:"askRequestId,omitempty"`  // ask request ID
	AskPrompt     string          `json:"askPrompt,omitempty"`     // ask prompt text
	AskWidgets    json.RawMessage `json:"askWidgets,omitempty"`    // ask widget definitions
	AskResponse   string          `json:"askResponse,omitempty"`   // user response (filled when answered)
}

type pendingRequest struct {
	client           *Client
	sessionID        string
	userID           string
	prompt           string
	createdAt        time.Time
	streamedContent  string
	isNewChat        bool
	toolCalls        []toolCallInfo
	thinking         string
	contentBlocks    []contentBlock
	messageID        string // DB message ID for partial saves
	cleanSentLen     int    // length of fence-cleaned content already sent to client
}

// NewChatContext creates a new chat context with service context for DB access
func NewChatContext(svcCtx *svc.ServiceContext, clientHub *Hub) (*ChatContext, error) {
	return &ChatContext{
		svcCtx:           svcCtx,
		pending:          make(map[string]*pendingRequest),
		activeSessions:   make(map[string]string),
		pendingApprovals: make(map[string]string),
		pendingAsks:      make(map[string]string),
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
	// Register to receive ask requests (interactive user prompts)
	hub.SetAskHandler(c.handleAskRequest)
	// Register to receive agent events (lane updates, etc.)
	hub.SetEventHandler(c.handleAgentEvent)
}

// handleAgentEvent forwards agent events (lane updates, etc.) to all UI clients
func (c *ChatContext) handleAgentEvent(agentID string, frame *agenthub.Frame) {
	if c.clientHub == nil {
		fmt.Printf("[Chat] handleAgentEvent: clientHub is nil, dropping event method=%s\n", frame.Method)
		return
	}
	data := make(map[string]interface{})
	if payload, ok := frame.Payload.(map[string]any); ok {
		data = payload
	}
	clientCount := c.clientHub.GetClientCount()
	fmt.Printf("[Chat] handleAgentEvent: broadcasting method=%s to %d clients\n", frame.Method, clientCount)
	if err := c.clientHub.Broadcast(&Message{
		Type:      frame.Method,
		Data:      data,
		Timestamp: time.Now(),
	}); err != nil {
		fmt.Printf("[Chat] handleAgentEvent: broadcast error: %v\n", err)
	}
}

// RegisterChatHandler sets up the chat handler
func RegisterChatHandler(chatCtx *ChatContext) {
	SetChatHandler(func(c *Client, msg *Message) {
		go handleChatMessage(c, msg, chatCtx)
	})
	SetApprovalResponseHandler(func(c *Client, msg *Message) {
		go chatCtx.handleApprovalResponse(msg)
	})
	SetRequestIntroductionHandler(func(c *Client, msg *Message) {
		go handleRequestIntroduction(c, msg, chatCtx)
	})
	SetCheckStreamHandler(func(c *Client, msg *Message) {
		go handleCheckStream(c, msg, chatCtx)
	})
	SetCancelHandler(func(c *Client, msg *Message) {
		go handleCancel(c, msg, chatCtx)
	})
	SetSessionResetHandler(func(c *Client, msg *Message) {
		handleSessionReset(c, msg, chatCtx)
	})
	SetAskResponseHandler(func(c *Client, msg *Message) {
		go chatCtx.handleAskResponse(msg)
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

// handleAskRequest forwards an ask request from agent to all connected clients
func (c *ChatContext) handleAskRequest(agentID string, requestID string, prompt string, widgets json.RawMessage) {
	logging.Infof("[Chat] Ask request from agent %s: id=%s prompt=%q", agentID, requestID, prompt)

	// Track the pending ask
	c.pendingAsksMu.Lock()
	c.pendingAsks[requestID] = agentID
	c.pendingAsksMu.Unlock()

	// Append an ask content block to the active pending request (if one is streaming)
	c.pendingMu.Lock()
	for _, req := range c.pending {
		req.contentBlocks = append(req.contentBlocks, contentBlock{
			Type:         "ask",
			AskRequestID: requestID,
			AskPrompt:    prompt,
			AskWidgets:   widgets,
		})
		break // only one active request expected
	}
	c.pendingMu.Unlock()

	// Broadcast to all connected clients
	if c.clientHub != nil {
		msg := &Message{
			Type: "ask_request",
			Data: map[string]interface{}{
				"request_id": requestID,
				"prompt":     prompt,
				"widgets":    json.RawMessage(widgets),
			},
			Timestamp: time.Now(),
		}
		c.clientHub.Broadcast(msg)
	}
}

// handleAskResponse processes an ask response from a client
func (c *ChatContext) handleAskResponse(msg *Message) {
	requestID, _ := msg.Data["request_id"].(string)
	value, _ := msg.Data["value"].(string)

	logging.Infof("[Chat] Ask response: id=%s value=%q", requestID, value)

	// Find the agent that requested this ask
	c.pendingAsksMu.Lock()
	agentID, ok := c.pendingAsks[requestID]
	if ok {
		delete(c.pendingAsks, requestID)
	}
	c.pendingAsksMu.Unlock()

	if !ok {
		logging.Infof("[Chat] No pending ask for id=%s", requestID)
		return
	}

	// Update the ask content block with the response
	c.pendingMu.Lock()
	for _, req := range c.pending {
		for i := range req.contentBlocks {
			if req.contentBlocks[i].AskRequestID == requestID {
				req.contentBlocks[i].AskResponse = value
				break
			}
		}
	}
	c.pendingMu.Unlock()

	// Send response back to agent via hub
	if c.hub != nil {
		if err := c.hub.SendAskResponse(agentID, requestID, value); err != nil {
			logging.Errorf("[Chat] Failed to send ask response: %v", err)
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

		// Handle text chunks
		if chunk, ok := payload["chunk"].(string); ok {
			// Accumulate content
			req.streamedContent += chunk

			// Stream all accumulated content that hasn't been sent yet.
			// Back up to a valid UTF-8 rune boundary so we don't split multi-byte chars (emojis, CJK, etc.)
			safeLen := len(req.streamedContent)
			for safeLen > req.cleanSentLen && safeLen < len(req.streamedContent) && !utf8.RuneStart(req.streamedContent[safeLen]) {
				safeLen--
			}

			delta := ""
			if safeLen > req.cleanSentLen {
				delta = req.streamedContent[req.cleanSentLen:safeLen]
				req.cleanSentLen = safeLen
			}

			if delta != "" {
				if len(req.contentBlocks) == 0 || req.contentBlocks[len(req.contentBlocks)-1].Type != "text" {
					req.contentBlocks = append(req.contentBlocks, contentBlock{Type: "text", Text: delta})
				} else {
					req.contentBlocks[len(req.contentBlocks)-1].Text += delta
				}
			}
			c.pendingMu.Unlock()
			if delta != "" {
				logging.Infof("[Chat] Streaming %d bytes to client for session %s", len(delta), req.sessionID)
				sendChatStream(req.client, req.sessionID, delta)
			}
		} else {
			c.pendingMu.Unlock()
		}

		// Handle tool start
		if tool, ok := payload["tool"].(string); ok {
			input := extractStringOrJSON(payload["input"])
			toolID, _ := payload["tool_id"].(string)
			fmt.Printf("[Chat] Tool start: %s (id=%s) input_len=%d\n", tool, toolID, len(input))
			// Flush buffered text before inserting tool card so text isn't split mid-word
			c.pendingMu.Lock()
			if req, ok := c.pending[frame.ID]; ok {
				if len(req.streamedContent) > req.cleanSentLen {
					flush := req.streamedContent[req.cleanSentLen:]
					if len(req.contentBlocks) == 0 || req.contentBlocks[len(req.contentBlocks)-1].Type != "text" {
						req.contentBlocks = append(req.contentBlocks, contentBlock{Type: "text", Text: flush})
					} else {
						req.contentBlocks[len(req.contentBlocks)-1].Text += flush
					}
					req.cleanSentLen = len(req.streamedContent)
					if req.client != nil {
						sendChatStream(req.client, req.sessionID, flush)
					}
				}
			}
			c.pendingMu.Unlock()
			// Track tool call and content block
			c.pendingMu.Lock()
			if req, ok := c.pending[frame.ID]; ok {
				toolIdx := len(req.toolCalls)
				req.toolCalls = append(req.toolCalls, toolCallInfo{
					ID:     toolID,
					Name:   tool,
					Input:  input,
					Status: "running",
				})
				req.contentBlocks = append(req.contentBlocks, contentBlock{
					Type:          "tool",
					ToolCallIndex: intPtr(toolIdx),
				})
			}
			c.pendingMu.Unlock()
			// Save partial message on tool start (AI has paused to use a tool)
			go c.savePartialMessage(frame.ID)
			sendToolStart(req.client, req.sessionID, tool, toolID, input)
		}

		// Handle tool result
		if toolResult, ok := payload["tool_result"].(string); ok {
			toolName, _ := payload["tool_name"].(string)
			toolID, _ := payload["tool_id"].(string)
			fmt.Printf("[Chat] Tool result: %s (id=%s) len=%d\n", toolName, toolID, len(toolResult))
			// Update matching tool call with result (by ID or last one as fallback)
			c.pendingMu.Lock()
			if req, ok := c.pending[frame.ID]; ok && len(req.toolCalls) > 0 {
				updated := false
				// Try to find by tool ID first
				for i := range req.toolCalls {
					if req.toolCalls[i].ID == toolID && toolID != "" {
						req.toolCalls[i].Output = toolResult
						req.toolCalls[i].Status = "complete"
						updated = true
						break
					}
				}
				// Fallback: update first running tool
				if !updated {
					for i := range req.toolCalls {
						if req.toolCalls[i].Status == "running" {
							req.toolCalls[i].Output = toolResult
							req.toolCalls[i].Status = "complete"
							break
						}
					}
				}
			}
			// If the tool produced an image, append an image content block
			if imageURL, ok := payload["image_url"].(string); ok && imageURL != "" {
				if req, ok := c.pending[frame.ID]; ok {
					req.contentBlocks = append(req.contentBlocks, contentBlock{
						Type:     "image",
						ImageURL: imageURL,
					})
				}
			}
			c.pendingMu.Unlock()
			sendToolResult(req.client, req.sessionID, toolName, toolID, toolResult)
			// Send image event if the tool produced one
			if imageURL, ok := payload["image_url"].(string); ok && imageURL != "" {
				sendImage(req.client, req.sessionID, imageURL)
			}
		} else if payload["tool_result"] != nil {
			fmt.Printf("[Chat] WARNING: tool_result not a string, type=%T\n", payload["tool_result"])
		}

		// Handle thinking/reasoning content
		if thinking, ok := payload["thinking"].(string); ok {
			// Accumulate thinking content
			c.pendingMu.Lock()
			if req, ok := c.pending[frame.ID]; ok {
				req.thinking += thinking
			}
			c.pendingMu.Unlock()
			sendThinking(req.client, req.sessionID, thinking)
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

	// Remove from active sessions
	c.activeSessionsMu.Lock()
	delete(c.activeSessions, req.sessionID)
	c.activeSessionsMu.Unlock()

	logging.Infof("[Chat] Received final response for request %s from agent %s", frame.ID, agentID)

	// Flush remaining buffered content for persistence.
	if len(req.streamedContent) > req.cleanSentLen {
		remaining := req.streamedContent[req.cleanSentLen:]
		if len(req.contentBlocks) == 0 || req.contentBlocks[len(req.contentBlocks)-1].Type != "text" {
			req.contentBlocks = append(req.contentBlocks, contentBlock{Type: "text", Text: remaining})
		} else {
			req.contentBlocks[len(req.contentBlocks)-1].Text += remaining
		}
		if req.client != nil {
			sendChatStream(req.client, req.sessionID, remaining)
		}
	}

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

	// Assistant message persistence is handled by the runner (single write path).
	// Only update the chat timestamp and generate title for new chats.
	if req.streamedContent != "" && c.svcCtx != nil && c.svcCtx.DB != nil {
		_ = c.svcCtx.DB.UpdateChatTimestamp(context.Background(), req.sessionID)

		if req.isNewChat {
			go c.requestTitleGeneration(agentID, req.sessionID, req.prompt, req.streamedContent)
		}
	}

	// Send completion to client
	if req.client != nil {
		sendChatComplete(req.client, req.sessionID)
	}
}

// companionUserIDFallback is used when no user is authenticated (standalone mode)
const companionUserIDFallback = "companion-default"

// handleRequestIntroduction handles a request for the agent to introduce itself to a new user
func handleRequestIntroduction(c *Client, msg *Message, chatCtx *ChatContext) {
	sessionID, _ := msg.Data["session_id"].(string)

	logging.Infof("[Chat] *** INTRODUCTION REQUESTED *** session=%s user=%s", sessionID, c.UserID)
	fmt.Printf("[Chat] *** INTRODUCTION REQUESTED *** session=%s user=%s\n", sessionID, c.UserID)

	if chatCtx.hub == nil {
		sendChatError(c, sessionID, "Agent hub not initialized")
		return
	}

	// Wait for agent to connect (handles startup race where frontend loads before agent)
	agent := waitForAgent(chatCtx.hub, 5*time.Second)
	if agent == nil {
		sendChatError(c, sessionID, "No agent connected. Make sure nebo is running.")
		return
	}

	// Create request and track it
	requestID := fmt.Sprintf("intro-%d", time.Now().UnixNano())

	chatCtx.pendingMu.Lock()
	chatCtx.pending[requestID] = &pendingRequest{
		client:          c,
		sessionID:       sessionID,
		userID:          c.UserID,
		prompt:          "__introduction__", // Marker to distinguish from title generation
		createdAt:       time.Now(),
		streamedContent: "",
		isNewChat:       false,
	}
	logging.Infof("[Chat] Registered pending introduction request: %s for session %s", requestID, sessionID)
	chatCtx.pendingMu.Unlock()

	// Send introduction request to the agent
	// The agent will check if this user needs introduction and respond appropriately
	frame := &agenthub.Frame{
		Type:   "req",
		ID:     requestID,
		Method: "introduce",
		Params: map[string]any{
			"session_key": sessionID,
			"user_id":     c.UserID,
		},
	}

	if err := chatCtx.hub.SendToAgent(agent.ID, frame); err != nil {
		// Remove from pending on error
		chatCtx.pendingMu.Lock()
		delete(chatCtx.pending, requestID)
		chatCtx.pendingMu.Unlock()

		logging.Errorf("[Chat] Failed to send introduction request to agent: %v", err)
		sendChatError(c, sessionID, "Failed to communicate with agent: "+err.Error())
		return
	}

	logging.Infof("[Chat] Sent introduction request to agent %s (request: %s)", agent.ID, requestID)
}

// handleCancel processes a cancel request from the client.
// It sends a cancel frame to the agent hub and broadcasts chat_cancelled to UI clients.
// IMPORTANT: Always broadcasts chat_cancelled even if agent is unavailable,
// so the frontend can reset its loading state.
func handleCancel(c *Client, msg *Message, chatCtx *ChatContext) {
	sessionID, _ := msg.Data["session_id"].(string)
	logging.Infof("[Chat] Cancel requested for session %s", sessionID)

	// Try to send cancel to agent (best-effort)
	if chatCtx.hub != nil {
		if agent := chatCtx.hub.GetAnyAgent(); agent != nil {
			frame := &agenthub.Frame{
				Type:   "req",
				ID:     fmt.Sprintf("cancel-%d", time.Now().UnixNano()),
				Method: "cancel",
				Params: map[string]any{
					"session_id": sessionID,
				},
			}
			if err := chatCtx.hub.SendToAgent(agent.ID, frame); err != nil {
				logging.Errorf("[Chat] Failed to send cancel to agent: %v", err)
			}
		} else {
			logging.Infof("[Chat] No agent connected for cancel, will still clean up and broadcast")
		}
	}

	// Clean up pending request for this session
	chatCtx.activeSessionsMu.Lock()
	requestID, hadActive := chatCtx.activeSessions[sessionID]
	delete(chatCtx.activeSessions, sessionID)
	chatCtx.activeSessionsMu.Unlock()

	if hadActive {
		chatCtx.pendingMu.Lock()
		delete(chatCtx.pending, requestID)
		chatCtx.pendingMu.Unlock()
	}

	// Always broadcast cancellation to all UI clients so frontend can reset state
	cancelMsg := &Message{
		Type:      "chat_cancelled",
		Data:      map[string]interface{}{"session_id": sessionID},
		Timestamp: time.Now(),
	}
	if chatCtx.clientHub != nil {
		chatCtx.clientHub.Broadcast(cancelMsg)
	}
}

// handleSessionReset clears session messages so the companion chat starts fresh.
// The session ID stays the same (Single Bot Paradigm), but all messages are deleted.
// The chatID from the frontend is the sessionKey (e.g., "companion-default"),
// which is used as both the chats.id and chat_messages.chat_id.
func handleSessionReset(c *Client, msg *Message, chatCtx *ChatContext) {
	chatID, _ := msg.Data["session_id"].(string)
	logging.Infof("[Chat] Session reset requested for %s", chatID)

	if chatID == "" || chatCtx.svcCtx == nil || chatCtx.svcCtx.DB == nil {
		sendSessionResetResult(c, chatID, false)
		return
	}

	ctx := context.Background()

	// Delete messages from chat_messages using chatID (= sessionKey)
	if err := chatCtx.svcCtx.DB.DeleteChatMessagesByChatId(ctx, chatID); err != nil {
		logging.Errorf("[Chat] Failed to delete chat messages: %v", err)
		sendSessionResetResult(c, chatID, false)
		return
	}

	// Reset session metadata via session name resolution
	sess, err := chatCtx.svcCtx.DB.GetSessionByName(ctx, sql.NullString{String: chatID, Valid: true})
	if err == nil {
		if err := chatCtx.svcCtx.DB.ResetSession(ctx, sess.ID); err != nil {
			logging.Errorf("[Chat] Failed to reset session: %v", err)
		}
	}

	sendSessionResetResult(c, chatID, true)
}

func sendSessionResetResult(c *Client, sessionID string, ok bool) {
	resp := &Message{
		Type:      "session_reset",
		Data:      map[string]interface{}{"session_id": sessionID, "ok": ok},
		Timestamp: time.Now(),
	}
	data, _ := json.Marshal(resp)
	select {
	case c.send <- data:
	default:
	}
}

// handleChatMessage processes a chat message by routing to connected agent
func handleChatMessage(c *Client, msg *Message, chatCtx *ChatContext) {
	sessionID, _ := msg.Data["session_id"].(string)
	prompt, _ := msg.Data["prompt"].(string)
	useCompanion, _ := msg.Data["companion"].(bool)
	system, _ := msg.Data["system"].(string)

	if chatCtx.hub == nil {
		sendChatError(c, sessionID, "Agent hub not initialized")
		return
	}

	// Wait for agent to connect (handles startup race where frontend loads before agent)
	agent := waitForAgent(chatCtx.hub, 5*time.Second)
	if agent == nil {
		sendChatError(c, sessionID, "No agent connected. Make sure nebo is running.")
		return
	}

	ctx := context.Background()
	isNewChat := false

	// Check if this is companion mode or a new chat (session_id is empty or doesn't exist)
	if sessionID == "" {
		if useCompanion {
			// Companion mode: get or create the single companion chat for this user
			userID := c.UserID
			if userID == "" {
				userID = companionUserIDFallback
			}
			if chatCtx.svcCtx != nil && chatCtx.svcCtx.DB != nil {
				chat, err := chatCtx.svcCtx.DB.GetOrCreateCompanionChat(ctx, db.GetOrCreateCompanionChatParams{
					ID:     uuid.New().String(),
					UserID: sql.NullString{String: userID, Valid: true},
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

	// User message persistence is handled by the runner (single write path).

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
	chatCtx.pendingMu.Unlock()

	// Track active session for stream resumption
	chatCtx.activeSessionsMu.Lock()
	chatCtx.activeSessions[sessionID] = requestID
	chatCtx.activeSessionsMu.Unlock()

	logging.Infof("[Chat] Registered pending request: %s for session %s", requestID, sessionID)

	// Send "run" request to the agent (agent handles "run" not "chat")
	frame := &agenthub.Frame{
		Type:   "req",
		ID:     requestID,
		Method: "run",
		Params: map[string]any{
			"session_key": sessionID,
			"prompt":      prompt,
			"user_id":     c.UserID, // Thread user_id to agent for user-scoped operations
			"system":      system,   // Optional system prompt override (dev assistant, etc.)
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

// handleCheckStream handles a client checking if there's an active stream for their session
func handleCheckStream(c *Client, msg *Message, chatCtx *ChatContext) {
	sessionID, _ := msg.Data["session_id"].(string)
	if sessionID == "" {
		return
	}

	logging.Infof("[Chat] Checking for active stream: session=%s", sessionID)

	// Check if there's an active request for this session
	chatCtx.activeSessionsMu.RLock()
	requestID, hasActive := chatCtx.activeSessions[sessionID]
	chatCtx.activeSessionsMu.RUnlock()

	if !hasActive {
		logging.Infof("[Chat] No active stream for session %s", sessionID)
		sendStreamStatus(c, sessionID, false, "")
		return
	}

	// Get the accumulated content
	chatCtx.pendingMu.RLock()
	req, ok := chatCtx.pending[requestID]
	var content string
	if ok {
		content = req.streamedContent
	}
	chatCtx.pendingMu.RUnlock()

	if !ok {
		logging.Infof("[Chat] Request %s not found for session %s", requestID, sessionID)
		sendStreamStatus(c, sessionID, false, "")
		return
	}

	logging.Infof("[Chat] Resuming stream for session %s, content length=%d", sessionID, len(content))

	// Update the client reference so new chunks go to this client
	chatCtx.pendingMu.Lock()
	if req, ok := chatCtx.pending[requestID]; ok {
		req.client = c
	}
	chatCtx.pendingMu.Unlock()

	// Send the accumulated content and mark as streaming
	sendStreamStatus(c, sessionID, true, content)
}

func sendStreamStatus(c *Client, sessionID string, active bool, content string) {
	msg := &Message{
		Type: "stream_status",
		Data: map[string]interface{}{
			"session_id": sessionID,
			"active":     active,
			"content":    content,
		},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
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

func sendToolStart(c *Client, sessionID, tool, toolID, input string) {
	msg := &Message{
		Type: "tool_start",
		Data: map[string]interface{}{
			"session_id": sessionID,
			"tool":       tool,
			"tool_id":    toolID,
			"input":      input,
		},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendToolResult(c *Client, sessionID, toolName, toolID, result string) {
	msg := &Message{
		Type: "tool_result",
		Data: map[string]interface{}{
			"session_id": sessionID,
			"result":     result,
			"tool_name":  toolName,
			"tool_id":    toolID,
		},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendImage(c *Client, sessionID, imageURL string) {
	msg := &Message{
		Type: "image",
		Data: map[string]interface{}{
			"session_id": sessionID,
			"image_url":  imageURL,
		},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

func sendThinking(c *Client, sessionID, thinking string) {
	msg := &Message{
		Type:      "thinking",
		Data:      map[string]interface{}{"session_id": sessionID, "content": thinking},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

// waitForAgent polls for a connected agent, waiting up to timeout.
// Handles the startup race where the frontend connects before the agent.
func waitForAgent(hub *agenthub.Hub, timeout time.Duration) *agenthub.AgentConnection {
	agent := hub.GetAnyAgent()
	if agent != nil {
		return agent
	}
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		time.Sleep(250 * time.Millisecond)
		if agent = hub.GetAnyAgent(); agent != nil {
			return agent
		}
	}
	return nil
}

func sendChatError(c *Client, sessionID, errStr string) {
	msg := &Message{
		Type:      "error",
		Data:      map[string]interface{}{"session_id": sessionID, "error": errStr},
		Timestamp: time.Now(),
	}
	sendToClient(c, msg)
}

// getPayloadKeys returns a list of keys in the payload map for debugging
// extractStringOrJSON converts a value to a string.
// If it's already a string, returns it directly.
// If it's a map/slice (from JSON unmarshal), re-marshals it to JSON string.
func extractStringOrJSON(v any) string {
	if v == nil {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	// Re-marshal non-string values (e.g., map[string]interface{} from json.RawMessage)
	data, err := json.Marshal(v)
	if err != nil {
		return fmt.Sprintf("%v", v)
	}
	return string(data)
}

func getPayloadKeys(payload map[string]any) []string {
	keys := make([]string, 0, len(payload))
	for k := range payload {
		keys = append(keys, k)
	}
	return keys
}

// buildMetadata serializes tool calls, thinking, and content blocks to metadata JSON
func (c *ChatContext) buildMetadata(req *pendingRequest) sql.NullString {
	if len(req.toolCalls) == 0 && req.thinking == "" && len(req.contentBlocks) == 0 {
		return sql.NullString{}
	}
	metaMap := make(map[string]interface{})
	if len(req.toolCalls) > 0 {
		metaMap["toolCalls"] = req.toolCalls
	}
	if req.thinking != "" {
		metaMap["thinking"] = req.thinking
	}
	if len(req.contentBlocks) > 0 {
		metaMap["contentBlocks"] = req.contentBlocks
	}
	metaJSON, err := json.Marshal(metaMap)
	if err != nil {
		return sql.NullString{}
	}
	return sql.NullString{String: string(metaJSON), Valid: true}
}

// savePartialMessage is a no-op. Message persistence is handled by the runner
// (single write path). Kept as a stub since it's called from handleAgentEvent.
func (c *ChatContext) savePartialMessage(requestID string) {}

func intPtr(i int) *int {
	return &i
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
