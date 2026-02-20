// Package neboloop implements a CommPlugin that connects to a NeboLoop server
// via the published NeboLoop Comms SDK (WebSocket + binary framing + JSON payloads).
package neboloop

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	neboloopsdk "github.com/NeboLoop/neboloop-go-sdk"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/apps/settings"
	neboloopapi "github.com/neboloop/nebo/internal/neboloop"
)

// ChannelMessage wraps the published SDK's ChannelMessage with envelope fields
// populated from the Message header (ConversationID, MessageID).
type ChannelMessage struct {
	ChannelType    string `json:"channel_type"`
	SenderName     string `json:"sender_name"`
	Text           string `json:"text"`
	ConversationID string `json:"-"` // from Message envelope
	MessageID      string `json:"-"` // from Message envelope
}

// LoopChannelMessage represents an inbound loop channel message with metadata.
type LoopChannelMessage struct {
	ChannelID      string `json:"channel_id"`
	ChannelName    string `json:"channel_name,omitempty"`
	LoopID         string `json:"loop_id,omitempty"`
	SenderID       string `json:"sender_id,omitempty"`
	SenderName     string `json:"sender_name,omitempty"`
	Text           string `json:"text"`
	ConversationID string `json:"-"` // from Message envelope
	MessageID      string `json:"-"` // from Message envelope
}

// Plugin implements comm.CommPlugin for NeboLoop comms SDK transport.
type Plugin struct {
	client  *neboloopsdk.Client
	handler func(comm.CommMessage)

	agentID   string
	botID     string
	apiServer string
	gateway   string
	token     string

	card *comm.AgentCard // Stored for re-publish on reconnect

	connected bool
	authDead  bool // credentials rejected, stop reconnecting
	done      chan struct{}
	mu        sync.RWMutex

	// Handlers for install events and channel messages (set by agent.go)
	onInstall            func(neboloopsdk.InstallEvent)
	onChannelMessage     func(ChannelMessage)
	onLoopChannelMessage func(LoopChannelMessage)

	// Loop channel tracking: channelID → conversationID (populated from messages)
	channelConvs map[string]string
}

// New creates a new NeboLoop plugin.
func New() *Plugin {
	return &Plugin{
		channelConvs: make(map[string]string),
		done:         make(chan struct{}),
	}
}

func (p *Plugin) Name() string    { return "neboloop" }
func (p *Plugin) Version() string { return "3.0.0" }

// OnInstall registers a handler for app install events delivered via the SDK.
func (p *Plugin) OnInstall(fn func(neboloopsdk.InstallEvent)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onInstall = fn
}

// OnLoopChannelMessage registers a handler for inbound loop channel messages delivered via the SDK.
func (p *Plugin) OnLoopChannelMessage(fn func(LoopChannelMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onLoopChannelMessage = fn
}

// OnChannelMessage registers a handler for inbound channel messages delivered via the SDK.
func (p *Plugin) OnChannelMessage(fn func(ChannelMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onChannelMessage = fn
}

// Client returns the underlying SDK client for direct send operations (e.g. channel outbound).
func (p *Plugin) Client() *neboloopsdk.Client {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.client
}

// Connect establishes a WebSocket connection to the NeboLoop gateway via the SDK.
//
// Config keys:
//   - gateway:     WebSocket URL (e.g. "wss://comms.neboloop.com")
//   - api_server:  NeboLoop REST API URL (e.g. "http://localhost:8888")
//   - bot_id:      Bot UUID for addressing
//   - token:       Owner OAuth JWT for authentication
func (p *Plugin) Connect(ctx context.Context, config map[string]string) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.connected {
		return fmt.Errorf("already connected")
	}

	p.apiServer = config["api_server"]
	p.botID = config["bot_id"]

	// Gateway URL: explicit, or derived from api_server
	p.gateway = config["gateway"]
	if p.gateway == "" && p.apiServer != "" {
		p.gateway = deriveGatewayURL(p.apiServer)
	}
	if p.gateway == "" {
		return fmt.Errorf("neboloop: 'gateway' or 'api_server' config is required")
	}

	p.token = config["token"]
	if p.token == "" {
		return fmt.Errorf("neboloop: 'token' (owner JWT) config is required")
	}

	client, err := neboloopsdk.Connect(ctx, neboloopsdk.Config{
		Endpoint:    p.gateway,
		APIEndpoint: p.apiServer,
		BotID:       p.botID,
		Token:       p.token,
	}, p.handleMessage)
	if err != nil {
		return fmt.Errorf("neboloop: %w", err)
	}

	p.client = client
	p.connected = true
	p.authDead = false

	// Reset done channel for reconnect goroutine
	select {
	case <-p.done:
		p.done = make(chan struct{})
	default:
	}

	// Wire install event handler (uses SDK's chain pattern)
	client.OnInstall(func(evt neboloopsdk.InstallEvent) {
		p.mu.RLock()
		fn := p.onInstall
		p.mu.RUnlock()
		if fn != nil {
			fn(evt)
		}
	})

	// Wire loop channel message handler
	client.OnLoopMessage(func(channelID string, msg neboloopsdk.Message) {
		loopMsg := LoopChannelMessage{
			ChannelID:      channelID,
			SenderID:       msg.SenderID,
			ConversationID: msg.ConversationID,
			MessageID:      msg.MsgID,
		}

		// Try to extract text from the message content
		var content struct {
			Text        string `json:"text"`
			SenderName  string `json:"sender_name"`
			ChannelName string `json:"channel_name"`
			LoopID      string `json:"loop_id"`
		}
		if err := json.Unmarshal(msg.Content, &content); err == nil {
			loopMsg.Text = content.Text
			loopMsg.SenderName = content.SenderName
			loopMsg.ChannelName = content.ChannelName
			loopMsg.LoopID = content.LoopID
		}

		// Look up channel metadata for names
		metas := client.ChannelMetas()
		if meta, ok := metas[channelID]; ok {
			if loopMsg.ChannelName == "" {
				loopMsg.ChannelName = meta.ChannelName
			}
			if loopMsg.LoopID == "" {
				loopMsg.LoopID = meta.LoopID
			}
		}

		// Track channel→conversation mapping
		if channelID != "" && msg.ConversationID != "" {
			p.mu.Lock()
			p.channelConvs[channelID] = msg.ConversationID
			p.mu.Unlock()
		}

		p.mu.RLock()
		fn := p.onLoopChannelMessage
		p.mu.RUnlock()
		if fn != nil {
			fn(loopMsg)
		}
	})

	fmt.Printf("[Comm:neboloop] Connected to %s\n", p.gateway)
	return nil
}

// handleMessage is the single handler for all incoming messages from the published SDK.
// It dispatches A2A messages (tasks, task results, direct messages) to the comm handler,
// and channel messages to the channel handler.
func (p *Plugin) handleMessage(msg neboloopsdk.Message) {
	// Channel inbound messages arrive on "channels/inbound" or "channel" stream
	if msg.Stream == "channels/inbound" || msg.Stream == "channel" {
		p.mu.RLock()
		fn := p.onChannelMessage
		p.mu.RUnlock()
		if fn == nil {
			return
		}

		var sdkMsg neboloopsdk.ChannelMessage
		if err := json.Unmarshal(msg.Content, &sdkMsg); err != nil {
			slog.Error("unmarshal channel message", "error", err)
			return
		}

		fn(ChannelMessage{
			ChannelType:    sdkMsg.ChannelType,
			SenderName:     sdkMsg.SenderName,
			Text:           sdkMsg.Text,
			ConversationID: msg.ConversationID,
			MessageID:      msg.MsgID,
		})
		return
	}

	// A2A streams
	if msg.Stream == "a2a" || strings.HasPrefix(msg.Stream, "a2a/") {
		p.handleA2AMessage(msg)
		return
	}
}

// handleA2AMessage dispatches A2A messages (tasks, results, direct messages).
func (p *Plugin) handleA2AMessage(msg neboloopsdk.Message) {
	p.mu.RLock()
	handler := p.handler
	p.mu.RUnlock()
	if handler == nil {
		return
	}

	// Try to determine message subtype from content
	var probe struct {
		CorrelationID string          `json:"correlation_id"`
		FromBotID     string          `json:"from_bot_id"`
		Status        string          `json:"status"`
		Input         json.RawMessage `json:"input"`
		Output        json.RawMessage `json:"output"`
		Error         string          `json:"error"`
		Text          string          `json:"text"`
	}
	if err := json.Unmarshal(msg.Content, &probe); err != nil {
		slog.Error("unmarshal a2a message", "error", err)
		return
	}

	// Task result (has status field)
	if probe.Status != "" {
		output := string(probe.Output)
		handler(comm.CommMessage{
			ID:            probe.CorrelationID,
			Type:          comm.CommTypeTaskResult,
			Content:       output,
			TaskID:        probe.CorrelationID,
			CorrelationID: probe.CorrelationID,
			TaskStatus:    comm.TaskStatus(probe.Status),
			Error:         probe.Error,
			Timestamp:     time.Now().Unix(),
		})
		return
	}

	// Task submission (has input field)
	if probe.Input != nil {
		input := string(probe.Input)
		handler(comm.CommMessage{
			ID:            probe.CorrelationID,
			From:          probe.FromBotID,
			Type:          comm.CommTypeTask,
			Content:       input,
			TaskID:        probe.CorrelationID,
			CorrelationID: probe.CorrelationID,
			TaskStatus:    comm.TaskStatusSubmitted,
			Timestamp:     time.Now().Unix(),
		})
		return
	}

	// Direct message (fallback)
	handler(comm.CommMessage{
		From:      msg.SenderID,
		Type:      comm.CommTypeMessage,
		Content:   probe.Text,
		Timestamp: time.Now().Unix(),
	})
}

// Disconnect performs graceful shutdown.
func (p *Plugin) Disconnect(_ context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if !p.connected || p.client == nil {
		return nil
	}

	// Signal reconnect goroutine to stop
	select {
	case <-p.done:
	default:
		close(p.done)
	}

	err := p.client.Close()
	p.connected = false
	p.client = nil

	fmt.Printf("[Comm:neboloop] Disconnected\n")
	return err
}

func (p *Plugin) IsConnected() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.connected
}

// Send publishes a CommMessage via the SDK.
func (p *Plugin) Send(ctx context.Context, msg comm.CommMessage) error {
	p.mu.RLock()
	if !p.connected || p.client == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	client := p.client
	p.mu.RUnlock()

	convIDStr := msg.ConversationID
	if convIDStr == "" {
		convIDStr = msg.To
	}

	convID, err := uuid.Parse(convIDStr)
	if err != nil {
		return fmt.Errorf("neboloop: invalid conversation ID %q: %w", convIDStr, err)
	}

	switch msg.Type {
	case comm.CommTypeTask:
		taskInput, _ := json.Marshal(msg.Content)
		content, _ := json.Marshal(neboloopsdk.TaskSubmission{
			FromBotID:     msg.From,
			Input:         taskInput,
			CorrelationID: msg.CorrelationID,
		})
		return client.Send(ctx, convID, "a2a", content)

	case comm.CommTypeTaskResult, comm.CommTypeTaskStatus:
		taskOutput, _ := json.Marshal(msg.Content)
		content, _ := json.Marshal(neboloopsdk.TaskResult{
			CorrelationID: msg.CorrelationID,
			Status:        string(msg.TaskStatus),
			Output:        taskOutput,
			Error:         msg.Error,
		})
		return client.Send(ctx, convID, "a2a", content)

	case comm.CommTypeLoopChannel:
		// Resolve channel ID → conversation ID from tracked mapping
		channelID := msg.To
		p.mu.RLock()
		resolvedConvID, ok := p.channelConvs[channelID]
		p.mu.RUnlock()
		if ok {
			convID, err = uuid.Parse(resolvedConvID)
			if err != nil {
				return fmt.Errorf("neboloop: invalid resolved conversation ID: %w", err)
			}
		}
		loopContent, _ := json.Marshal(map[string]string{
			"channel_id": channelID,
			"text":       msg.Content,
		})
		return client.Send(ctx, convID, "channel", loopContent)

	default:
		// Standard messages → direct message
		content, _ := json.Marshal(neboloopsdk.DirectMessage{
			Text: msg.Content,
		})
		return client.Send(ctx, convID, "a2a", content)
	}
}

// Subscribe joins a conversation on the NeboLoop gateway.
func (p *Plugin) Subscribe(_ context.Context, topic string) error {
	p.mu.RLock()
	if !p.connected || p.client == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	client := p.client
	p.mu.RUnlock()

	client.JoinConversation(topic, 0)
	return nil
}

// Unsubscribe is a no-op for the published SDK (no explicit leave support needed).
func (p *Plugin) Unsubscribe(_ context.Context, _ string) error {
	return nil
}

// Register announces this agent to the NeboLoop network.
// Stores the card for re-publish on reconnect. Agent card is published via REST API.
func (p *Plugin) Register(_ context.Context, agentID string, card *comm.AgentCard) error {
	p.mu.Lock()
	p.agentID = agentID
	p.card = card
	apiServer := p.apiServer
	botID := p.botID
	p.mu.Unlock()

	if card == nil || apiServer == "" || botID == "" {
		return nil
	}

	// Publish agent card via REST API (not wire protocol)
	cardPayload, err := json.Marshal(card)
	if err != nil {
		return fmt.Errorf("marshal card: %w", err)
	}
	_ = cardPayload // TODO: POST to {apiServer}/api/v1/bots/{botID}/card

	fmt.Printf("[Comm:neboloop] Registered agent %s (bot: %s)\n", agentID, botID)
	return nil
}

// Deregister removes this agent from the NeboLoop network.
func (p *Plugin) Deregister(_ context.Context) error {
	p.mu.Lock()
	p.card = nil
	p.mu.Unlock()
	return nil
}

// SetMessageHandler sets the callback for incoming messages.
func (p *Plugin) SetMessageHandler(handler func(msg comm.CommMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.handler = handler
}

// ---------------------------------------------------------------------------
// Configurable interface
// ---------------------------------------------------------------------------

// Manifest returns the settings schema for the NeboLoop plugin.
func (p *Plugin) Manifest() settings.SettingsManifest {
	return settings.SettingsManifest{
		Groups: []settings.SettingsGroup{
			{
				Title:       "Connection",
				Description: "NeboLoop gateway connection settings",
				Fields: []settings.SettingsField{
					{
						Key:         "gateway",
						Title:       "Gateway URL",
						Type:        settings.FieldURL,
						Placeholder: "wss://comms.neboloop.com",
						Description: "WebSocket URL for the NeboLoop comms gateway",
					},
					{
						Key:         "api_server",
						Title:       "API Server",
						Type:        settings.FieldURL,
						Required:    true,
						Placeholder: "http://localhost:8888",
						Description: "NeboLoop REST API base URL",
					},
				},
			},
			{
				Title:       "Authentication",
				Description: "Owner JWT and bot identity for NeboLoop network",
				Fields: []settings.SettingsField{
					{
						Key:         "bot_id",
						Title:       "Bot ID",
						Type:        settings.FieldText,
						Description: "Bot UUID assigned by NeboLoop",
					},
					{
						Key:         "token",
						Title:       "Token",
						Type:        settings.FieldPassword,
						Secret:      true,
						Description: "Owner OAuth JWT for authentication",
					},
				},
			},
		},
	}
}

// OnSettingsChanged applies new settings without requiring a restart.
func (p *Plugin) OnSettingsChanged(newSettings map[string]string) error {
	p.mu.RLock()
	wasConnected := p.connected
	p.mu.RUnlock()

	if wasConnected {
		ctx := context.Background()
		if err := p.Disconnect(ctx); err != nil {
			fmt.Printf("[neboloop] Warning: disconnect during settings change: %v\n", err)
		}
		return p.Connect(ctx, newSettings)
	}

	return nil
}

// --- reconnect ---

// reconnect attempts to re-establish the connection with exponential backoff.
// Called when the read loop detects a disconnect.
func (p *Plugin) reconnect() {
	p.mu.RLock()
	dead := p.authDead
	gateway := p.gateway
	apiServer := p.apiServer
	botID := p.botID
	token := p.token
	p.mu.RUnlock()

	if dead {
		slog.Error("[Comm:neboloop] credentials rejected, not reconnecting")
		return
	}

	base := 100 * time.Millisecond
	cap_ := 10 * time.Second
	attempt := 0

	for {
		select {
		case <-p.done:
			return
		default:
		}

		delay := min(base*time.Duration(1<<attempt), cap_)
		jitter := time.Duration(rand.Int64N(int64(delay) / 2))
		delay = delay - delay/4 + jitter
		attempt++

		slog.Info("[Comm:neboloop] reconnecting", "attempt", attempt, "delay", delay)
		time.Sleep(delay)

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		client, err := neboloopsdk.Connect(ctx, neboloopsdk.Config{
			Endpoint:    gateway,
			APIEndpoint: apiServer,
			BotID:       botID,
			Token:       token,
		}, p.handleMessage)
		cancel()

		if err != nil {
			if strings.Contains(err.Error(), "auth failed") {
				p.mu.Lock()
				p.authDead = true
				p.mu.Unlock()
				slog.Error("[Comm:neboloop] auth dead during reconnect, giving up")
				return
			}
			slog.Warn("[Comm:neboloop] reconnect failed", "error", err, "attempt", attempt)
			continue
		}

		p.mu.Lock()
		p.client = client
		p.connected = true
		p.mu.Unlock()

		fmt.Printf("[Comm:neboloop] Reconnected to %s\n", gateway)

		// Re-wire typed handlers
		client.OnInstall(func(evt neboloopsdk.InstallEvent) {
			p.mu.RLock()
			fn := p.onInstall
			p.mu.RUnlock()
			if fn != nil {
				fn(evt)
			}
		})
		client.OnLoopMessage(func(channelID string, msg neboloopsdk.Message) {
			loopMsg := LoopChannelMessage{
				ChannelID:      channelID,
				SenderID:       msg.SenderID,
				ConversationID: msg.ConversationID,
				MessageID:      msg.MsgID,
			}
			var content struct {
				Text        string `json:"text"`
				SenderName  string `json:"sender_name"`
				ChannelName string `json:"channel_name"`
				LoopID      string `json:"loop_id"`
			}
			if err := json.Unmarshal(msg.Content, &content); err == nil {
				loopMsg.Text = content.Text
				loopMsg.SenderName = content.SenderName
				loopMsg.ChannelName = content.ChannelName
				loopMsg.LoopID = content.LoopID
			}
			metas := client.ChannelMetas()
			if meta, ok := metas[channelID]; ok {
				if loopMsg.ChannelName == "" {
					loopMsg.ChannelName = meta.ChannelName
				}
				if loopMsg.LoopID == "" {
					loopMsg.LoopID = meta.LoopID
				}
			}
			if channelID != "" && msg.ConversationID != "" {
				p.mu.Lock()
				p.channelConvs[channelID] = msg.ConversationID
				p.mu.Unlock()
			}
			p.mu.RLock()
			fn := p.onLoopChannelMessage
			p.mu.RUnlock()
			if fn != nil {
				fn(loopMsg)
			}
		})

		// Re-register if we had a card
		p.mu.RLock()
		card := p.card
		agentID := p.agentID
		p.mu.RUnlock()

		if card != nil {
			p.Register(context.Background(), agentID, card)
		}

		return
	}
}

// --- helpers ---

// deriveGatewayURL converts an API server URL to a WebSocket gateway URL.
// Only used as fallback when no explicit gateway URL is provided via config.
func deriveGatewayURL(apiServer string) string {
	gw := strings.Replace(apiServer, "https://", "wss://", 1)
	gw = strings.Replace(gw, "http://", "ws://", 1)
	return strings.TrimRight(gw, "/") + "/ws"
}

// ListLoopChannels returns all loop channels this bot belongs to.
// Prefers the in-memory map populated by JOIN responses (zero HTTP calls).
// Falls back to the REST API if no channels are cached yet.
func (p *Plugin) ListLoopChannels(ctx context.Context) ([]comm.LoopChannelInfo, error) {
	p.mu.RLock()
	client := p.client
	p.mu.RUnlock()

	// Fast path: use in-memory channel metadata from JOIN responses
	if client != nil {
		metas := client.ChannelMetas()
		if len(metas) > 0 {
			result := make([]comm.LoopChannelInfo, 0, len(metas))
			for _, meta := range metas {
				result = append(result, comm.LoopChannelInfo{
					ChannelID:   meta.ChannelID,
					ChannelName: meta.ChannelName,
					LoopID:      meta.LoopID,
				})
			}

			// Update conversation mapping from SDK's channel→conv map
			channels := client.Channels()
			for channelID, convID := range channels {
				if convID != "" {
					p.mu.Lock()
					p.channelConvs[channelID] = convID
					p.mu.Unlock()
				}
			}

			return result, nil
		}
	}

	// Slow path: REST API fallback (e.g., before JOINs are processed)
	restClient, err := p.restClient()
	if err != nil {
		return nil, err
	}

	channels, err := restClient.ListBotChannels(ctx)
	if err != nil {
		return nil, err
	}

	result := make([]comm.LoopChannelInfo, len(channels))
	for i, ch := range channels {
		result[i] = comm.LoopChannelInfo{
			ChannelID:   ch.ChannelID,
			ChannelName: ch.ChannelName,
			LoopID:      ch.LoopID,
			LoopName:    ch.LoopName,
		}
		if ch.ConversationID != "" {
			p.mu.Lock()
			p.channelConvs[ch.ChannelID] = ch.ConversationID
			p.mu.Unlock()
		}
	}

	return result, nil
}

// --- Loop query methods (Bot Query System) ---

// ListBotLoops returns all loops this bot belongs to.
func (p *Plugin) ListBotLoops(ctx context.Context) ([]neboloopapi.Loop, error) {
	c, err := p.restClient()
	if err != nil {
		return nil, err
	}
	return c.ListBotLoops(ctx)
}

// GetLoop fetches a single loop by ID.
func (p *Plugin) GetLoop(ctx context.Context, loopID string) (*neboloopapi.Loop, error) {
	c, err := p.restClient()
	if err != nil {
		return nil, err
	}
	return c.GetLoop(ctx, loopID)
}

// ListLoopMembers returns members of a loop with online presence.
func (p *Plugin) ListLoopMembers(ctx context.Context, loopID string) ([]neboloopapi.LoopMember, error) {
	c, err := p.restClient()
	if err != nil {
		return nil, err
	}
	return c.ListLoopMembers(ctx, loopID)
}

// ListChannelMembers returns members of a channel with online presence.
func (p *Plugin) ListChannelMembers(ctx context.Context, channelID string) ([]neboloopapi.ChannelMember, error) {
	c, err := p.restClient()
	if err != nil {
		return nil, err
	}
	return c.ListChannelMembers(ctx, channelID)
}

// ListChannelMessages fetches recent messages from a channel.
func (p *Plugin) ListChannelMessages(ctx context.Context, channelID string, limit int) ([]neboloopapi.ChannelMessageItem, error) {
	c, err := p.restClient()
	if err != nil {
		return nil, err
	}
	return c.ListChannelMessages(ctx, channelID, limit)
}

// restClient creates a REST client from the plugin's current credentials.
func (p *Plugin) restClient() (*neboloopapi.Client, error) {
	p.mu.RLock()
	apiServer := p.apiServer
	botID := p.botID
	token := p.token
	p.mu.RUnlock()

	if apiServer == "" || botID == "" || token == "" {
		return nil, fmt.Errorf("not connected to NeboLoop")
	}

	return neboloopapi.NewClient(map[string]string{
		"api_server": apiServer,
		"bot_id":     botID,
		"token":      token,
	})
}

// Compile-time interface checks
var (
	_ comm.CommPlugin        = (*Plugin)(nil)
	_ settings.Configurable  = (*Plugin)(nil)
	_ comm.LoopChannelLister = (*Plugin)(nil)
)
