// Package neboloop implements a CommPlugin that connects to a NeboLoop server
// via the published NeboLoop Comms SDK (WebSocket + binary framing + JSON payloads).
package neboloop

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	neboloopsdk "github.com/NeboLoop/neboloop-go-sdk"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/apps/settings"
	"github.com/neboloop/nebo/internal/defaults"
	neboloopapi "github.com/neboloop/nebo/internal/neboloop"
)

// commLog is a dedicated logger that writes all comms traffic to a file
// at <data_dir>/logs/comms.log, independent of the global log level.
var commLog *slog.Logger

func init() {
	commLog = slog.Default() // fallback

	dataDir, err := defaults.DataDir()
	if err != nil {
		return
	}
	logDir := filepath.Join(dataDir, "logs")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return
	}
	f, err := os.OpenFile(filepath.Join(logDir, "comms.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	if err != nil {
		return
	}
	commLog = slog.New(slog.NewTextHandler(f, &slog.HandlerOptions{Level: slog.LevelDebug}))
}

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

// DMMessage represents an inbound direct message (stream=dm).
type DMMessage struct {
	SenderID       string
	Text           string
	ConversationID string
	MessageID      string
	IsOwner        bool // true when the sender is the bot's owner
}

// HistoryRequest represents an inbound request for recent conversation history (stream=history).
// NeboLoop sends this when the owner opens the P2P DM so the chat can catch up
// with what's been happening on the local machine.
type HistoryRequest struct {
	ConversationID string
	Limit          int // how many messages to return (default 20)
}

// HistoryMessage is a single message in a history response.
type HistoryMessage struct {
	Role      string `json:"role"`      // "user" or "assistant"
	Content   string `json:"content"`
	Timestamp int64  `json:"timestamp"` // unix seconds
}

// AccountEvent represents an inbound account-level event (stream=account).
// NeboLoop sends these when the user's billing plan changes (e.g., "plan_changed").
type AccountEvent struct {
	Type    string          `json:"type"`
	Payload json.RawMessage `json:"payload"`
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
	ownerID   string // JWT sub claim — identifies the bot owner

	card *comm.AgentCard // Stored for re-publish on reconnect

	connected    bool
	authDead     bool // credentials rejected, stop reconnecting
	reconnecting bool // guard against concurrent reconnect calls
	done         chan struct{}
	mu           sync.RWMutex

	// Handlers for install events and channel messages (set by agent.go)
	onInstall            func(neboloopsdk.InstallEvent)
	onChannelMessage     func(ChannelMessage)
	onLoopChannelMessage func(LoopChannelMessage)
	onDMMessage          func(DMMessage)
	onHistoryRequest     func(HistoryRequest) []HistoryMessage

	// Loop channel tracking: channelID → conversationID (populated from messages)
	channelConvs map[string]string

	// tokenRefresher is called on auth failure during reconnect to obtain a fresh JWT.
	// Returns the new access token or an error if refresh is not possible.
	tokenRefresher func(ctx context.Context) (string, error)

	// onAccountEvent is called when an account-level event arrives (stream=account).
	onAccountEvent func(AccountEvent)

	// onConnected is fired after a successful connect or reconnect.
	onConnected func()

	// Health tracking: lastMessageTime, lastPingSuccess
	lastMessageTime time.Time
	lastPingSuccess time.Time
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

// OnDMMessage registers a handler for inbound direct messages (stream=dm).
func (p *Plugin) OnDMMessage(fn func(DMMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onDMMessage = fn
}

// OnHistoryRequest registers a handler for history requests (stream=history).
// The handler receives the request and returns messages to send back.
func (p *Plugin) OnHistoryRequest(fn func(HistoryRequest) []HistoryMessage) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onHistoryRequest = fn
}

// SetTokenRefresher registers a callback that obtains a fresh JWT when
// the current token is rejected during reconnect. This prevents the plugin
// from permanently dying (authDead) when the access token expires.
func (p *Plugin) SetTokenRefresher(fn func(ctx context.Context) (string, error)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.tokenRefresher = fn
}

// OnAccountEvent registers a handler for account-level events (stream=account).
func (p *Plugin) OnAccountEvent(fn func(AccountEvent)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onAccountEvent = fn
}

// OnConnected registers a handler that fires after a successful connect or reconnect.
func (p *Plugin) OnConnected(fn func()) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.onConnected = fn
}

// OwnerID returns the owner's user ID extracted from the JWT sub claim.
func (p *Plugin) OwnerID() string {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.ownerID
}

// SendDM sends a direct message response back on the given conversation.
// Logs both attempt and result to comms.log for diagnostics.
func (p *Plugin) SendDM(ctx context.Context, conversationID, text string) error {
	p.mu.RLock()
	client := p.client
	p.mu.RUnlock()

	if client == nil {
		commLog.Error("[Comm:neboloop] SendDM: client is nil")
		return fmt.Errorf("neboloop: not connected")
	}

	convID, err := uuid.Parse(conversationID)
	if err != nil {
		commLog.Error("[Comm:neboloop] SendDM: invalid conversation ID", "conv_id", conversationID, "error", err)
		return fmt.Errorf("neboloop: invalid conversation ID %q: %w", conversationID, err)
	}

	content, _ := json.Marshal(map[string]string{"text": text})
	commLog.Debug("[Comm:neboloop] → SEND DM",
		"conv_id", conversationID,
		"text_len", len(text),
		"text_preview", truncateForLog(text, 200),
	)

	if err := client.Send(ctx, convID, "dm", content); err != nil {
		commLog.Error("[Comm:neboloop] SendDM failed", "conv_id", conversationID, "error", err)
		return err
	}
	commLog.Debug("[Comm:neboloop] SendDM ok", "conv_id", conversationID)
	return nil
}

// SendTyping sends a typing indicator on a DM conversation.
func (p *Plugin) SendTyping(ctx context.Context, conversationID string, typing bool) {
	p.mu.RLock()
	client := p.client
	p.mu.RUnlock()
	if client == nil {
		return
	}
	convID, err := uuid.Parse(conversationID)
	if err != nil {
		return
	}
	payload, _ := json.Marshal(map[string]bool{"typing": typing})
	_ = client.Send(ctx, convID, "typing", payload)
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
	p.ownerID = jwtSubClaim(p.token)

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
	p.lastMessageTime = time.Now()
	p.lastPingSuccess = time.Now()

	// Reset done channel for reconnect goroutine
	select {
	case <-p.done:
		p.done = make(chan struct{})
	default:
	}

	// Start watchdog to detect SDK disconnects and trigger reconnect.
	// The SDK closes its internal done channel on read errors (EOF, etc.)
	// which makes Send() return "client closed". We poll for this.
	go p.watchConnection(client)

	// Start health checker to proactively detect and reconnect stale connections.
	// This catches silent disconnects that the watchdog might miss.
	go p.startHealthChecker()

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
		commLog.Debug("[Comm:neboloop] ← LOOP message",
			"channel_id", channelID,
			"msg_id", msg.MsgID,
			"conv_id", msg.ConversationID,
			"sender_id", msg.SenderID,
			"content", string(msg.Content),
		)
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

	commLog.Info("[Comm:neboloop] Connected",
		"gateway", p.gateway,
		"bot_id", p.botID,
		"api_server", p.apiServer,
	)

	// Log channel subscriptions after a short delay (JOINs are async)
	go func() {
		time.Sleep(2 * time.Second)
		metas := client.ChannelMetas()
		channels := client.Channels()
		commLog.Info("[Comm:neboloop] Channel subscriptions after connect",
			"channel_metas_count", len(metas),
			"channel_convs_count", len(channels),
		)
		for chID, meta := range metas {
			commLog.Info("[Comm:neboloop] Subscribed channel",
				"channel_id", chID,
				"channel_name", meta.ChannelName,
				"loop_id", meta.LoopID,
			)
		}
		for chID, convID := range channels {
			commLog.Info("[Comm:neboloop] Channel→Conversation mapping",
				"channel_id", chID,
				"conversation_id", convID,
			)
		}
		if len(metas) == 0 && len(channels) == 0 {
			commLog.Warn("[Comm:neboloop] No channels subscribed — bot may not be a member of any loop channels")
		}
	}()

	fmt.Printf("[Comm:neboloop] Connected to %s\n", p.gateway)

	// Fire onConnected callback in a goroutine (deferred mu.Unlock runs first)
	if onConn := p.onConnected; onConn != nil {
		go onConn()
	}

	return nil
}

// handleMessage is the single handler for all incoming messages from the published SDK.
// It dispatches A2A messages (tasks, task results, direct messages) to the comm handler,
// and channel messages to the channel handler.
func (p *Plugin) handleMessage(msg neboloopsdk.Message) {
	// Update last message time for health checker
	p.mu.Lock()
	p.lastMessageTime = time.Now()
	p.mu.Unlock()

	commLog.Debug("[Comm:neboloop] ← RECV",
		"stream", msg.Stream,
		"msg_id", msg.MsgID,
		"conv_id", msg.ConversationID,
		"sender_id", msg.SenderID,
		"content", string(msg.Content),
	)
	// Channel inbound messages from external bridges (Telegram, Discord, etc.)
	// Note: loop channel messages (stream=channel) are handled exclusively by
	// the OnLoopMessage chain handler to avoid double-processing.
	if msg.Stream == "channels/inbound" {
		p.mu.RLock()
		fn := p.onChannelMessage
		p.mu.RUnlock()
		if fn == nil {
			return
		}

		var sdkMsg neboloopsdk.ChannelMessage
		if err := json.Unmarshal(msg.Content, &sdkMsg); err != nil {
			commLog.Error("unmarshal channel message", "error", err)
			return
		}

		cm := ChannelMessage{
			ChannelType:    sdkMsg.ChannelType,
			SenderName:     sdkMsg.SenderName,
			Text:           sdkMsg.Text,
			ConversationID: msg.ConversationID,
			MessageID:      msg.MsgID,
		}
		commLog.Debug("[Comm:neboloop] ← CHANNEL inbound",
			"channel_type", cm.ChannelType,
			"sender_name", cm.SenderName,
			"conv_id", cm.ConversationID,
			"text_len", len(cm.Text),
			"text_preview", truncateForLog(cm.Text, 200),
		)
		fn(cm)
		return
	}

	// Direct messages from owner or other bots
	if msg.Stream == "dm" {
		p.mu.RLock()
		fn := p.onDMMessage
		ownerID := p.ownerID
		p.mu.RUnlock()
		if fn == nil {
			return
		}

		var content struct {
			Text string `json:"text"`
		}
		if err := json.Unmarshal(msg.Content, &content); err != nil {
			commLog.Error("unmarshal dm message", "error", err)
			return
		}

		dm := DMMessage{
			SenderID:       msg.SenderID,
			Text:           content.Text,
			ConversationID: msg.ConversationID,
			MessageID:      msg.MsgID,
			IsOwner:        ownerID != "" && msg.SenderID == ownerID,
		}
		commLog.Debug("[Comm:neboloop] ← DM",
			"sender_id", dm.SenderID,
			"is_owner", dm.IsOwner,
			"conv_id", dm.ConversationID,
			"text_len", len(dm.Text),
			"text_preview", truncateForLog(dm.Text, 200),
		)
		fn(dm)
		return
	}

	// History request: NeboLoop asks for recent conversation history
	if msg.Stream == "history" {
		p.handleHistoryRequest(msg)
		return
	}

	// Account events (plan changes, etc.)
	if msg.Stream == "account" {
		var evt AccountEvent
		if err := json.Unmarshal(msg.Content, &evt); err != nil {
			commLog.Error("[Comm:neboloop] failed to parse account event", "error", err)
			return
		}
		commLog.Info("[Comm:neboloop] <- ACCOUNT event", "type", evt.Type)
		p.mu.RLock()
		handler := p.onAccountEvent
		p.mu.RUnlock()
		if handler != nil {
			handler(evt)
		}
		return
	}

	// A2A streams
	if msg.Stream == "a2a" || strings.HasPrefix(msg.Stream, "a2a/") {
		p.handleA2AMessage(msg)
		return
	}
}

// handleHistoryRequest processes a history request from NeboLoop and responds with recent messages.
func (p *Plugin) handleHistoryRequest(msg neboloopsdk.Message) {
	p.mu.RLock()
	fn := p.onHistoryRequest
	client := p.client
	p.mu.RUnlock()
	if fn == nil || client == nil {
		return
	}

	var req struct {
		Limit int `json:"limit"`
	}
	_ = json.Unmarshal(msg.Content, &req)
	if req.Limit <= 0 {
		req.Limit = 20
	}

	commLog.Debug("[Comm:neboloop] ← HISTORY request",
		"conv_id", msg.ConversationID,
		"limit", req.Limit,
	)

	messages := fn(HistoryRequest{
		ConversationID: msg.ConversationID,
		Limit:          req.Limit,
	})

	// Respond on the same conversation with stream=history_response
	payload, _ := json.Marshal(map[string]any{
		"messages": messages,
	})
	convID, err := uuid.Parse(msg.ConversationID)
	if err != nil {
		commLog.Error("[Comm:neboloop] history: invalid conv_id", "error", err)
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := client.Send(ctx, convID, "history_response", payload); err != nil {
		commLog.Error("[Comm:neboloop] history: failed to send response", "error", err)
	} else {
		commLog.Debug("[Comm:neboloop] → HISTORY response", "count", len(messages))
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
		commLog.Error("unmarshal a2a message", "error", err)
		return
	}

	// Task result (has status field)
	if probe.Status != "" {
		commLog.Debug("[Comm:neboloop] ← A2A task_result",
			"correlation_id", probe.CorrelationID,
			"status", probe.Status,
			"error", probe.Error,
			"output_len", len(probe.Output),
		)
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
		commLog.Debug("[Comm:neboloop] ← A2A task_submission",
			"correlation_id", probe.CorrelationID,
			"from_bot_id", probe.FromBotID,
			"input_len", len(probe.Input),
		)
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
	commLog.Debug("[Comm:neboloop] ← A2A direct_message",
		"sender_id", msg.SenderID,
		"text_len", len(probe.Text),
		"text_preview", truncateForLog(probe.Text, 200),
	)
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

	commLog.Debug("[Comm:neboloop] → SEND",
		"type", string(msg.Type),
		"to", msg.To,
		"from", msg.From,
		"conv_id", msg.ConversationID,
		"correlation_id", msg.CorrelationID,
		"task_status", string(msg.TaskStatus),
		"content_len", len(msg.Content),
		"content_preview", truncateForLog(msg.Content, 500),
	)

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

// --- watchdog & reconnect ---

// watchConnection polls the SDK client to detect silent disconnects.
// The SDK closes its internal done channel on read errors, causing Send()
// to return "client closed". When detected, marks disconnected and reconnects.
func (p *Plugin) watchConnection(client *neboloopsdk.Client) {
	ticker := time.NewTicker(15 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-p.done:
			return
		case <-ticker.C:
			// Probe liveness: Send to nil UUID with "ping" stream.
			// If the SDK's done channel is closed, this returns immediately.
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			err := client.Send(ctx, uuid.Nil, "ping", nil)
			cancel()

			if err == nil {
				p.mu.Lock()
				p.lastPingSuccess = time.Now()
				p.mu.Unlock()
				continue // Send succeeded (or was queued) — connection alive
			}

			if strings.Contains(err.Error(), "client closed") {
				commLog.Warn("[Comm:neboloop] connection lost (watchdog detected EOF)")
				p.mu.Lock()
				p.connected = false
				p.client = nil
				p.mu.Unlock()

				p.reconnect()
				return
			}
			// Other errors (timeout, etc.) are transient — keep watching
		}
	}
}

// startHealthChecker runs a background goroutine that periodically validates
// the WebSocket connection by checking ping success. This is independent of
// the watchdog and catches silent disconnects that happen between pings.
// Only uses ping timeout — absence of inbound messages is normal for idle bots.
func (p *Plugin) startHealthChecker() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	const pingTimeout = 1 * time.Minute // No successful pings = reconnect

	for {
		select {
		case <-p.done:
			return
		case <-ticker.C:
			p.mu.RLock()
			connected := p.connected
			lastPing := p.lastPingSuccess
			client := p.client
			p.mu.RUnlock()

			if !connected || client == nil {
				continue // Not connected, skip health check
			}

			// Check for stale ping (last successful ping was too long ago)
			if time.Since(lastPing) > pingTimeout {
				commLog.Warn("[Comm:neboloop] health check: no successful pings in 1 minute, marking stale")
				p.mu.Lock()
				p.connected = false
				p.client = nil
				p.mu.Unlock()
				p.reconnect()
				return
			}
		}
	}
}

// reconnect attempts to re-establish the connection with exponential backoff.
// Called when the watchdog detects a disconnect.
// Never stops retrying unless credentials are permanently rejected or p.done closes.
func (p *Plugin) reconnect() {
	p.mu.Lock()
	if p.reconnecting {
		p.mu.Unlock()
		commLog.Debug("[Comm:neboloop] reconnect already in progress, skipping")
		return
	}
	p.reconnecting = true
	dead := p.authDead
	gateway := p.gateway
	apiServer := p.apiServer
	botID := p.botID
	token := p.token
	p.mu.Unlock()

	defer func() {
		p.mu.Lock()
		p.reconnecting = false
		p.mu.Unlock()
	}()

	if dead {
		commLog.Error("[Comm:neboloop] credentials rejected, not reconnecting")
		return
	}

	// Exponential backoff: 100ms base, cap at 10min, never stop retrying
	// At scale (1M+ users), 60s would create thundering herd.
	// 10min allows graceful stagger across all clients.
	base := 100 * time.Millisecond
	maxDelay := 10 * time.Minute
	attempt := 0

	for {
		select {
		case <-p.done:
			return
		default:
		}

		// Calculate delay with exponential backoff capped at maxDelay
		delay := base * time.Duration(1<<uint(min(attempt, 9))) // 2^9 = 512, so max ~50s before cap
		if delay > maxDelay {
			delay = maxDelay
		}
		// Add jitter: ±25% of delay
		jitter := time.Duration(rand.Int64N(int64(delay) / 2))
		delay = delay - delay/4 + jitter
		attempt++

		commLog.Info("[Comm:neboloop] reconnecting", "attempt", attempt, "delay", delay)
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
				// Try to refresh the token before giving up
				p.mu.RLock()
				refresher := p.tokenRefresher
				p.mu.RUnlock()

				if refresher != nil {
					commLog.Info("[Comm:neboloop] auth failed, attempting token refresh")
					freshToken, refreshErr := refresher(context.Background())
					if refreshErr == nil && freshToken != "" {
						p.mu.Lock()
						p.token = freshToken
						p.ownerID = jwtSubClaim(freshToken)
						p.mu.Unlock()
						token = freshToken
						commLog.Info("[Comm:neboloop] token refreshed, retrying connection")
						continue // retry the reconnect loop with fresh token
					}
					commLog.Error("[Comm:neboloop] token refresh failed", "error", refreshErr)
				}

				p.mu.Lock()
				p.authDead = true
				p.mu.Unlock()
				commLog.Error("[Comm:neboloop] auth dead during reconnect, giving up (re-authenticate via Settings)")
				return
			}
			commLog.Warn("[Comm:neboloop] reconnect failed", "error", err, "attempt", attempt)
			continue // Always continue on transient errors, never give up
		}

		p.mu.Lock()
		p.client = client
		p.connected = true
		p.lastMessageTime = time.Now()
		p.lastPingSuccess = time.Now()
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

		// Re-sync loop channel subscriptions after reconnect
		// This ensures we're still subscribed to all channels we were in before
		go func() {
			time.Sleep(2 * time.Second)
			metas := client.ChannelMetas()
			channels := client.Channels()
			commLog.Info("[Comm:neboloop] Channel subscriptions after reconnect",
				"channel_metas_count", len(metas),
				"channel_convs_count", len(channels),
			)
			for chID, meta := range metas {
				commLog.Debug("[Comm:neboloop] Subscription verified after reconnect",
					"channel_id", chID,
					"channel_name", meta.ChannelName,
					"loop_id", meta.LoopID,
				)
			}
		}()

		// Start new watchdog for the fresh connection
		go p.watchConnection(client)

		// Start new health checker for the fresh connection
		go p.startHealthChecker()

		// Fire onConnected callback
		p.mu.RLock()
		onConn := p.onConnected
		p.mu.RUnlock()
		if onConn != nil {
			go onConn()
		}

		return
	}
}

// --- helpers ---

// jwtSubClaim extracts the "sub" claim from a JWT without verification.
// This is safe because the token is our own — we just need the owner ID for
// routing decisions, not for security checks.
func jwtSubClaim(token string) string {
	parts := strings.SplitN(token, ".", 3)
	if len(parts) < 2 {
		return ""
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return ""
	}
	var claims struct {
		Sub string `json:"sub"`
	}
	if json.Unmarshal(payload, &claims) != nil {
		return ""
	}
	return claims.Sub
}

// truncateForLog truncates a string for log output.
func truncateForLog(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

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
