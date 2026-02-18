// Package neboloop implements a CommPlugin that connects to a NeboLoop server
// via the NeboLoop Comms SDK (WebSocket + binary framing + protobuf payloads).
package neboloop

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/neboloop/sdk"
)

// Plugin implements comm.CommPlugin for NeboLoop comms SDK transport.
type Plugin struct {
	client  *sdk.Client
	handler func(comm.CommMessage)

	agentID   string
	botID     string
	apiServer string
	gateway   string

	card *comm.AgentCard // Stored for re-publish on reconnect

	connected bool
	mu        sync.RWMutex

	// Handlers for install events and channel messages (set by agent.go)
	onInstall        func(sdk.InstallEvent)
	onChannelMessage func(sdk.ChannelMessage)
}

// New creates a new NeboLoop plugin.
func New() *Plugin {
	return &Plugin{}
}

func (p *Plugin) Name() string    { return "neboloop" }
func (p *Plugin) Version() string { return "3.0.0" }

// OnInstall registers a handler for app install events delivered via the SDK.
func (p *Plugin) OnInstall(fn func(sdk.InstallEvent)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onInstall = fn
}

// OnChannelMessage registers a handler for inbound channel messages delivered via the SDK.
func (p *Plugin) OnChannelMessage(fn func(sdk.ChannelMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onChannelMessage = fn
}

// Client returns the underlying SDK client for direct send operations (e.g. channel outbound).
func (p *Plugin) Client() *sdk.Client {
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
//   - device_id:   Optional device/session identifier
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

	token := config["token"]
	if token == "" {
		return fmt.Errorf("neboloop: 'token' (owner JWT) config is required")
	}

	client, err := sdk.Connect(ctx, sdk.Config{
		Gateway:  p.gateway,
		BotID:    p.botID,
		Token:    token,
		DeviceID: config["device_id"],
	})
	if err != nil {
		return fmt.Errorf("neboloop: %w", err)
	}

	p.client = client
	p.connected = true

	// Wire SDK handlers
	p.wireHandlers()

	fmt.Printf("[Comm:neboloop] Connected to %s\n", p.gateway)
	return nil
}

// wireHandlers connects SDK callbacks to the plugin's dispatch logic.
// Must be called with p.mu held or during init.
func (p *Plugin) wireHandlers() {
	if p.client == nil {
		return
	}

	// A2A task submissions → comm handler
	p.client.OnTask(func(task sdk.TaskSubmission) {
		p.mu.RLock()
		handler := p.handler
		p.mu.RUnlock()
		if handler == nil {
			return
		}

		handler(comm.CommMessage{
			ID:            task.CorrelationID,
			From:          task.From,
			Type:          comm.CommTypeTask,
			Content:       task.Input,
			TaskID:        task.CorrelationID,
			CorrelationID: task.CorrelationID,
			TaskStatus:    comm.TaskStatusSubmitted,
			Timestamp:     time.Now().Unix(),
		})
	})

	// A2A task results → comm handler
	p.client.OnTaskResult(func(result sdk.TaskResult) {
		p.mu.RLock()
		handler := p.handler
		p.mu.RUnlock()
		if handler == nil {
			return
		}

		handler(comm.CommMessage{
			ID:            result.CorrelationID,
			Type:          comm.CommTypeTaskResult,
			Content:       result.Output,
			TaskID:        result.CorrelationID,
			CorrelationID: result.CorrelationID,
			TaskStatus:    comm.TaskStatus(result.Status),
			Error:         result.Error,
			Timestamp:     time.Now().Unix(),
		})
	})

	// A2A direct messages → comm handler
	p.client.OnDirectMessage(func(dm sdk.DirectMessage) {
		p.mu.RLock()
		handler := p.handler
		p.mu.RUnlock()
		if handler == nil {
			return
		}

		handler(comm.CommMessage{
			From:      dm.From,
			Type:      comm.CommMessageType(dm.Type),
			Content:   dm.Content,
			Timestamp: time.Now().Unix(),
		})
	})

	// Install events → forwarded to plugin's install handler
	p.client.OnInstall(func(evt sdk.InstallEvent) {
		p.mu.RLock()
		fn := p.onInstall
		p.mu.RUnlock()
		if fn != nil {
			fn(evt)
		}
	})

	// Channel messages → forwarded to plugin's channel handler
	p.client.OnChannelMessage(func(msg sdk.ChannelMessage) {
		p.mu.RLock()
		fn := p.onChannelMessage
		p.mu.RUnlock()
		if fn != nil {
			fn(msg)
		}
	})

	// Reconnect → re-publish agent card
	p.client.OnReconnect(func() {
		p.mu.Lock()
		p.connected = true
		p.mu.Unlock()

		fmt.Printf("[Comm:neboloop] Reconnected to %s\n", p.gateway)

		// Re-register if we had a card
		p.mu.RLock()
		card := p.card
		agentID := p.agentID
		p.mu.RUnlock()

		if card != nil {
			p.Register(context.Background(), agentID, card)
		}
	})
}

// Disconnect performs graceful shutdown.
func (p *Plugin) Disconnect(_ context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if !p.connected || p.client == nil {
		return nil
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

	convID := msg.ConversationID
	if convID == "" {
		convID = msg.To
	}

	switch msg.Type {
	case comm.CommTypeTask:
		return client.SubmitTask(ctx, convID, sdk.TaskSubmission{
			From:          msg.From,
			Input:         msg.Content,
			CorrelationID: msg.CorrelationID,
		})

	case comm.CommTypeTaskResult, comm.CommTypeTaskStatus:
		return client.SendTaskResult(ctx, convID, sdk.TaskResult{
			CorrelationID: msg.CorrelationID,
			Status:        string(msg.TaskStatus),
			Output:        msg.Content,
			Error:         msg.Error,
		})

	default:
		// Standard messages → direct message
		return client.SendDirect(ctx, convID, sdk.DirectMessage{
			From:    msg.From,
			Type:    string(msg.Type),
			Content: msg.Content,
		})
	}
}

// Subscribe joins a conversation on the NeboLoop gateway.
func (p *Plugin) Subscribe(ctx context.Context, topic string) error {
	p.mu.RLock()
	if !p.connected || p.client == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	client := p.client
	p.mu.RUnlock()

	return client.Join(ctx, topic)
}

// Unsubscribe leaves a conversation on the NeboLoop gateway.
func (p *Plugin) Unsubscribe(ctx context.Context, topic string) error {
	p.mu.RLock()
	if !p.connected || p.client == nil {
		p.mu.RUnlock()
		return fmt.Errorf("neboloop: not connected")
	}
	client := p.client
	p.mu.RUnlock()

	return client.Leave(ctx, topic)
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

// --- helpers ---

// deriveGatewayURL converts an API server URL to a WebSocket gateway URL.
// http://host:port → ws://host:port/ws, https://host:port → wss://host:port/ws
func deriveGatewayURL(apiServer string) string {
	gw := strings.Replace(apiServer, "https://", "wss://", 1)
	gw = strings.Replace(gw, "http://", "ws://", 1)
	return strings.TrimRight(gw, "/") + "/ws"
}

// Compile-time interface checks
var (
	_ comm.CommPlugin       = (*Plugin)(nil)
	_ settings.Configurable = (*Plugin)(nil)
)
