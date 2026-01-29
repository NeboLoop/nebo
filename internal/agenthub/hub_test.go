package agenthub

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

func TestNewHub(t *testing.T) {
	hub := NewHub()
	if hub == nil {
		t.Fatal("NewHub returned nil")
	}

	if hub.register == nil {
		t.Error("register channel is nil")
	}
	if hub.unregister == nil {
		t.Error("unregister channel is nil")
	}
}

func TestHubAddRemoveAgent(t *testing.T) {
	hub := NewHub()

	// Start hub
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go hub.Run(ctx)

	// Give hub time to start
	time.Sleep(10 * time.Millisecond)

	// Create mock agent connection (Conn is nil for unit tests)
	agent := &AgentConnection{
		ID:        "agent-1",
		Send:      make(chan []byte, 256),
		CreatedAt: time.Now(),
		// Conn is nil - hub.removeAgent handles this safely
	}

	// Register agent
	hub.register <- agent
	time.Sleep(10 * time.Millisecond)

	// Verify agent was added
	retrieved := hub.GetAgent("agent-1")
	if retrieved == nil {
		t.Error("agent not found after registration")
	}
	if retrieved.ID != "agent-1" {
		t.Errorf("expected agent ID 'agent-1', got %s", retrieved.ID)
	}

	// Unregister agent (Conn is nil but removeAgent handles this)
	hub.unregister <- agent
	time.Sleep(10 * time.Millisecond)

	// Verify agent was removed
	retrieved = hub.GetAgent("agent-1")
	if retrieved != nil {
		t.Error("agent should be removed after unregistration")
	}
}

func TestGetAllAgents(t *testing.T) {
	hub := NewHub()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go hub.Run(ctx)
	time.Sleep(10 * time.Millisecond)

	// Add multiple agents
	agent1 := &AgentConnection{
		ID: "agent-1", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}
	agent2 := &AgentConnection{
		ID: "agent-2", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}
	agent3 := &AgentConnection{
		ID: "agent-3", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}

	hub.register <- agent1
	hub.register <- agent2
	hub.register <- agent3
	time.Sleep(20 * time.Millisecond)

	// Get all agents
	agents := hub.GetAllAgents()
	if len(agents) != 3 {
		t.Errorf("expected 3 agents, got %d", len(agents))
	}
}

func TestSendToAgent(t *testing.T) {
	hub := NewHub()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go hub.Run(ctx)
	time.Sleep(10 * time.Millisecond)

	agent := &AgentConnection{
		ID: "agent-1", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}
	hub.register <- agent
	time.Sleep(10 * time.Millisecond)

	// Send to existing agent
	frame := &Frame{
		Type:   "event",
		Method: "test",
	}
	err := hub.SendToAgent("agent-1", frame)
	if err != nil {
		t.Errorf("SendToAgent failed: %v", err)
	}

	// Verify message was sent
	select {
	case msg := <-agent.Send:
		var received Frame
		if err := json.Unmarshal(msg, &received); err != nil {
			t.Errorf("failed to unmarshal frame: %v", err)
		}
		if received.Type != "event" {
			t.Errorf("expected type 'event', got %s", received.Type)
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("no message received")
	}

	// Send to non-existent agent
	err = hub.SendToAgent("nonexistent", frame)
	if err == nil {
		t.Error("expected error for non-existent agent")
	}
}

func TestBroadcast(t *testing.T) {
	hub := NewHub()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go hub.Run(ctx)
	time.Sleep(10 * time.Millisecond)

	agent1 := &AgentConnection{
		ID: "agent-1", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}
	agent2 := &AgentConnection{
		ID: "agent-2", Send: make(chan []byte, 256), CreatedAt: time.Now(),
	}

	hub.register <- agent1
	hub.register <- agent2
	time.Sleep(20 * time.Millisecond)

	// Broadcast to all agents
	frame := &Frame{
		Type:    "event",
		Method:  "broadcast",
		Payload: "hello",
	}
	hub.Broadcast(frame)

	// Verify all agents received
	for _, agent := range []*AgentConnection{agent1, agent2} {
		select {
		case msg := <-agent.Send:
			var received Frame
			json.Unmarshal(msg, &received)
			if received.Method != "broadcast" {
				t.Errorf("expected method 'broadcast', got %s", received.Method)
			}
		case <-time.After(100 * time.Millisecond):
			t.Errorf("agent %s did not receive broadcast", agent.ID)
		}
	}
}

func TestFrame(t *testing.T) {
	// Test request frame
	reqFrame := Frame{
		Type:   "req",
		ID:     "123",
		Method: "ping",
		Params: map[string]string{"key": "value"},
	}

	data, err := json.Marshal(reqFrame)
	if err != nil {
		t.Fatalf("failed to marshal frame: %v", err)
	}

	var decoded Frame
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("failed to unmarshal frame: %v", err)
	}

	if decoded.Type != "req" {
		t.Errorf("expected type 'req', got %s", decoded.Type)
	}
	if decoded.Method != "ping" {
		t.Errorf("expected method 'ping', got %s", decoded.Method)
	}

	// Test response frame
	resFrame := Frame{
		Type:    "res",
		ID:      "123",
		OK:      true,
		Payload: map[string]any{"pong": true},
	}

	data, _ = json.Marshal(resFrame)
	json.Unmarshal(data, &decoded)

	if decoded.OK != true {
		t.Error("expected OK to be true")
	}
}

func TestWebSocketHandler(t *testing.T) {
	hub := NewHub()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go hub.Run(ctx)

	// Create test server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hub.HandleWebSocket(w, r, "test-agent")
	}))
	defer server.Close()

	// Connect via WebSocket
	wsURL := "ws" + strings.TrimPrefix(server.URL, "http")
	ws, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer ws.Close()

	// Give time for registration
	time.Sleep(50 * time.Millisecond)

	// Verify agent is registered
	agent := hub.GetAgent("test-agent")
	if agent == nil {
		t.Fatal("agent not registered")
	}

	// Send a ping request
	pingReq := Frame{
		Type:   "req",
		ID:     "1",
		Method: "ping",
	}
	data, _ := json.Marshal(pingReq)
	if err := ws.WriteMessage(websocket.TextMessage, data); err != nil {
		t.Fatalf("failed to send ping: %v", err)
	}

	// Read response
	ws.SetReadDeadline(time.Now().Add(1 * time.Second))
	_, msg, err := ws.ReadMessage()
	if err != nil {
		t.Fatalf("failed to read response: %v", err)
	}

	var response Frame
	if err := json.Unmarshal(msg, &response); err != nil {
		t.Fatalf("failed to unmarshal response: %v", err)
	}

	if response.Type != "res" {
		t.Errorf("expected response type 'res', got %s", response.Type)
	}
	if response.ID != "1" {
		t.Errorf("expected ID '1', got %s", response.ID)
	}
	if !response.OK {
		t.Error("expected OK to be true")
	}
}

func TestHandleRequestStatus(t *testing.T) {
	hub := NewHub()

	agent := &AgentConnection{
		ID:        "test-agent",
		Send:      make(chan []byte, 256),
		CreatedAt: time.Now().Add(-10 * time.Second), // 10 seconds ago
	}

	// Send status request
	frame := &Frame{
		Type:   "req",
		ID:     "status-1",
		Method: "status",
	}

	hub.handleRequest(agent, frame)

	// Read response
	select {
	case msg := <-agent.Send:
		var response Frame
		json.Unmarshal(msg, &response)

		if !response.OK {
			t.Error("expected OK to be true")
		}

		payload, ok := response.Payload.(map[string]any)
		if !ok {
			t.Fatal("payload is not a map")
		}

		if payload["agent_id"] != "test-agent" {
			t.Errorf("expected agent_id 'test-agent', got %v", payload["agent_id"])
		}
		if payload["connected"] != true {
			t.Error("expected connected to be true")
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("no response received")
	}
}

func TestHandleRequestUnknownMethod(t *testing.T) {
	hub := NewHub()

	agent := &AgentConnection{
		ID:   "test-agent",
		Send: make(chan []byte, 256),
	}

	frame := &Frame{
		Type:   "req",
		ID:     "unknown-1",
		Method: "unknown_method",
	}

	hub.handleRequest(agent, frame)

	select {
	case msg := <-agent.Send:
		var response Frame
		json.Unmarshal(msg, &response)

		if response.OK {
			t.Error("expected OK to be false for unknown method")
		}
		if response.Error == "" {
			t.Error("expected error message")
		}
	case <-time.After(100 * time.Millisecond):
		t.Error("no response received")
	}
}

func TestAgentConnectionMutex(t *testing.T) {
	agent := &AgentConnection{
		ID:   "test",
		Send: make(chan []byte, 256),
	}

	// Test mutex is usable
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			agent.mu.Lock()
			agent.mu.Unlock()
		}()
	}
	wg.Wait()
}

func TestHubRunContextCancel(t *testing.T) {
	hub := NewHub()

	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan struct{})
	go func() {
		hub.Run(ctx)
		close(done)
	}()

	// Cancel context
	cancel()

	// Hub should exit
	select {
	case <-done:
		// Expected
	case <-time.After(1 * time.Second):
		t.Error("hub did not exit after context cancel")
	}
}
