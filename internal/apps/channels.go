package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"sync"
	"time"

	"github.com/eclipse/paho.golang/autopaho"
	"github.com/eclipse/paho.golang/paho"

	"github.com/nebolabs/nebo/internal/channels"
)

// ChannelBridgeConfig holds MQTT connection settings for the channel bridge.
// These are the same credentials used by the NeboLoop comm plugin and install listener.
type ChannelBridgeConfig struct {
	Broker       string // MQTT broker address (e.g., "tcp://localhost:1883")
	BotID        string // Bot UUID assigned by NeboLoop
	MQTTUsername string // MQTT username
	MQTTPassword string // MQTT password
}

// ChannelBridge subscribes to NeboLoop MQTT channel inbound messages and
// publishes agent responses back to the outbound topic.
//
// Inbound topic:  neboloop/bot/{botID}/channels/inbound
// Outbound topic: neboloop/bot/{botID}/channels/outbound
type ChannelBridge struct {
	config    ChannelBridgeConfig
	cm        *autopaho.ConnectionManager
	handler   func(channels.InboundMessage)
	connected bool
	cancel    context.CancelFunc
	mu        sync.RWMutex
}

// channelInboundMessage is the JSON format published by NeboLoop channel bridges.
type channelInboundMessage struct {
	ChannelType string `json:"channel_type"`
	ChannelID   string `json:"channel_id"`
	MessageID   string `json:"message_id"`
	SenderID    string `json:"sender_id"`
	SenderName  string `json:"sender_name"`
	Text        string `json:"text"`
	ReplyToID   string `json:"reply_to_id,omitempty"`
	ThreadID    string `json:"thread_id,omitempty"`
}

// channelOutboundMessage is the JSON format Nebo publishes for NeboLoop to deliver.
type channelOutboundMessage struct {
	ChannelType string `json:"channel_type"`
	ChannelID   string `json:"channel_id"`
	Text        string `json:"text"`
	ReplyToID   string `json:"reply_to_id,omitempty"`
	ThreadID    string `json:"thread_id,omitempty"`
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
		CleanStartOnInitialConnection: true,
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

// onConnect subscribes to the channels inbound topic after each (re)connection.
func (cb *ChannelBridge) onConnect(cm *autopaho.ConnectionManager) {
	cb.mu.RLock()
	botID := cb.config.BotID
	cb.mu.RUnlock()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	topic := fmt.Sprintf("neboloop/bot/%s/channels/inbound", botID)
	_, err := cm.Subscribe(ctx, &paho.Subscribe{
		Subscriptions: []paho.SubscribeOptions{
			{Topic: topic, QoS: 1},
		},
	})
	if err != nil {
		fmt.Printf("[apps:channels] Subscribe failed for %s: %v\n", topic, err)
		return
	}
	fmt.Printf("[apps:channels] Subscribed to %s\n", topic)
}

// onMessage handles incoming channel messages from NeboLoop.
func (cb *ChannelBridge) onMessage(pub *paho.Publish) {
	cb.mu.RLock()
	handler := cb.handler
	cb.mu.RUnlock()

	if handler == nil {
		fmt.Printf("[apps:channels] No message handler set, dropping message on %s\n", pub.Topic)
		return
	}

	var raw channelInboundMessage
	if err := json.Unmarshal(pub.Payload, &raw); err != nil {
		fmt.Printf("[apps:channels] Invalid message on %s: %v\n", pub.Topic, err)
		return
	}

	if raw.Text == "" {
		fmt.Printf("[apps:channels] Empty text in message from %s on %s, ignoring\n", raw.SenderName, raw.ChannelType)
		return
	}

	fmt.Printf("[apps:channels] Inbound %s message from %s (channel=%s): %s\n",
		raw.ChannelType, raw.SenderName, raw.ChannelID, truncateText(raw.Text, 80))

	msg := channels.InboundMessage{
		ChannelType: raw.ChannelType,
		ChannelID:   raw.ChannelID,
		MessageID:   raw.MessageID,
		SenderID:    raw.SenderID,
		SenderName:  raw.SenderName,
		Text:        raw.Text,
		ReplyToID:   raw.ReplyToID,
		ThreadID:    raw.ThreadID,
	}

	handler(msg)
}

// SetMessageHandler sets the callback for incoming channel messages.
func (cb *ChannelBridge) SetMessageHandler(fn func(channels.InboundMessage)) {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.handler = fn
}

// SendResponse publishes an outbound message to NeboLoop for delivery to the channel.
func (cb *ChannelBridge) SendResponse(channelType string, msg channels.OutboundMessage) error {
	cb.mu.RLock()
	if !cb.connected || cb.cm == nil {
		cb.mu.RUnlock()
		return fmt.Errorf("channel bridge: not connected")
	}
	cm := cb.cm
	botID := cb.config.BotID
	cb.mu.RUnlock()

	outbound := channelOutboundMessage{
		ChannelType: channelType,
		ChannelID:   msg.ChannelID,
		Text:        msg.Text,
		ReplyToID:   msg.ReplyToID,
		ThreadID:    msg.ThreadID,
	}

	payload, err := json.Marshal(outbound)
	if err != nil {
		return fmt.Errorf("channel bridge: marshal error: %w", err)
	}

	topic := fmt.Sprintf("neboloop/bot/%s/channels/outbound", botID)

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

	fmt.Printf("[apps:channels] Outbound %s message to channel=%s (%d chars)\n",
		channelType, msg.ChannelID, len(msg.Text))

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
