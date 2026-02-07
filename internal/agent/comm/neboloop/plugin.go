// Package neboloop implements a CommPlugin that connects to a NeboLoop server
// via MQTT v5 for real-time inter-agent communication.
//
// NeboLoop runs an embedded mochi-mqtt broker. This plugin connects as an MQTT v5
// client using autopaho for automatic reconnection, authenticating with credentials
// obtained via the NeboLoop REST API token exchange flow. All topic permissions are
// enforced server-side via Redis-backed ACL.
package neboloop

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/eclipse/paho.golang/autopaho"
	"github.com/eclipse/paho.golang/paho"

	"github.com/nebolabs/nebo/internal/agent/comm"
	"github.com/nebolabs/nebo/internal/plugin"
)

// Plugin implements comm.CommPlugin for NeboLoop MQTT v5 transport.
type Plugin struct {
	cm      *autopaho.ConnectionManager
	handler func(comm.CommMessage)

	agentID   string
	botID     string
	broker    string
	apiServer string

	mqttUsername string
	mqttPassword string

	card *comm.AgentCard // Stored for re-publish on reconnect

	topics    map[string]bool
	connected bool
	authDead  bool // Set when credentials are revoked (0x86) — stop reconnecting
	mu        sync.RWMutex

	cancelConn context.CancelFunc // Cancel the autopaho connection manager
}

// tokenExchangeRequest is the JSON body for POST /api/v1/bots/exchange-token.
type tokenExchangeRequest struct {
	Token string `json:"token"`
}

// tokenExchangeResponse is the JSON response from POST /api/v1/bots/exchange-token.
type tokenExchangeResponse struct {
	MQTTUsername string `json:"mqtt_username"`
	MQTTPassword string `json:"mqtt_password"`
}

// New creates a new NeboLoop MQTT plugin.
func New() *Plugin {
	return &Plugin{
		topics: make(map[string]bool),
	}
}

func (p *Plugin) Name() string    { return "neboloop" }
func (p *Plugin) Version() string { return "2.0.0" }

// Connect establishes an MQTT v5 connection to the NeboLoop broker.
//
// Config keys:
//   - broker:           MQTT broker address (e.g., "tcp://localhost:1883")
//   - api_server:       NeboLoop REST API URL (e.g., "http://localhost:8888")
//   - connection_token: One-time token to exchange for MQTT credentials
//   - mqtt_username:    Direct MQTT username (skip token exchange)
//   - mqtt_password:    Direct MQTT password (skip token exchange)
//   - bot_id:           Bot UUID for topic addressing
func (p *Plugin) Connect(ctx context.Context, config map[string]string) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.connected {
		return fmt.Errorf("already connected")
	}

	// Read config
	p.broker = config["broker"]
	if p.broker == "" {
		return fmt.Errorf("neboloop: 'broker' config is required (e.g., tcp://localhost:1883)")
	}
	p.apiServer = config["api_server"]
	p.botID = config["bot_id"]
	p.mqttUsername = config["mqtt_username"]
	p.mqttPassword = config["mqtt_password"]

	// Exchange connection token for MQTT credentials if needed
	token := config["connection_token"]
	if token != "" && p.mqttUsername == "" {
		if p.apiServer == "" {
			return fmt.Errorf("neboloop: 'api_server' is required when using connection_token")
		}
		username, password, err := exchangeToken(ctx, p.apiServer, token)
		if err != nil {
			return fmt.Errorf("neboloop: token exchange failed: %w", err)
		}
		p.mqttUsername = username
		p.mqttPassword = password
	}

	// Convert broker URL to autopaho format (tcp:// → mqtt://)
	serverURL, err := brokerToURL(p.broker)
	if err != nil {
		return fmt.Errorf("neboloop: invalid broker URL: %w", err)
	}

	// Build autopaho config
	cfg := autopaho.ClientConfig{
		ServerUrls:                    []*url.URL{serverURL},
		KeepAlive:                     30,
		CleanStartOnInitialConnection: true,
		ConnectUsername:                p.mqttUsername,
		ConnectPassword:               []byte(p.mqttPassword),
		ConnectTimeout:                10 * time.Second,

		// Exponential backoff: 1s initial, max 60s per spec
		ReconnectBackoff: autopaho.NewExponentialBackoff(
			1*time.Second,  // minDelay
			60*time.Second, // maxDelay (spec says max 60s)
			2*time.Second,  // initialMaxDelay
			2.0,            // factor
		),

		// OnConnectionUp runs the mandatory on-connect sequence (spec §4)
		OnConnectionUp: func(cm *autopaho.ConnectionManager, connack *paho.Connack) {
			p.mu.Lock()
			p.connected = true
			p.mu.Unlock()

			fmt.Printf("[Comm:neboloop] Connected to %s\n", p.broker)
			p.onConnect(cm)
		},

		// OnConnectionDown handles connection loss; return false to stop reconnecting
		OnConnectionDown: func() bool {
			p.mu.Lock()
			p.connected = false
			dead := p.authDead
			p.mu.Unlock()

			if dead {
				fmt.Printf("[Comm:neboloop] Auth revoked, stopping reconnect\n")
				return false
			}

			fmt.Printf("[Comm:neboloop] Connection lost, will reconnect\n")
			return true
		},

		// OnConnectError handles connect failures; check for auth errors (0x86)
		OnConnectError: func(err error) {
			var connackErr *autopaho.ConnackError
			if errors.As(err, &connackErr) && connackErr.ReasonCode == 0x86 {
				fmt.Printf("[Comm:neboloop] Auth rejected (0x86): credentials revoked\n")
				p.mu.Lock()
				p.authDead = true
				p.mu.Unlock()
				return
			}
			fmt.Printf("[Comm:neboloop] Connect error: %v\n", err)
		},

		ClientConfig: paho.ClientConfig{
			ClientID: fmt.Sprintf("nebo-%s", p.agentID),
			OnServerDisconnect: func(d *paho.Disconnect) {
				if d.ReasonCode == 0x8B {
					fmt.Printf("[Comm:neboloop] Force-disconnected by server (0x8B)\n")
				} else if d.ReasonCode == 0x86 {
					fmt.Printf("[Comm:neboloop] Auth rejected (0x86): credentials revoked\n")
					p.mu.Lock()
					p.authDead = true
					p.mu.Unlock()
				} else {
					fmt.Printf("[Comm:neboloop] Server disconnect: reason=0x%02X\n", d.ReasonCode)
				}
			},
			OnPublishReceived: []func(paho.PublishReceived) (bool, error){
				func(pr paho.PublishReceived) (bool, error) {
					p.onMessage(pr.Packet)
					return true, nil
				},
			},
		},
	}

	// Will message: offline status on unexpected disconnect (spec §2.3)
	if p.botID != "" {
		willTopic := fmt.Sprintf("neboloop/bot/%s/status", p.botID)
		willPayload, _ := json.Marshal(map[string]string{
			"status": "offline",
			"reason": "unexpected",
		})
		cfg.WillMessage = &paho.WillMessage{
			Topic:   willTopic,
			Payload: willPayload,
			QoS:     1,
			Retain:  true,
		}
	}

	connCtx, cancel := context.WithCancel(context.Background())
	p.cancelConn = cancel

	cm, err := autopaho.NewConnection(connCtx, cfg)
	if err != nil {
		cancel()
		return fmt.Errorf("neboloop: failed to create connection: %w", err)
	}
	p.cm = cm

	// Wait for initial connection (with caller's context timeout)
	if err := cm.AwaitConnection(ctx); err != nil {
		cancel()
		return fmt.Errorf("neboloop: initial connection failed: %w", err)
	}

	return nil
}

// onConnect runs the mandatory on-connect sequence per spec §4.
// Called from OnConnectionUp — must not block long.
func (p *Plugin) onConnect(cm *autopaho.ConnectionManager) {
	p.mu.RLock()
	botID := p.botID
	card := p.card
	topics := make(map[string]bool, len(p.topics))
	for t := range p.topics {
		topics[t] = true
	}
	p.mu.RUnlock()

	if botID == "" {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Step 1: Subscribe to tasks topic (mandatory)
	tasksTopic := fmt.Sprintf("neboloop/bot/%s/tasks", botID)
	p.subscribeOne(ctx, cm, tasksTopic)

	// Step 2: Subscribe to inbox (mandatory)
	inboxTopic := fmt.Sprintf("neboloop/bot/%s/inbox", botID)
	p.subscribeOne(ctx, cm, inboxTopic)

	// Also subscribe to results (for tasks we submitted to other bots)
	resultsTopic := fmt.Sprintf("neboloop/bot/%s/results", botID)
	p.subscribeOne(ctx, cm, resultsTopic)

	// Step 3: Publish online status (retained, mandatory)
	statusTopic := fmt.Sprintf("neboloop/bot/%s/status", botID)
	statusPayload, _ := json.Marshal(map[string]string{
		"status":  "online",
		"version": "1.0.0",
	})
	p.publishRetained(ctx, cm, statusTopic, statusPayload)

	// Step 4: Publish capabilities (retained, optional but recommended)
	if card != nil {
		// Capabilities as plain JSON array per spec §4
		var caps []string
		for _, skill := range card.Skills {
			caps = append(caps, skill.ID)
		}
		if len(caps) > 0 {
			capsTopic := fmt.Sprintf("neboloop/bot/%s/capabilities", botID)
			capsPayload, _ := json.Marshal(caps)
			p.publishRetained(ctx, cm, capsTopic, capsPayload)
		}

		// Re-publish Agent Card (retained)
		cardTopic := fmt.Sprintf("neboloop/bot/%s/card", botID)
		cardPayload, _ := json.Marshal(card)
		p.publishRetained(ctx, cm, cardTopic, cardPayload)
	}

	// Step 5: Re-subscribe to any loop channels or custom topics
	for topic := range topics {
		// Skip topics we already subscribed to above
		if topic == tasksTopic || topic == inboxTopic || topic == resultsTopic {
			continue
		}
		p.subscribeOne(ctx, cm, topic)
	}

	// Step 6: Subscribe to system announcements
	p.subscribeOne(ctx, cm, "neboloop/system/announcements")
}

// subscribeOne subscribes to a single topic with QoS 1.
func (p *Plugin) subscribeOne(ctx context.Context, cm *autopaho.ConnectionManager, topic string) {
	_, err := cm.Subscribe(ctx, &paho.Subscribe{
		Subscriptions: []paho.SubscribeOptions{
			{Topic: topic, QoS: 1},
		},
	})
	if err != nil {
		fmt.Printf("[Comm:neboloop] Subscribe failed for %s: %v\n", topic, err)
	}
}

// publishRetained publishes a retained QoS 1 message.
func (p *Plugin) publishRetained(ctx context.Context, cm *autopaho.ConnectionManager, topic string, payload []byte) {
	_, err := cm.Publish(ctx, &paho.Publish{
		Topic:   topic,
		Payload: payload,
		QoS:     1,
		Retain:  true,
	})
	if err != nil {
		fmt.Printf("[Comm:neboloop] Publish failed for %s: %v\n", topic, err)
	}
}

// Disconnect performs graceful shutdown per spec §13.
func (p *Plugin) Disconnect(ctx context.Context) error {
	p.mu.Lock()
	if !p.connected || p.cm == nil {
		p.mu.Unlock()
		return nil
	}
	cm := p.cm
	botID := p.botID
	p.mu.Unlock()

	// Step 1: Publish offline status
	if botID != "" {
		statusTopic := fmt.Sprintf("neboloop/bot/%s/status", botID)
		payload, _ := json.Marshal(map[string]string{
			"status": "offline",
		})
		_, _ = cm.Publish(ctx, &paho.Publish{
			Topic:   statusTopic,
			Payload: payload,
			QoS:     1,
			Retain:  true,
		})

		// Step 3: Unsubscribe from all bot topics
		topics := []string{
			fmt.Sprintf("neboloop/bot/%s/tasks", botID),
			fmt.Sprintf("neboloop/bot/%s/inbox", botID),
			fmt.Sprintf("neboloop/bot/%s/results", botID),
			"neboloop/system/announcements",
		}

		// Also unsubscribe from any custom topics
		p.mu.RLock()
		for t := range p.topics {
			topics = append(topics, t)
		}
		p.mu.RUnlock()

		if len(topics) > 0 {
			_, _ = cm.Unsubscribe(ctx, &paho.Unsubscribe{
				Topics: topics,
			})
		}
	}

	// Step 4: Disconnect MQTT cleanly
	if p.cancelConn != nil {
		p.cancelConn()
	}

	// Wait for clean shutdown
	select {
	case <-cm.Done():
	case <-ctx.Done():
	}

	p.mu.Lock()
	p.connected = false
	p.cm = nil
	p.topics = make(map[string]bool)
	p.mu.Unlock()

	fmt.Printf("[Comm:neboloop] Disconnected\n")
	return nil
}

func (p *Plugin) IsConnected() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.connected
}

// Send publishes a CommMessage to the appropriate MQTT topic.
// Task results/status go to OWN /results topic with MQTT v5 Correlation Data.
// Standard messages go to msg.Topic or the recipient's /inbox.
func (p *Plugin) Send(ctx context.Context, msg comm.CommMessage) error {
	p.mu.RLock()
	if !p.connected || p.cm == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	cm := p.cm
	botID := p.botID
	p.mu.RUnlock()

	var topic string
	var payload []byte
	var props *paho.PublishProperties
	var err error

	switch msg.Type {
	case comm.CommTypeTaskResult, comm.CommTypeTaskStatus:
		// Publish results to OWN /results topic (spec §5.3)
		if botID == "" {
			return fmt.Errorf("neboloop: no bot_id configured for results")
		}
		topic = fmt.Sprintf("neboloop/bot/%s/results", botID)

		// Set MQTT v5 Correlation Data property (spec §5.4)
		if msg.CorrelationID != "" {
			props = &paho.PublishProperties{
				CorrelationData: []byte(msg.CorrelationID),
			}
		}

		// Marshal in NeboLoop's A2A result format
		result := a2aResultMessage{
			TaskID:        msg.TaskID,
			CorrelationID: msg.CorrelationID,
			Status:        string(msg.TaskStatus),
		}

		// Include error field for failed tasks (spec appendix B)
		if msg.TaskStatus == comm.TaskStatusFailed && msg.Error != "" {
			result.Error = msg.Error
		}

		for _, art := range msg.Artifacts {
			var parts []a2aArtifactPart
			for _, pt := range art.Parts {
				parts = append(parts, a2aArtifactPart{
					Type: pt.Type,
					Text: pt.Text,
					Data: pt.Data,
				})
			}
			result.Artifacts = append(result.Artifacts, a2aArtifact{Parts: parts})
		}
		payload, err = json.Marshal(result)

	default:
		// Standard message routing
		topic = msg.Topic
		if topic == "" && msg.To != "" {
			topic = fmt.Sprintf("neboloop/bot/%s/inbox", msg.To)
		}
		if topic == "" {
			return fmt.Errorf("neboloop: message has no topic and no recipient")
		}
		payload, err = json.Marshal(msg)
	}

	if err != nil {
		return fmt.Errorf("neboloop: marshal error: %w", err)
	}

	pub := &paho.Publish{
		Topic:      topic,
		Payload:    payload,
		QoS:        1,
		Properties: props,
	}
	if _, err := cm.Publish(ctx, pub); err != nil {
		return fmt.Errorf("neboloop: publish failed: %w", err)
	}

	return nil
}

// Subscribe subscribes to an MQTT topic and tracks it for auto-resubscribe.
func (p *Plugin) Subscribe(ctx context.Context, topic string) error {
	p.mu.Lock()
	if !p.connected || p.cm == nil {
		p.mu.Unlock()
		return fmt.Errorf("neboloop: not connected")
	}
	p.topics[topic] = true
	cm := p.cm
	p.mu.Unlock()

	_, err := cm.Subscribe(ctx, &paho.Subscribe{
		Subscriptions: []paho.SubscribeOptions{
			{Topic: topic, QoS: 1},
		},
	})
	if err != nil {
		return fmt.Errorf("neboloop: subscribe failed: %w", err)
	}

	return nil
}

// Unsubscribe unsubscribes from an MQTT topic.
func (p *Plugin) Unsubscribe(ctx context.Context, topic string) error {
	p.mu.Lock()
	if !p.connected || p.cm == nil {
		p.mu.Unlock()
		return fmt.Errorf("neboloop: not connected")
	}
	delete(p.topics, topic)
	cm := p.cm
	p.mu.Unlock()

	_, err := cm.Unsubscribe(ctx, &paho.Unsubscribe{
		Topics: []string{topic},
	})
	if err != nil {
		return fmt.Errorf("neboloop: unsubscribe failed: %w", err)
	}

	return nil
}

// Register announces this agent to the NeboLoop network.
// Stores the Agent Card for re-publish on reconnect. The actual publishing
// happens in onConnect() which runs on every (re)connection.
func (p *Plugin) Register(ctx context.Context, agentID string, card *comm.AgentCard) error {
	p.mu.Lock()
	p.agentID = agentID
	p.card = card
	botID := p.botID
	cm := p.cm
	connected := p.connected
	p.mu.Unlock()

	if botID == "" || !connected || cm == nil {
		fmt.Printf("[Comm:neboloop] Registered agent %s (no bot_id or not connected, skipping)\n", agentID)
		return nil
	}

	// Publish card, capabilities, and status immediately
	if card != nil {
		// Publish Agent Card (retained)
		cardTopic := fmt.Sprintf("neboloop/bot/%s/card", botID)
		cardPayload, _ := json.Marshal(card)
		p.publishRetained(ctx, cm, cardTopic, cardPayload)

		// Publish capabilities as plain JSON array (spec §4)
		var caps []string
		for _, skill := range card.Skills {
			caps = append(caps, skill.ID)
		}
		if len(caps) > 0 {
			capsTopic := fmt.Sprintf("neboloop/bot/%s/capabilities", botID)
			capsPayload, _ := json.Marshal(caps)
			p.publishRetained(ctx, cm, capsTopic, capsPayload)
		}
	}

	fmt.Printf("[Comm:neboloop] Registered agent %s (bot: %s)\n", agentID, botID)
	return nil
}

// Deregister removes this agent from the NeboLoop network.
// Clears retained status, card, and capabilities. Unsubscribes from all bot topics.
func (p *Plugin) Deregister(ctx context.Context) error {
	p.mu.RLock()
	if !p.connected || p.cm == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	cm := p.cm
	botID := p.botID
	p.mu.RUnlock()

	if botID == "" {
		return nil
	}

	// Clear retained messages by publishing empty retained payloads
	for _, suffix := range []string{"status", "card", "capabilities"} {
		topic := fmt.Sprintf("neboloop/bot/%s/%s", botID, suffix)
		_, _ = cm.Publish(ctx, &paho.Publish{
			Topic:  topic,
			QoS:    1,
			Retain: true,
		})
	}

	// Unsubscribe from all bot topics
	topics := []string{
		fmt.Sprintf("neboloop/bot/%s/inbox", botID),
		fmt.Sprintf("neboloop/bot/%s/tasks", botID),
		fmt.Sprintf("neboloop/bot/%s/results", botID),
	}
	_, _ = cm.Unsubscribe(ctx, &paho.Unsubscribe{
		Topics: topics,
	})

	fmt.Printf("[Comm:neboloop] Deregistered bot %s\n", botID)
	return nil
}

// SetMessageHandler sets the callback for incoming messages.
func (p *Plugin) SetMessageHandler(handler func(msg comm.CommMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.handler = handler
}

// onMessage is the MQTT v5 message callback. It dispatches to topic-specific
// handlers based on the MQTT topic suffix.
func (p *Plugin) onMessage(pub *paho.Publish) {
	p.mu.RLock()
	handler := p.handler
	p.mu.RUnlock()

	if handler == nil {
		return
	}

	topic := pub.Topic

	if strings.HasSuffix(topic, "/tasks") {
		p.handleTaskMessage(handler, topic, pub.Payload)
		return
	}
	if strings.HasSuffix(topic, "/results") {
		p.handleResultMessage(handler, topic, pub.Payload)
		return
	}

	// Standard message (inbox, loop channels, system announcements, etc.)
	p.handleStandardMessage(handler, topic, pub.Payload)
}

// handleStandardMessage parses a standard CommMessage from an MQTT payload.
func (p *Plugin) handleStandardMessage(handler func(comm.CommMessage), topic string, payload []byte) {
	var commMsg comm.CommMessage
	if err := json.Unmarshal(payload, &commMsg); err != nil {
		fmt.Printf("[Comm:neboloop] Invalid message on %s: %v\n", topic, err)
		return
	}
	if commMsg.Topic == "" {
		commMsg.Topic = topic
	}
	handler(commMsg)
}

// a2aTaskMessage is the NeboLoop A2A task format on MQTT.
type a2aTaskMessage struct {
	TaskID        string `json:"task_id"`
	CorrelationID string `json:"correlation_id"`
	From          string `json:"from"`
	Status        string `json:"status"`
	Message       struct {
		Role  string `json:"role"`
		Parts []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"parts"`
	} `json:"message"`
}

// handleTaskMessage parses an A2A task submission from the /tasks topic.
// Distinguishes between new tasks (status=submitted) and cancellations (status=canceled).
func (p *Plugin) handleTaskMessage(handler func(comm.CommMessage), topic string, payload []byte) {
	var task a2aTaskMessage
	if err := json.Unmarshal(payload, &task); err != nil {
		fmt.Printf("[Comm:neboloop] Invalid task on %s: %v\n", topic, err)
		return
	}

	// Check for cancellation (spec §5.2)
	if task.Status == "canceled" {
		commMsg := comm.CommMessage{
			ID:            task.TaskID,
			From:          task.From,
			Topic:         topic,
			Type:          comm.CommTypeTask,
			TaskID:        task.TaskID,
			CorrelationID: task.CorrelationID,
			TaskStatus:    comm.TaskStatusCanceled,
			Timestamp:     time.Now().Unix(),
		}
		handler(commMsg)
		return
	}

	// Extract text content from message parts
	var content strings.Builder
	for _, part := range task.Message.Parts {
		if part.Type == "text" {
			if content.Len() > 0 {
				content.WriteString("\n")
			}
			content.WriteString(part.Text)
		}
	}

	commMsg := comm.CommMessage{
		ID:            task.TaskID,
		From:          task.From,
		Topic:         topic,
		Type:          comm.CommTypeTask,
		Content:       content.String(),
		TaskID:        task.TaskID,
		CorrelationID: task.CorrelationID,
		TaskStatus:    comm.TaskStatus(task.Status),
		Timestamp:     time.Now().Unix(),
	}

	handler(commMsg)
}

// a2aArtifactPart is a single part within an artifact.
type a2aArtifactPart struct {
	Type string `json:"type"`
	Text string `json:"text"`
	Data []byte `json:"data,omitempty"`
}

// a2aArtifact is a structured result from a completed A2A task.
type a2aArtifact struct {
	Parts []a2aArtifactPart `json:"parts"`
}

// a2aResultMessage is the NeboLoop A2A task result format on MQTT.
type a2aResultMessage struct {
	TaskID        string        `json:"task_id"`
	CorrelationID string        `json:"correlation_id"`
	Status        string        `json:"status"`
	Artifacts     []a2aArtifact `json:"artifacts,omitempty"`
	Error         string        `json:"error,omitempty"`
}

// handleResultMessage parses an A2A task result from the /results topic.
func (p *Plugin) handleResultMessage(handler func(comm.CommMessage), topic string, payload []byte) {
	var result a2aResultMessage
	if err := json.Unmarshal(payload, &result); err != nil {
		fmt.Printf("[Comm:neboloop] Invalid result on %s: %v\n", topic, err)
		return
	}

	// Convert artifacts
	var artifacts []comm.TaskArtifact
	var contentText strings.Builder
	for _, art := range result.Artifacts {
		var parts []comm.ArtifactPart
		for _, pt := range art.Parts {
			parts = append(parts, comm.ArtifactPart{
				Type: pt.Type,
				Text: pt.Text,
				Data: pt.Data,
			})
			if pt.Type == "text" {
				if contentText.Len() > 0 {
					contentText.WriteString("\n")
				}
				contentText.WriteString(pt.Text)
			}
		}
		artifacts = append(artifacts, comm.TaskArtifact{Parts: parts})
	}

	commMsg := comm.CommMessage{
		ID:            result.TaskID,
		Topic:         topic,
		Type:          comm.CommTypeTaskResult,
		Content:       contentText.String(),
		TaskID:        result.TaskID,
		CorrelationID: result.CorrelationID,
		TaskStatus:    comm.TaskStatus(result.Status),
		Artifacts:     artifacts,
		Error:         result.Error,
		Timestamp:     time.Now().Unix(),
	}

	handler(commMsg)
}

// brokerToURL converts a broker address to a *url.URL for autopaho.
// Handles scheme conversion: tcp:// → mqtt://, ssl:// → mqtts://, ws:// stays ws://
func brokerToURL(broker string) (*url.URL, error) {
	// If no scheme, assume mqtt://
	if !strings.Contains(broker, "://") {
		broker = "mqtt://" + broker
	}

	u, err := url.Parse(broker)
	if err != nil {
		return nil, err
	}

	// Convert scheme for autopaho compatibility
	switch u.Scheme {
	case "tcp":
		u.Scheme = "mqtt"
	case "ssl", "tls":
		u.Scheme = "mqtts"
	case "mqtt", "mqtts", "ws", "wss":
		// Already valid
	default:
		return nil, fmt.Errorf("unsupported scheme: %s", u.Scheme)
	}

	return u, nil
}

// exchangeToken calls the NeboLoop REST API to exchange a connection token
// for MQTT credentials.
func exchangeToken(ctx context.Context, apiServer, token string) (username, password string, err error) {
	reqBody, _ := json.Marshal(tokenExchangeRequest{Token: token})

	apiURL := apiServer + "/api/v1/bots/exchange-token"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, apiURL, bytes.NewReader(reqBody))
	if err != nil {
		return "", "", err
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		return "", "", fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return "", "", fmt.Errorf("exchange returned %d: %s", resp.StatusCode, string(body))
	}

	var result tokenExchangeResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return "", "", fmt.Errorf("decode response: %w", err)
	}

	if result.MQTTUsername == "" || result.MQTTPassword == "" {
		return "", "", fmt.Errorf("empty credentials in response")
	}

	return result.MQTTUsername, result.MQTTPassword, nil
}

// ---------------------------------------------------------------------------
// Configurable interface (iPhone Settings.bundle model)
// ---------------------------------------------------------------------------

// Manifest returns the settings schema for the NeboLoop plugin.
// The UI renders this dynamically — no hardcoded forms needed.
func (p *Plugin) Manifest() plugin.SettingsManifest {
	return plugin.SettingsManifest{
		Groups: []plugin.SettingsGroup{
			{
				Title:       "Connection",
				Description: "MQTT broker connection settings",
				Fields: []plugin.SettingsField{
					{
						Key:         "broker",
						Title:       "MQTT Broker",
						Type:        plugin.FieldURL,
						Required:    true,
						Placeholder: "tcp://192.168.86.31:1883",
						Description: "MQTT broker address (tcp:// or ssl://)",
					},
					{
						Key:         "api_server",
						Title:       "API Server",
						Type:        plugin.FieldURL,
						Required:    true,
						Placeholder: "http://192.168.86.31:8888",
						Description: "NeboLoop REST API base URL",
					},
				},
			},
			{
				Title:       "Authentication",
				Description: "Bot credentials for NeboLoop network",
				Fields: []plugin.SettingsField{
					{
						Key:         "connection_token",
						Title:       "Connection Token",
						Type:        plugin.FieldPassword,
						Secret:      true,
						Description: "One-time token from NeboLoop (exchanged for MQTT credentials)",
					},
					{
						Key:         "bot_id",
						Title:       "Bot ID",
						Type:        plugin.FieldText,
						Description: "Bot UUID assigned by NeboLoop",
					},
					{
						Key:         "mqtt_username",
						Title:       "MQTT Username",
						Type:        plugin.FieldText,
						Description: "Direct MQTT username (alternative to token exchange)",
					},
					{
						Key:         "mqtt_password",
						Title:       "MQTT Password",
						Type:        plugin.FieldPassword,
						Secret:      true,
						Description: "Direct MQTT password",
					},
				},
			},
		},
	}
}

// OnSettingsChanged applies new settings without requiring a restart.
// If the plugin is connected, it disconnects and reconnects with new values.
func (p *Plugin) OnSettingsChanged(settings map[string]string) error {
	p.mu.RLock()
	wasConnected := p.connected
	p.mu.RUnlock()

	if wasConnected {
		// Disconnect with old config, reconnect with new
		ctx := context.Background()
		if err := p.Disconnect(ctx); err != nil {
			fmt.Printf("[neboloop] Warning: disconnect during settings change: %v\n", err)
		}
		return p.Connect(ctx, settings)
	}

	// Not connected — just store the config for next Connect() call
	return nil
}

// Compile-time interface check
var _ plugin.Configurable = (*Plugin)(nil)
