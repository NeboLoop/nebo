package agenthub

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"github.com/neboloop/nebo/internal/lifecycle"
	"github.com/neboloop/nebo/internal/middleware"
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
	Name      string          // Agent name: "main", "coder", "researcher", etc.
	Conn      *websocket.Conn
	Send      chan []byte
	CreatedAt time.Time
	Metadata  map[string]any  // Capabilities, status, config

	mu sync.Mutex
}

// ResponseHandler is called when an agent sends a response
type ResponseHandler func(agentID string, frame *Frame)

// ApprovalRequestHandler is called when an agent requests approval
type ApprovalRequestHandler func(agentID string, requestID string, toolName string, input json.RawMessage)

// AskRequestHandler is called when an agent sends an interactive prompt
type AskRequestHandler func(agentID string, requestID string, prompt string, widgets json.RawMessage)

// Hub manages agent connections (multi-agent paradigm)
type Hub struct {
	// Multi-agent: map of agent name -> connection
	agentMu sync.RWMutex
	agents  map[string]*AgentConnection

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

	// Ask request handler (interactive user prompts)
	askHandler   AskRequestHandler
	askHandlerMu sync.RWMutex

	// Event handler for agent events (lane updates, etc.)
	eventHandler   func(agentID string, frame *Frame)
	eventHandlerMu sync.RWMutex

	// Sync request/response tracking
	pendingSync   map[string]*pendingSync
	pendingSyncMu sync.RWMutex

	upgrader websocket.Upgrader
}

// NewHub creates a new agent hub
func NewHub() *Hub {
	return &Hub{
		agents:     make(map[string]*AgentConnection),
		register:   make(chan *AgentConnection, 1),
		unregister: make(chan *AgentConnection, 1),
		upgrader: websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				origin := r.Header.Get("Origin")
				return origin == "" || middleware.IsLocalhostOrigin(origin)
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

// addAgent adds an agent to the hub (multi-agent paradigm)
func (h *Hub) addAgent(newAgent *AgentConnection) {
	h.agentMu.Lock()
	defer h.agentMu.Unlock()

	name := newAgent.Name
	if name == "" {
		name = "main"
		newAgent.Name = name
	}

	// If there's an existing agent with the same name, disconnect it first
	if existing, ok := h.agents[name]; ok {
		fmt.Printf("[AgentHub] Disconnecting existing agent %s (name=%s) to accept new agent %s\n", existing.ID, name, newAgent.ID)
		close(existing.Send)
		if existing.Conn != nil {
			existing.Conn.Close()
		}
		lifecycle.Emit(lifecycle.EventAgentDisconnected, existing.ID)
	}

	h.agents[name] = newAgent
	fmt.Printf("[AgentHub] Agent connected: %s (name=%s)\n", newAgent.ID, name)
	lifecycle.Emit(lifecycle.EventAgentConnected, newAgent.ID)

	// Send "ready" event to agent so it can start any initialization work
	// This is processed asynchronously after the agent is fully registered
	go func() {
		readyFrame := &Frame{
			Type:   "event",
			Method: "ready",
			Payload: map[string]any{
				"agent_id": newAgent.ID,
				"name":     name,
			},
		}
		if data, err := json.Marshal(readyFrame); err == nil {
			select {
			case newAgent.Send <- data:
				fmt.Printf("[AgentHub] Sent ready event to agent %s\n", name)
			default:
				fmt.Printf("[AgentHub] Could not send ready event to agent %s (buffer full)\n", name)
			}
		}
	}()
}

// removeAgent removes an agent from the hub
func (h *Hub) removeAgent(agent *AgentConnection) {
	h.agentMu.Lock()
	defer h.agentMu.Unlock()

	name := agent.Name
	if name == "" {
		name = "main"
	}

	// Only remove if this agent is still the registered one
	// (prevents double-close if addAgent already replaced it)
	if existing, ok := h.agents[name]; ok && existing.ID == agent.ID {
		// Safe close - channel may already be closed by addAgent
		defer func() {
			if r := recover(); r != nil {
				// Channel already closed, ignore
			}
		}()
		close(agent.Send)
		if agent.Conn != nil {
			agent.Conn.Close()
		}
		delete(h.agents, name)
		lifecycle.Emit(lifecycle.EventAgentDisconnected, agent.ID)
	}
}

// GetTheAgent returns the main agent (for backwards compatibility)
func (h *Hub) GetTheAgent() *AgentConnection {
	return h.GetAgentByName("main")
}

// GetAgentByName returns an agent by name
func (h *Hub) GetAgentByName(name string) *AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	if name == "" {
		name = "main"
	}
	return h.agents[name]
}

// GetAgent returns an agent by ID
func (h *Hub) GetAgent(agentID string) *AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	for _, agent := range h.agents {
		if agent.ID == agentID {
			return agent
		}
	}
	return nil
}

// GetAnyAgent returns any connected agent (for backwards compatibility)
func (h *Hub) GetAnyAgent() *AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	for _, agent := range h.agents {
		return agent
	}
	return nil
}

// GetAllAgents returns all connected agents
func (h *Hub) GetAllAgents() []*AgentConnection {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	agents := make([]*AgentConnection, 0, len(h.agents))
	for _, agent := range h.agents {
		agents = append(agents, agent)
	}
	return agents
}

// IsConnected returns true if at least one agent is connected
func (h *Hub) IsConnected() bool {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	return len(h.agents) > 0
}

// IsAgentConnected returns true if the specified agent is connected
func (h *Hub) IsAgentConnected(name string) bool {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	if name == "" {
		name = "main"
	}
	_, ok := h.agents[name]
	return ok
}

// AgentCount returns the number of connected agents
func (h *Hub) AgentCount() int {
	h.agentMu.RLock()
	defer h.agentMu.RUnlock()
	return len(h.agents)
}

// SendToAgent sends a frame to a specific agent by ID
func (h *Hub) SendToAgent(agentID string, frame *Frame) error {
	agent := h.GetAgent(agentID)
	if agent == nil {
		// Fallback to main agent for backwards compatibility
		agent = h.GetTheAgent()
	}
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

// SendToAgentByName sends a frame to a specific agent by name
func (h *Hub) SendToAgentByName(name string, frame *Frame) error {
	agent := h.GetAgentByName(name)
	if agent == nil {
		return fmt.Errorf("agent %s not connected", name)
	}

	data, err := json.Marshal(frame)
	if err != nil {
		return err
	}

	select {
	case agent.Send <- data:
		return nil
	default:
		return fmt.Errorf("agent %s send buffer full", name)
	}
}

// Send sends a frame to the main agent (simpler API for backwards compatibility)
func (h *Hub) Send(frame *Frame) error {
	return h.SendToAgentByName("main", frame)
}

// pendingSync tracks synchronous request/response pairs
type pendingSync struct {
	ch chan *Frame
}

// SendRequestSync sends a request to the main agent and waits for the matching response.
// Returns the response frame or an error on timeout.
func (h *Hub) SendRequestSync(ctx context.Context, method string, params map[string]any) (*Frame, error) {
	id := fmt.Sprintf("sync-%d", time.Now().UnixNano())
	frame := &Frame{
		Type:   "req",
		ID:     id,
		Method: method,
		Params: params,
	}

	ch := make(chan *Frame, 1)
	h.pendingSyncMu.Lock()
	if h.pendingSync == nil {
		h.pendingSync = make(map[string]*pendingSync)
	}
	h.pendingSync[id] = &pendingSync{ch: ch}
	h.pendingSyncMu.Unlock()

	defer func() {
		h.pendingSyncMu.Lock()
		delete(h.pendingSync, id)
		h.pendingSyncMu.Unlock()
	}()

	if err := h.Send(frame); err != nil {
		return nil, err
	}

	select {
	case resp := <-ch:
		return resp, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

// routeSyncResponse checks if a response matches a pending sync request.
func (h *Hub) routeSyncResponse(frame *Frame) bool {
	h.pendingSyncMu.RLock()
	ps, ok := h.pendingSync[frame.ID]
	h.pendingSyncMu.RUnlock()
	if !ok {
		return false
	}
	select {
	case ps.ch <- frame:
	default:
	}
	return true
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

// SetAskHandler sets the handler for ask requests (interactive user prompts)
func (h *Hub) SetAskHandler(handler AskRequestHandler) {
	h.askHandlerMu.Lock()
	defer h.askHandlerMu.Unlock()
	h.askHandler = handler
}

// SetEventHandler sets the handler for agent events (lane updates, etc.)
func (h *Hub) SetEventHandler(handler func(agentID string, frame *Frame)) {
	h.eventHandlerMu.Lock()
	defer h.eventHandlerMu.Unlock()
	h.eventHandler = handler
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

// SendAskResponse sends an ask response back to the agent
func (h *Hub) SendAskResponse(agentID, requestID, value string) error {
	frame := &Frame{
		Type:    "ask_response",
		ID:      requestID,
		Payload: map[string]any{"request_id": requestID, "value": value},
	}
	return h.Send(frame)
}

// Broadcast sends a frame to all connected agents
func (h *Hub) Broadcast(frame *Frame) {
	h.agentMu.RLock()
	agents := make([]*AgentConnection, 0, len(h.agents))
	for _, agent := range h.agents {
		agents = append(agents, agent)
	}
	h.agentMu.RUnlock()

	data, err := json.Marshal(frame)
	if err != nil {
		return
	}

	for _, agent := range agents {
		select {
		case agent.Send <- data:
		default:
			// Skip agents with full buffers
		}
	}
}

// HandleWebSocket handles a WebSocket connection from an agent
func (h *Hub) HandleWebSocket(w http.ResponseWriter, r *http.Request, agentID string) {
	conn, err := h.upgrader.Upgrade(w, r, nil)
	if err != nil {
		fmt.Printf("[AgentHub] Upgrade error: %v\n", err)
		return
	}

	// Parse agent name from URL query parameter (default: "main")
	agentName := r.URL.Query().Get("name")
	if agentName == "" {
		agentName = "main"
	}

	agent := &AgentConnection{
		ID:        agentID,
		Name:      agentName,
		Conn:      conn,
		Send:      make(chan []byte, 256),
		CreatedAt: time.Now(),
		Metadata:  make(map[string]any),
	}

	h.register <- agent

	// Start reader and writer goroutines
	go h.readPump(agent)
	go h.writePump(agent)
}

// readPump reads messages from the agent
func (h *Hub) readPump(agent *AgentConnection) {
	defer func() {
		h.unregister <- agent
	}()

	agent.Conn.SetReadLimit(10 * 1024 * 1024) // 10MB max message size (tool results can be large)
	agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))
	agent.Conn.SetPongHandler(func(string) error {
		agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))
		return nil
	})
	agent.Conn.SetPingHandler(func(appData string) error {
		agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))
		return agent.Conn.WriteControl(websocket.PongMessage, []byte(appData), time.Now().Add(time.Second))
	})

	for {
		_, message, err := agent.Conn.ReadMessage()
		if err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				fmt.Printf("[AgentHub] Unexpected close error for %s: %v\n", agent.ID, err)
			}
			break
		}

		// Reset deadline on any received data (belt-and-suspenders with pong handler)
		agent.Conn.SetReadDeadline(time.Now().Add(10 * time.Minute))

		// Parse frame
		var frame Frame
		if err := json.Unmarshal(message, &frame); err != nil {
			preview := string(message)
			if len(preview) > 200 {
				preview = preview[:200] + "..."
			}
			fmt.Printf("[AgentHub] Invalid frame from %s: %v (len=%d raw=%q)\n", agent.ID, err, len(message), preview)
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
		// Check if this is a sync response first
		if h.routeSyncResponse(frame) {
			return
		}
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
	case "ask_request":
		// Interactive prompt from agent — forward to UI
		h.askHandlerMu.RLock()
		handler := h.askHandler
		h.askHandlerMu.RUnlock()

		if handler != nil {
			if payload, ok := frame.Payload.(map[string]any); ok {
				prompt, _ := payload["prompt"].(string)
				var widgetsRaw json.RawMessage
				if w, ok := payload["widgets"]; ok {
					widgetsRaw, _ = json.Marshal(w)
				}
				handler(agent.ID, frame.ID, prompt, widgetsRaw)
			}
		}
	case "event":
		fmt.Printf("[AgentHub] Event frame from %s: method=%s\n", agent.ID, frame.Method)
		h.eventHandlerMu.RLock()
		handler := h.eventHandler
		h.eventHandlerMu.RUnlock()
		if handler != nil {
			handler(agent.ID, frame)
		} else {
			fmt.Printf("[AgentHub] WARNING: no event handler registered!\n")
		}
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
