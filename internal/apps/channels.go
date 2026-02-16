package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/eclipse/paho.golang/autopaho"
	"github.com/eclipse/paho.golang/paho"
)

// InboundMessage represents a message received from a channel.
type InboundMessage struct {
	ChannelType string `json:"channel_type"`
	ChannelID   string `json:"channel_id"`
	MessageID   string `json:"message_id"`
	Text        string `json:"text"`
	SenderID    string `json:"sender_id"`
	SenderName  string `json:"sender_name"`
	ReplyToID   string `json:"reply_to_id,omitempty"`
	ThreadID    string `json:"thread_id,omitempty"`
}

// OutboundMessage represents a message to send to a channel.
type OutboundMessage struct {
	ChannelID string `json:"channel_id"`
	Text      string `json:"text"`
	ReplyToID string `json:"reply_to_id,omitempty"`
	ThreadID  string `json:"thread_id,omitempty"`
}

// ChannelBridgeConfig holds MQTT connection settings for the channel bridge.
// These are the same credentials used by the NeboLoop comm plugin and install listener.
type ChannelBridgeConfig struct {
	Broker       string // MQTT broker address (e.g., "tcp://localhost:1883")
	BotID        string // Bot UUID assigned by NeboLoop
	MQTTUsername string // MQTT username
	MQTTPassword string // MQTT password
	AgentName    string // Agent name for outbound envelope sender
	AgentRole    string // Agent role for outbound envelope sender
}

// ChannelBridge subscribes to NeboLoop MQTT web chat messages and
// publishes agent responses back to the outbound topic.
//
// Inbound topic:  neboloop/bot/{botID}/chat/in   (user → bot)
// Outbound topic: neboloop/bot/{botID}/chat/out  (bot → user)
type ChannelBridge struct {
	config    ChannelBridgeConfig
	cm        *autopaho.ConnectionManager
	handler   func(InboundMessage)
	connected bool
	cancel    context.CancelFunc
	mu        sync.RWMutex
}

// chatInboundMessage is the JSON payload published by NeboLoop web chat (user → bot).
type chatInboundMessage struct {
	Text      string `json:"text"`
	Sender    string `json:"sender"`     // "owner" or "bot"
	Timestamp string `json:"timestamp"`  // ISO8601
	MessageID string `json:"message_id"` // UUID
}

// chatOutboundMessage is the JSON payload Nebo publishes to NeboLoop web chat (bot → user).
type chatOutboundMessage struct {
	Text      string `json:"text"`
	Sender    string `json:"sender"`    // always "bot"
	Timestamp string `json:"timestamp"` // ISO8601
	MessageID string `json:"message_id,omitempty"`
}

// NewChannelBridge creates a new channel bridge instance.
func NewChannelBridge() *ChannelBridge {
	return &ChannelBridge{}
}

// Start connects to the NeboLoop MQTT broker and subscribes to channel inbound messages.
// Blocks until the initial connection is established or the context is cancelled.
func (cb *ChannelBridge) Start(ctx context.Context, config ChannelBridgeConfig) error {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	if cb.connected {
		return fmt.Errorf("channel bridge already running")
	}

	if config.BotID == "" {
		return fmt.Errorf("channel bridge: bot_id is required")
	}
	if config.Broker == "" {
		return fmt.Errorf("channel bridge: broker is required")
	}

	cb.config = config

	serverURL, err := brokerToInstallURL(config.Broker)
	if err != nil {
		return fmt.Errorf("channel bridge: invalid broker URL: %w", err)
	}

	connCtx, cancel := context.WithCancel(context.Background())
	cb.cancel = cancel

	cfg := autopaho.ClientConfig{
		ServerUrls:                    []*url.URL{serverURL},
		KeepAlive:                     30,
		CleanStartOnInitialConnection: false, // Persist subscriptions + queue QoS 1 messages while offline
		ConnectUsername:                config.MQTTUsername,
		ConnectPassword:               []byte(config.MQTTPassword),
		ConnectTimeout:                10 * time.Second,

		ReconnectBackoff: autopaho.NewExponentialBackoff(
			1*time.Second,
			60*time.Second,
			2*time.Second,
			2.0,
		),

		OnConnectionUp: func(cm *autopaho.ConnectionManager, connack *paho.Connack) {
			cb.mu.Lock()
			cb.connected = true
			cb.mu.Unlock()

			fmt.Printf("[apps:channels] Connected to MQTT broker\n")
			cb.onConnect(cm)
		},

		OnConnectionDown: func() bool {
			cb.mu.Lock()
			cb.connected = false
			cb.mu.Unlock()
			fmt.Printf("[apps:channels] Connection lost, will reconnect\n")
			return true
		},

		OnConnectError: func(err error) {
			fmt.Printf("[apps:channels] Connect error: %v\n", err)
		},

		ClientConfig: paho.ClientConfig{
			ClientID: fmt.Sprintf("nebo-channels-%s", config.BotID),
			OnPublishReceived: []func(paho.PublishReceived) (bool, error){
				func(pr paho.PublishReceived) (bool, error) {
					cb.onMessage(pr.Packet)
					return true, nil
				},
			},
		},
	}

	cm, err := autopaho.NewConnection(connCtx, cfg)
	if err != nil {
		cancel()
		return fmt.Errorf("channel bridge: failed to create connection: %w", err)
	}
	cb.cm = cm

	if err := cm.AwaitConnection(ctx); err != nil {
		cancel()
		return fmt.Errorf("channel bridge: initial connection failed: %w", err)
	}

	return nil
}

// onConnect subscribes to channel inbound topics after each (re)connection.
// Subscribes to both legacy topic and per-channel wildcard for backward compatibility.
func (cb *ChannelBridge) onConnect(cm *autopaho.ConnectionManager) {
	cb.mu.RLock()
	botID := cb.config.BotID
	cb.mu.RUnlock()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Legacy topic: neboloop/bot/{botID}/chat/in
	legacyTopic := fmt.Sprintf("neboloop/bot/%s/chat/in", botID)
	// Per-channel wildcard: neboloop/bot/{botID}/channels/+/inbound
	channelTopic := fmt.Sprintf("neboloop/bot/%s/channels/+/inbound", botID)

	_, err := cm.Subscribe(ctx, &paho.Subscribe{
		Subscriptions: []paho.SubscribeOptions{
			{Topic: legacyTopic, QoS: 1},
			{Topic: channelTopic, QoS: 1},
		},
	})
	if err != nil {
		fmt.Printf("[apps:channels] Subscribe failed: %v\n", err)
		return
	}
	fmt.Printf("[apps:channels] Subscribed to %s and %s\n", legacyTopic, channelTopic)
}

// onMessage handles incoming channel messages from NeboLoop.
// Supports both v1 envelope format (per-channel topics) and legacy chat format.
func (cb *ChannelBridge) onMessage(pub *paho.Publish) {
	cb.mu.RLock()
	handler := cb.handler
	botID := cb.config.BotID
	cb.mu.RUnlock()

	if handler == nil {
		fmt.Printf("[apps:channels] No message handler set, dropping message on %s\n", pub.Topic)
		return
	}

	// Detect topic format to choose parser:
	// Per-channel: neboloop/bot/{botID}/channels/{channelType}/inbound
	// Legacy:      neboloop/bot/{botID}/chat/in
	channelType := channelTypeFromTopic(pub.Topic, botID)

	if channelType != "" {
		// Per-channel topic — try v1 envelope first
		cb.handleEnvelopeMessage(pub, handler, channelType)
	} else {
		// Legacy chat topic
		cb.handleLegacyChatMessage(pub, handler)
	}
}

// handleEnvelopeMessage parses a v1 ChannelEnvelope from a per-channel topic.
func (cb *ChannelBridge) handleEnvelopeMessage(pub *paho.Publish, handler func(InboundMessage), channelType string) {
	var env ChannelEnvelope
	if err := json.Unmarshal(pub.Payload, &env); err != nil {
		fmt.Printf("[apps:channels] Invalid envelope on %s: %v\n", pub.Topic, err)
		return
	}

	if env.Text == "" {
		return
	}

	// Echo prevention: ignore messages from this bot
	if env.Sender.BotID == cb.config.BotID {
		return
	}

	fmt.Printf("[apps:channels] Inbound %s message (sender=%s): %s\n",
		channelType, env.Sender.Name, truncateText(env.Text, 80))

	msg := InboundMessage{
		ChannelType: channelType,
		ChannelID:   env.ChannelID,
		MessageID:   env.MessageID,
		SenderID:    env.Sender.BotID,
		SenderName:  env.Sender.Name,
		Text:        env.Text,
		ReplyToID:   env.ReplyTo,
	}

	handler(msg)
}

// handleLegacyChatMessage parses the old chatInboundMessage format.
func (cb *ChannelBridge) handleLegacyChatMessage(pub *paho.Publish, handler func(InboundMessage)) {
	var raw chatInboundMessage
	if err := json.Unmarshal(pub.Payload, &raw); err != nil {
		fmt.Printf("[apps:channels] Invalid message on %s: %v\n", pub.Topic, err)
		return
	}

	if raw.Text == "" {
		return
	}

	// Ignore messages from the bot itself (echo prevention)
	if raw.Sender == "bot" {
		return
	}

	fmt.Printf("[apps:channels] Inbound chat message (sender=%s): %s\n",
		raw.Sender, truncateText(raw.Text, 80))

	msg := InboundMessage{
		ChannelType: "neboloop",
		ChannelID:   cb.config.BotID,
		MessageID:   raw.MessageID,
		SenderID:    raw.Sender,
		SenderName:  raw.Sender,
		Text:        raw.Text,
	}

	handler(msg)
}

// channelTypeFromTopic extracts the channel type from a per-channel MQTT topic.
// Returns empty string for legacy topics.
// Per-channel format: neboloop/bot/{botID}/channels/{channelType}/inbound
func channelTypeFromTopic(topic, botID string) string {
	prefix := fmt.Sprintf("neboloop/bot/%s/channels/", botID)
	if !strings.HasPrefix(topic, prefix) {
		return ""
	}
	rest := strings.TrimPrefix(topic, prefix)
	// rest should be "{channelType}/inbound"
	parts := strings.SplitN(rest, "/", 2)
	if len(parts) == 2 && parts[1] == "inbound" {
		return parts[0]
	}
	return ""
}

// SetMessageHandler sets the callback for incoming channel messages.
func (cb *ChannelBridge) SetMessageHandler(fn func(InboundMessage)) {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.handler = fn
}

// SendResponse publishes an outbound message.
// For per-channel types, publishes a v1 envelope to the channel-specific topic.
// For legacy "neboloop" type, publishes the old chat format for backward compatibility.
func (cb *ChannelBridge) SendResponse(channelType string, msg OutboundMessage) error {
	cb.mu.RLock()
	if !cb.connected || cb.cm == nil {
		cb.mu.RUnlock()
		return fmt.Errorf("channel bridge: not connected")
	}
	cm := cb.cm
	config := cb.config
	cb.mu.RUnlock()

	var topic string
	var payload []byte
	var err error

	if channelType == "" || channelType == "neboloop" {
		// Legacy chat format for NeboLoop web chat
		outbound := chatOutboundMessage{
			Text:      msg.Text,
			Sender:    "bot",
			Timestamp: time.Now().UTC().Format(time.RFC3339),
			MessageID: NewMessageID(),
		}
		payload, err = json.Marshal(outbound)
		topic = fmt.Sprintf("neboloop/bot/%s/chat/out", config.BotID)
	} else {
		// v1 envelope for per-channel topics
		env := ChannelEnvelope{
			MessageID: NewMessageID(),
			ChannelID: msg.ChannelID,
			Sender: EnvelopeSender{
				Name:  config.AgentName,
				Role:  config.AgentRole,
				BotID: config.BotID,
			},
			Text:      msg.Text,
			ReplyTo:   msg.ReplyToID,
			Timestamp: time.Now().UTC(),
		}
		payload, err = json.Marshal(env)
		topic = fmt.Sprintf("neboloop/bot/%s/channels/%s/outbound", config.BotID, channelType)
	}

	if err != nil {
		return fmt.Errorf("channel bridge: marshal error: %w", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	_, err = cm.Publish(ctx, &paho.Publish{
		Topic:   topic,
		Payload: payload,
		QoS:     1,
	})
	if err != nil {
		return fmt.Errorf("channel bridge: publish failed: %w", err)
	}

	fmt.Printf("[apps:channels] Outbound %s message (%d chars)\n", channelType, len(msg.Text))

	return nil
}

// Stop disconnects from the MQTT broker and cleans up.
func (cb *ChannelBridge) Stop() {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	if cb.cancel != nil {
		cb.cancel()
	}

	if cb.cm != nil {
		select {
		case <-cb.cm.Done():
		case <-time.After(5 * time.Second):
		}
		cb.cm = nil
	}

	cb.connected = false
	fmt.Printf("[apps:channels] Stopped\n")
}

// IsRunning returns true if the bridge is connected to the MQTT broker.
func (cb *ChannelBridge) IsRunning() bool {
	cb.mu.RLock()
	defer cb.mu.RUnlock()
	return cb.connected
}

// truncateText truncates a string for logging purposes.
func truncateText(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
