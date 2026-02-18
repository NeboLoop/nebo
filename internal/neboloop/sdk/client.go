package sdk

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"net"
	"sync"
	"time"

	"github.com/gobwas/ws"
	"github.com/gobwas/ws/wsutil"
)

// Client is a NeboLoop comms SDK client. Single WebSocket, multiplexed conversations.
type Client struct {
	config     Config
	conn       net.Conn
	ackTracker map[string]uint64 // conversation_id hex → last_acked_seq
	seq        uint64            // monotonic send counter

	onInstall        func(InstallEvent)
	onChannelMessage func(ChannelMessage)
	onTask           func(TaskSubmission)
	onTaskResult     func(TaskResult)
	onDirectMessage  func(DirectMessage)
	onReconnect      func()

	sessionID string // from AUTH_OK

	connected bool
	authDead  bool // credentials rejected, stop reconnecting
	done      chan struct{}
	mu        sync.RWMutex
	writeMu   sync.Mutex // serializes writes
	logger    *slog.Logger
}

// Connect dials the NeboLoop gateway, authenticates, and starts read/heartbeat loops.
func Connect(ctx context.Context, cfg Config, opts ...Option) (*Client, error) {
	c := &Client{
		config:     cfg,
		ackTracker: make(map[string]uint64),
		done:       make(chan struct{}),
		logger:     slog.Default().With("component", "neboloop-sdk"),
	}
	for _, opt := range opts {
		opt(c)
	}

	if err := c.dial(ctx); err != nil {
		return nil, fmt.Errorf("connect: %w", err)
	}

	go c.readLoop()
	go c.heartbeat()

	return c, nil
}

// Option configures a Client.
type Option func(*Client)

// WithLogger sets a custom logger.
func WithLogger(l *slog.Logger) Option {
	return func(c *Client) { c.logger = l }
}

// OnInstall registers a handler for app install events.
func (c *Client) OnInstall(fn func(InstallEvent)) { c.onInstall = fn }

// OnChannelMessage registers a handler for inbound channel messages.
func (c *Client) OnChannelMessage(fn func(ChannelMessage)) { c.onChannelMessage = fn }

// OnTask registers a handler for A2A task submissions.
func (c *Client) OnTask(fn func(TaskSubmission)) { c.onTask = fn }

// OnTaskResult registers a handler for A2A task results.
func (c *Client) OnTaskResult(fn func(TaskResult)) { c.onTaskResult = fn }

// OnDirectMessage registers a handler for A2A direct messages.
func (c *Client) OnDirectMessage(fn func(DirectMessage)) { c.onDirectMessage = fn }

// OnReconnect registers a callback invoked after successful reconnection.
func (c *Client) OnReconnect(fn func()) { c.onReconnect = fn }

// Close shuts down the client gracefully.
func (c *Client) Close() error {
	c.mu.Lock()
	if c.connected {
		c.connected = false
	}
	c.mu.Unlock()

	select {
	case <-c.done:
	default:
		close(c.done)
	}

	if c.conn != nil {
		return c.conn.Close()
	}
	return nil
}

// IsConnected returns whether the client has an active authenticated connection.
func (c *Client) IsConnected() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.connected
}

// SessionID returns the session ID assigned by the gateway on auth.
func (c *Client) SessionID() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.sessionID
}

// --- Send methods ---

// SendChannelMessage sends a channel message (outbound to Telegram/Discord/etc).
func (c *Client) SendChannelMessage(ctx context.Context, convID string, msg ChannelMessage) error {
	content, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("marshal channel message: %w", err)
	}
	return c.sendMessage(ctx, convID, "channel", content, "")
}

// SubmitTask sends an A2A task submission.
func (c *Client) SubmitTask(ctx context.Context, convID string, task TaskSubmission) error {
	content, err := json.Marshal(task)
	if err != nil {
		return fmt.Errorf("marshal task: %w", err)
	}
	return c.sendMessage(ctx, convID, "task", content, "a2a")
}

// SendTaskResult sends an A2A task result.
func (c *Client) SendTaskResult(ctx context.Context, convID string, result TaskResult) error {
	content, err := json.Marshal(result)
	if err != nil {
		return fmt.Errorf("marshal task result: %w", err)
	}
	return c.sendMessage(ctx, convID, "task_result", content, "a2a")
}

// SendDirect sends an A2A direct message.
func (c *Client) SendDirect(ctx context.Context, convID string, msg DirectMessage) error {
	content, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("marshal direct: %w", err)
	}
	return c.sendMessage(ctx, convID, "direct", content, "a2a")
}

// --- Conversation methods ---

// Join subscribes to a conversation with optional delta replay from last acked seq.
func (c *Client) Join(ctx context.Context, conversationIDs ...string) error {
	c.mu.RLock()
	seqs := make(map[string]uint64, len(conversationIDs))
	for _, id := range conversationIDs {
		seqs[id] = c.ackTracker[id]
	}
	c.mu.RUnlock()

	payload, err := json.Marshal(joinPayload{LastAckedSeqs: seqs})
	if err != nil {
		return fmt.Errorf("marshal join: %w", err)
	}

	// Use first conversation ID for the frame, or zero if multiple
	var convBytes [16]byte
	if len(conversationIDs) == 1 {
		convBytes = parseConvID(conversationIDs[0])
	}

	return c.writeFrame(&Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameJoinConversation,
		ConversationID: convBytes,
		MessageID:      NewMessageID(),
		Payload:        payload,
	})
}

// Leave unsubscribes from a conversation.
func (c *Client) Leave(_ context.Context, conversationID string) error {
	payload, _ := json.Marshal(leavePayload{})
	return c.writeFrame(&Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameLeaveConversation,
		ConversationID: parseConvID(conversationID),
		MessageID:      NewMessageID(),
		Payload:        payload,
	})
}

// --- Internal ---

func (c *Client) dial(ctx context.Context) error {
	conn, _, _, err := ws.Dial(ctx, c.config.Gateway)
	if err != nil {
		return fmt.Errorf("websocket dial: %w", err)
	}
	c.conn = conn

	// Send CONNECT frame
	cp, err := json.Marshal(connectPayload{
		Token:    c.config.Token,
		BotID:    c.config.BotID,
		DeviceID: c.config.DeviceID,
	})
	if err != nil {
		conn.Close()
		return fmt.Errorf("marshal connect: %w", err)
	}

	frame := &Frame{
		ProtoVersion: ProtoVersion,
		FrameType:    FrameConnect,
		MessageID:    NewMessageID(),
		Payload:      cp,
	}
	encoded, err := EncodeFrame(frame)
	if err != nil {
		conn.Close()
		return fmt.Errorf("encode connect: %w", err)
	}
	if err := wsutil.WriteClientBinary(conn, encoded); err != nil {
		conn.Close()
		return fmt.Errorf("write connect: %w", err)
	}

	// Read auth response (with timeout)
	if dl, ok := ctx.Deadline(); ok {
		conn.SetReadDeadline(dl)
	} else {
		conn.SetReadDeadline(time.Now().Add(10 * time.Second))
	}

	data, err := wsutil.ReadServerBinary(conn)
	if err != nil {
		conn.Close()
		return fmt.Errorf("read auth response: %w", err)
	}
	conn.SetReadDeadline(time.Time{}) // Clear deadline

	authFrame, err := DecodeFrameFromBytes(data)
	if err != nil {
		conn.Close()
		return fmt.Errorf("decode auth response: %w", err)
	}

	switch authFrame.FrameType {
	case FrameAuthOK:
		var ok authOKPayload
		if err := json.Unmarshal(authFrame.Payload, &ok); err != nil {
			conn.Close()
			return fmt.Errorf("unmarshal auth ok: %w", err)
		}
		c.mu.Lock()
		c.sessionID = ok.SessionID
		c.connected = true
		c.mu.Unlock()
		c.logger.Info("authenticated", "session_id", ok.SessionID)
		return nil

	case FrameAuthFail:
		var fail authFailPayload
		json.Unmarshal(authFrame.Payload, &fail)
		conn.Close()
		c.mu.Lock()
		c.authDead = true
		c.mu.Unlock()
		return fmt.Errorf("auth failed: %s (code %d)", fail.Reason, fail.Code)

	default:
		conn.Close()
		return fmt.Errorf("unexpected frame type during auth: %d", authFrame.FrameType)
	}
}

func (c *Client) readLoop() {
	for {
		select {
		case <-c.done:
			return
		default:
		}

		data, err := wsutil.ReadServerBinary(c.conn)
		if err != nil {
			c.mu.Lock()
			wasConnected := c.connected
			c.connected = false
			c.mu.Unlock()

			if wasConnected {
				c.logger.Warn("connection lost", "error", err)
				go c.reconnect()
			}
			return
		}

		frame, err := DecodeFrameFromBytes(data)
		if err != nil {
			c.logger.Error("decode frame", "error", err)
			continue
		}

		c.handleFrame(frame)
	}
}

func (c *Client) handleFrame(f *Frame) {
	switch f.FrameType {
	case FrameMessageDelivery:
		c.handleDelivery(f)

	case FrameAck:
		// Server-side ack — nothing to do on client side currently

	case FrameSlowDown:
		c.logger.Warn("backpressure: slow down received")

	case FrameReplay:
		var summary resumeSummaryPayload
		if err := json.Unmarshal(f.Payload, &summary); err != nil {
			c.logger.Error("unmarshal resume summary", "error", err)
			return
		}
		for _, gap := range summary.Gaps {
			c.logger.Info("conversation gap",
				"conversation_id", gap.ConversationID,
				"gap_size", gap.GapSize)
		}

	case FramePresence:
		// Presence updates from other bots — log if needed

	default:
		c.logger.Debug("unhandled frame type", "type", f.FrameType)
	}
}

func (c *Client) handleDelivery(f *Frame) {
	convID := hex.EncodeToString(f.ConversationID[:])

	// Track seq for acking
	c.mu.Lock()
	if f.Seq > c.ackTracker[convID] {
		c.ackTracker[convID] = f.Seq
	}
	c.mu.Unlock()

	// Send ack
	c.sendAck(convID, f.Seq)

	var delivery deliveryPayload
	if err := json.Unmarshal(f.Payload, &delivery); err != nil {
		c.logger.Error("unmarshal delivery", "error", err)
		return
	}

	switch delivery.ContentType {
	case "install":
		if c.onInstall == nil {
			return
		}
		var evt InstallEvent
		if err := json.Unmarshal(delivery.Content, &evt); err != nil {
			c.logger.Error("unmarshal install event", "error", err)
			return
		}
		c.onInstall(evt)

	case "channel":
		if c.onChannelMessage == nil {
			return
		}
		var msg ChannelMessage
		if err := json.Unmarshal(delivery.Content, &msg); err != nil {
			c.logger.Error("unmarshal channel message", "error", err)
			return
		}
		msg.ConversationID = convID
		msg.MessageID = hex.EncodeToString(f.MessageID[:])
		c.onChannelMessage(msg)

	case "task":
		if c.onTask == nil {
			return
		}
		var task TaskSubmission
		if err := json.Unmarshal(delivery.Content, &task); err != nil {
			c.logger.Error("unmarshal task", "error", err)
			return
		}
		c.onTask(task)

	case "task_result":
		if c.onTaskResult == nil {
			return
		}
		var result TaskResult
		if err := json.Unmarshal(delivery.Content, &result); err != nil {
			c.logger.Error("unmarshal task result", "error", err)
			return
		}
		c.onTaskResult(result)

	case "direct":
		if c.onDirectMessage == nil {
			return
		}
		var dm DirectMessage
		if err := json.Unmarshal(delivery.Content, &dm); err != nil {
			c.logger.Error("unmarshal direct message", "error", err)
			return
		}
		c.onDirectMessage(dm)

	default:
		c.logger.Warn("unknown content_type in delivery", "content_type", delivery.ContentType)
	}
}

func (c *Client) sendMessage(ctx context.Context, convID, contentType string, content json.RawMessage, stream string) error {
	payload, err := json.Marshal(sendPayload{
		ContentType: contentType,
		Content:     content,
		Stream:      stream,
	})
	if err != nil {
		return fmt.Errorf("marshal send: %w", err)
	}

	c.mu.Lock()
	c.seq++
	seq := c.seq
	c.mu.Unlock()

	return c.writeFrame(&Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameSendMessage,
		ConversationID: parseConvID(convID),
		Seq:            seq,
		MessageID:      NewMessageID(),
		Payload:        payload,
	})
}

func (c *Client) sendAck(convID string, seq uint64) {
	payload, _ := json.Marshal(ackPayload{ConversationID: convID, AckedSeq: seq})
	c.writeFrame(&Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameAck,
		ConversationID: parseConvID(convID),
		MessageID:      NewMessageID(),
		Payload:        payload,
	})
}

func (c *Client) heartbeat() {
	ticker := time.NewTicker(20 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-c.done:
			return
		case <-ticker.C:
			c.mu.RLock()
			connected := c.connected
			c.mu.RUnlock()
			if !connected {
				continue
			}

			payload, _ := json.Marshal(presencePayload{Status: "online"})
			c.writeFrame(&Frame{
				ProtoVersion: ProtoVersion,
				FrameType:    FramePresence,
				MessageID:    NewMessageID(),
				Payload:      payload,
			})
		}
	}
}

func (c *Client) reconnect() {
	c.mu.RLock()
	authDead := c.authDead
	c.mu.RUnlock()
	if authDead {
		c.logger.Error("credentials rejected, not reconnecting")
		return
	}

	base := 100 * time.Millisecond
	cap := 10 * time.Second
	attempt := 0

	for {
		select {
		case <-c.done:
			return
		default:
		}

		delay := min(base*time.Duration(1<<attempt), cap)
		// Add jitter: ±25%
		jitter := time.Duration(rand.Int64N(int64(delay) / 2))
		delay = delay - delay/4 + jitter
		attempt++

		c.logger.Info("reconnecting", "attempt", attempt, "delay", delay)
		time.Sleep(delay)

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		err := c.dial(ctx)
		cancel()

		if err != nil {
			c.mu.RLock()
			dead := c.authDead
			c.mu.RUnlock()
			if dead {
				c.logger.Error("auth dead during reconnect, giving up")
				return
			}
			c.logger.Warn("reconnect failed", "error", err, "attempt", attempt)
			continue
		}

		// Re-join conversations with last acked seqs for delta replay
		c.mu.RLock()
		convIDs := make([]string, 0, len(c.ackTracker))
		for id := range c.ackTracker {
			convIDs = append(convIDs, id)
		}
		c.mu.RUnlock()

		if len(convIDs) > 0 {
			if err := c.Join(context.Background(), convIDs...); err != nil {
				c.logger.Error("rejoin conversations failed", "error", err)
			}
		}

		// Restart read loop
		go c.readLoop()

		if c.onReconnect != nil {
			c.onReconnect()
		}

		c.logger.Info("reconnected", "attempt", attempt)
		return
	}
}

func (c *Client) writeFrame(f *Frame) error {
	encoded, err := EncodeFrame(f)
	if err != nil {
		return fmt.Errorf("encode frame: %w", err)
	}

	c.writeMu.Lock()
	defer c.writeMu.Unlock()

	if c.conn == nil {
		return fmt.Errorf("not connected")
	}
	c.conn.SetWriteDeadline(time.Now().Add(5 * time.Second))
	return wsutil.WriteClientBinary(c.conn, encoded)
}

// --- helpers ---

func parseConvID(s string) [16]byte {
	var out [16]byte
	b, _ := hex.DecodeString(s)
	copy(out[:], b)
	return out
}

