package agenthub

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"gobot/internal/lifecycle"
)

// Frame represents a message frame between server and agent
type Frame struct {
	Type    string `json:"type"`              // req, res, event
	ID      string `json:"id,omitempty"`      // Request/response correlation ID
	Method  string `json:"method,omitempty"`  // For requests
	Params  any    `json:"params,omitempty"`  // Request parameters
	OK      bool   `json:"ok,omitempty"`      // Response success
	Payload any    `json:"payload,omitempty"` // Response data
	Error   string `json:"error,omitempty"`   // Error message
}

// AgentConnection represents a connected agent
type AgentConnection struct {
	ID        string
	Conn      *websocket.Conn
	Send      chan []byte
	CreatedAt time.Time

	mu sync.Mutex
}

// ResponseHandler is called when an agent sends a response
type ResponseHandler func(agentID string, frame *Frame)

// ApprovalRequestHandler is called when an agent requests approval
type ApprovalRequestHandler func(agentID string, requestID string, toolName string, input json.RawMessage)

// Hub manages THE agent connection (single-bot paradigm)
type Hub struct {
	// Single Bot Paradigm: ONE agent connection
	agentMu sync.RWMutex
	agent   *AgentConnection

	// Register channel
	register chan *AgentConnection

	// Unregister channel
	unregister chan *AgentConnection

	// Response handler for routing agent responses
	responseHandler   ResponseHandler
	responseHandlerMu sync.RWMutex

	// Approval request handler
	approvalHandler   ApprovalRequestHandler
	approvalHandlerMu sync.RWMutex

	upgrader websocket.Upgrader
}

// NewHub creates a new agent hub
func NewHub() *Hub {
	return &Hub{
		register:   make(chan *AgentConnection, 1),
		unregister: make(chan *AgentConnection, 1),
		upgrader: websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true // Allow all origins for agents
			},
		},
	}
}

// Run starts the hub's main loop
func (h *Hub) Run(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case agent := <-h.register:
			h.addAgent(agent)
		case agent := <-h.unregister:
			h.removeAgent(agent)
		}
	}
}

// addAgent sets THE agent (single-bot paradigm: disconnects any existing agent first)
func (h *Hub) addAgent(newAgent *AgentConnection) {
	h.agentMu.Lock()
	defer h.agentMu.Unlock()

	// Single Bot Paradigm: If there's an existing agent, disconnect it first
	if h.agent != nil {
		fmt.Printf("[AgentHub] Single Bot: Disconnecting existing agent %s to accept new agent %s\n", h.agent.ID, newAgent.ID)
		close(h.agent.Send)
		if h.agent.Conn != nil {
			h.agent.Conn.Close()
		}
		lifecycle.Emit(lifecycle.EventAgentDisconnected, h.agent.ID)
	}

	h.agent = newAgent
	fmt.Printf("[AgentHub] Single Bot: THE agent connected: %s\n", newAgent.ID)
	lifecycle.Emit(lifecycle.EventAgentConnected, newAgent.ID)
}

// removeAgent removes THE agent if it matches
func (h *Hub) removeAgent(agent *AgentConnection) {
	h.agentMu.Lock()
	defer h.agentMu.Unlock()

	if h.agent != nil && h.agent.ID == agent.ID {
		close(agent.Send)
		if agent.Conn != nil {
			agent.Conn.Close()
		}
		h.agent = nil
		lifecycle.Emit(lifecycle.EventAgentDisconnected, agent.ID)
	}
}

// GetTheAgent returns THE agent (single-bot paradigm)
func (h *Hub) GetTheAgent() *AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	return h.agent
}

// GetAgent returns an agent by ID (for backwards compatibility)
func (h *Hub) GetAgent(agentID string) *AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	if h.agent != nil && h.agent.ID == agentID {
		return h.agent
	}
	return nil
}

// GetAnyAgent returns THE agent (for backwards compatibility)
func (h *Hub) GetAnyAgent() *AgentConnection {
	return h.GetTheAgent()
}

// GetAllAgents returns THE agent as a slice (for backwards compatibility)
func (h *Hub) GetAllAgents() []*AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	if h.agent != nil {
		return []*AgentConnection{h.agent}
	}
	return nil
}

// IsConnected returns true if THE agent is connected
func (h *Hub) IsConnected() bool {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	return h.agent != nil
}

// SendToAgent sends a frame to THE agent (agentID is ignored in single-bot mode)
func (h *Hub) SendToAgent(agentID string, frame *Frame) error {
	agent := h.GetTheAgent()
	if agent == nil {
		return fmt.Errorf("agent not connected")
	}

	data, err := json.Marshal(frame)
	if err != nil {
		return err
	}

	select {
	case agent.Send <- data:
		return nil
	default:
		return fmt.Errorf("agent send buffer full")
	}
}

// Send sends a frame to THE agent (simpler API for single-bot)
func (h *Hub) Send(frame *Frame) error {
	return h.SendToAgent("", frame)
}

// SetResponseHandler sets the handler for agent responses
func (h *Hub) SetResponseHandler(handler ResponseHandler) {
	h.responseHandlerMu.Lock()
	defer h.responseHandlerMu.Unlock()
	h.responseHandler = handler
	fmt.Printf("[AgentHub] Response handler registered (handler=%v)\n", handler != nil)
}

// SetApprovalHandler sets the handler for approval requests
func (h *Hub) SetApprovalHandler(handler ApprovalRequestHandler) {
	h.approvalHandlerMu.Lock()
	defer h.approvalHandlerMu.Unlock()
	h.approvalHandler = handler
}

// SendApprovalResponse sends an approval response back to THE agent
func (h *Hub) SendApprovalResponse(agentID, requestID string, approved bool) error {
	return h.SendApprovalResponseWithAlways(agentID, requestID, approved, false)
}

// SendApprovalResponseWithAlways sends an approval response with the "always" flag
func (h *Hub) SendApprovalResponseWithAlways(agentID, requestID string, approved, always bool) error {
	frame := &Frame{
		Type:    "approval_response",
		ID:      requestID,
		Payload: map[string]any{"approved": approved, "always": always},
	}
	return h.Send(frame)
}

// Broadcast sends a frame to THE agent (same as Send in single-bot mode)
func (h *Hub) Broadcast(frame *Frame) {
	_ = h.Send(frame)
}

// HandleWebSocket handles a WebSocket connection from an agent
func (h *Hub) HandleWebSocket(w http.ResponseWriter, r *http.Request, agentID string) {
	conn, err := h.upgrader.Upgrade(w, r, nil)
	if err != nil {
		fmt.Printf("[AgentHub] Upgrade error: %v\n", err)
		return
	}

	agent := &AgentConnection{
		ID:        agentID,
		Conn:      conn,
		Send:      make(chan []byte, 256),
		CreatedAt: time.Now(),
	}

	h.register <- agent

	// Start reader and writer goroutines
	go h.readPump(agent)
	go h.writePump(agent)
}

// readPump reads messages from the agent
func (h *Hub) readPump(agent *AgentConnection) {
	defer func() {
		fmt.Printf("[AgentHub] readPump exiting for agent %s\n", agent.ID)
		h.unregister <- agent
	}()

	agent.Conn.SetReadLimit(512 * 1024) // 512KB max message size
	agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))
	agent.Conn.SetPongHandler(func(string) error {
		agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))
		return nil
	})

	for {
		_, message, err := agent.Conn.ReadMessage()
		if err != nil {
			fmt.Printf("[AgentHub] ReadMessage error for %s: %v\n", agent.ID, err)
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				fmt.Printf("[AgentHub] Unexpected close error: %v\n", err)
			}
			break
		}

		// Parse frame
		var frame Frame
		if err := json.Unmarshal(message, &frame); err != nil {
			fmt.Printf("[AgentHub] Invalid frame: %v\n", err)
			continue
		}

		// Handle frame
		h.handleFrame(agent, &frame)
	}
}

// writePump writes messages to the agent
func (h *Hub) writePump(agent *AgentConnection) {
	ticker := time.NewTicker(30 * time.Second)
	defer func() {
		ticker.Stop()
		agent.Conn.Close()
	}()

	for {
		select {
		case message, ok := <-agent.Send:
			agent.Conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
			if !ok {
				agent.Conn.WriteMessage(websocket.CloseMessage, []byte{})
				return
			}

			w, err := agent.Conn.NextWriter(websocket.TextMessage)
			if err != nil {
				return
			}
			w.Write(message)

			if err := w.Close(); err != nil {
				return
			}

		case <-ticker.C:
			agent.Conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
			if err := agent.Conn.WriteMessage(websocket.PingMessage, nil); err != nil {
				return
			}
		}
	}
}

// handleFrame processes an incoming frame from an agent
func (h *Hub) handleFrame(agent *AgentConnection, frame *Frame) {
	switch frame.Type {
	case "res":
		// Response to a request we sent - route to handler
		h.responseHandlerMu.RLock()
		handler := h.responseHandler
		h.responseHandlerMu.RUnlock()

		if handler != nil {
			handler(agent.ID, frame)
		}
	case "stream":
		// Streaming chunk from agent - route to same handler as responses
		h.responseHandlerMu.RLock()
		handler := h.responseHandler
		h.responseHandlerMu.RUnlock()

		if handler != nil {
			handler(agent.ID, frame)
		}
	case "approval_request":
		// Approval request from agent - forward to UI
		h.approvalHandlerMu.RLock()
		handler := h.approvalHandler
		h.approvalHandlerMu.RUnlock()

		if handler != nil {
			if payload, ok := frame.Payload.(map[string]any); ok {
				toolName, _ := payload["tool"].(string)
				var inputRaw json.RawMessage
				if input, ok := payload["input"]; ok {
					inputRaw, _ = json.Marshal(input)
				}
				handler(agent.ID, frame.ID, toolName, inputRaw)
			}
		}
	case "event":
		// Event from agent - could be broadcast to other systems
	case "req":
		// Request from agent - handle and respond
		h.handleRequest(agent, frame)
	}
}

// handleRequest handles a request from an agent
func (h *Hub) handleRequest(agent *AgentConnection, frame *Frame) {
	var response Frame
	response.Type = "res"
	response.ID = frame.ID

	switch frame.Method {
	case "ping":
		response.OK = true
		response.Payload = map[string]any{"pong": true, "time": time.Now().Unix()}

	case "status":
		response.OK = true
		response.Payload = map[string]any{
			"agent_id":   agent.ID,
			"connected":  true,
			"uptime_sec": int(time.Since(agent.CreatedAt).Seconds()),
		}

	default:
		response.OK = false
		response.Error = fmt.Sprintf("unknown method: %s", frame.Method)
	}

	data, _ := json.Marshal(response)
	agent.Send <- data
}
