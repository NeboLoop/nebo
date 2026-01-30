package realtime

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"nebo/internal/logging"
)

const (
	// Time allowed to write a message to the peer.
	writeWait = 10 * time.Second

	// Time allowed to read the next pong message from the peer.
	pongWait = 60 * time.Second

	// Send pings to peer with this period. Must be less than pongWait.
	pingPeriod = (pongWait * 9) / 10

	// Maximum message size allowed from peer.
	maxMessageSize = 32768 // 32KB
)

// Error types
var (
	ErrClientSendBufferFull = errors.New("client send buffer full")
	ErrClientClosed         = errors.New("client connection closed")
)

// Client represents a websocket connection
type Client struct {
	// The websocket connection.
	conn *websocket.Conn

	// Buffered channel of outbound messages.
	send chan []byte

	// Hub reference
	hub *Hub

	// Client metadata
	ID     string
	UserID string

	// Context for cancellation
	ctx    context.Context
	cancel context.CancelFunc

	// Closed flag to prevent sending on closed channel
	closed   bool
	closedMu sync.RWMutex
}

// NewClient creates a new WebSocket client
func NewClient(conn *websocket.Conn, hub *Hub, id, userID string) *Client {
	ctx, cancel := context.WithCancel(context.Background())

	return &Client{
		conn:   conn,
		hub:    hub,
		send:   make(chan []byte, 256),
		ID:     id,
		UserID: userID,
		ctx:    ctx,
		cancel: cancel,
	}
}

// readPump pumps messages from the websocket connection to the hub.
func (c *Client) readPump() {
	defer func() {
		c.hub.unregister <- c
		c.conn.Close()
		c.cancel()
	}()

	c.conn.SetReadLimit(maxMessageSize)
	c.conn.SetReadDeadline(time.Now().Add(pongWait))
	c.conn.SetPongHandler(func(string) error {
		c.conn.SetReadDeadline(time.Now().Add(pongWait))
		return nil
	})

	for {
		_, msg, err := c.conn.ReadMessage()
		if err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				logging.Errorf("WebSocket read error: %v", err)
			}
			break
		}

		c.handleTextMessage(msg)
	}
}

// writePump pumps messages from the hub to the websocket connection.
func (c *Client) writePump() {
	ticker := time.NewTicker(pingPeriod)
	defer func() {
		ticker.Stop()
		c.conn.Close()
	}()

	for {
		select {
		case message, ok := <-c.send:
			c.conn.SetWriteDeadline(time.Now().Add(writeWait))
			if !ok {
				// The hub closed the channel.
				c.conn.WriteMessage(websocket.CloseMessage, []byte{})
				return
			}

				// Send each message as its own WebSocket frame for proper streaming
			if err := c.conn.WriteMessage(websocket.TextMessage, message); err != nil {
				return
			}

		case <-ticker.C:
			c.conn.SetWriteDeadline(time.Now().Add(writeWait))
			if err := c.conn.WriteMessage(websocket.PingMessage, nil); err != nil {
				return
			}

		case <-c.ctx.Done():
			return
		}
	}
}

// handleTextMessage processes incoming text messages from the client
func (c *Client) handleTextMessage(msg []byte) {
	var message Message
	if err := json.Unmarshal(msg, &message); err != nil {
		logging.Errorf("Error unmarshaling message: %v", err)
		return
	}

	c.handleMessage(&message)
}

// MessageHandler is a function type for custom message handlers
type MessageHandler func(c *Client, msg *Message)

// rewriteHandler is set by the rewrite package to handle rewrite messages
var rewriteHandler MessageHandler

// chatHandler is set to handle chat messages
var chatHandler MessageHandler

// approvalResponseHandler is set to handle approval responses
var approvalResponseHandler MessageHandler

// SetRewriteHandler sets the handler for rewrite messages
func SetRewriteHandler(handler MessageHandler) {
	rewriteHandler = handler
}

// SetChatHandler sets the handler for chat messages
func SetChatHandler(handler MessageHandler) {
	chatHandler = handler
}

// SetApprovalResponseHandler sets the handler for approval responses
func SetApprovalResponseHandler(handler MessageHandler) {
	approvalResponseHandler = handler
}

// handleMessage processes incoming messages from the client
func (c *Client) handleMessage(msg *Message) {
	logging.Infof("[Client] Received message type=%s from client %s", msg.Type, c.ID)
	switch msg.Type {
	case "ping":
		c.handlePing(msg)
	case "rewrite":
		c.handleRewrite(msg)
	case "chat":
		c.handleChat(msg)
	case "approval_response":
		c.handleApprovalResponse(msg)
	default:
		logging.Infof("Unknown message type: %s", msg.Type)
	}
}

// handleApprovalResponse processes approval responses from the client
func (c *Client) handleApprovalResponse(msg *Message) {
	if approvalResponseHandler != nil {
		approvalResponseHandler(c, msg)
	} else {
		logging.Error("Approval response handler not registered")
	}
}

// handleChat processes chat messages
func (c *Client) handleChat(msg *Message) {
	logging.Infof("[Client] handleChat called: session=%v", msg.Data["session_id"])
	if chatHandler != nil {
		chatHandler(c, msg)
	} else {
		logging.Error("Chat handler not registered")
		// Send error back to client
		errMsg := &Message{
			Type:      "error",
			Data:      map[string]interface{}{"error": "Chat handler not available", "session_id": msg.Data["session_id"]},
			Timestamp: time.Now(),
		}
		data, _ := json.Marshal(errMsg)
		select {
		case c.send <- data:
		default:
		}
	}
}

// handleRewrite processes rewrite requests
func (c *Client) handleRewrite(msg *Message) {
	if rewriteHandler != nil {
		rewriteHandler(c, msg)
	} else {
		logging.Error("Rewrite handler not registered")
	}
}

// handlePing responds to ping messages
func (c *Client) handlePing(msg *Message) {
	pong := &Message{
		Type:      "pong",
		Timestamp: time.Now(),
	}

	data, err := json.Marshal(pong)
	if err != nil {
		logging.Errorf("Error marshaling pong message: %v", err)
		return
	}

	select {
	case c.send <- data:
	default:
		close(c.send)
	}
}

// SendMessage sends a message to the client
func (c *Client) SendMessage(msg *Message) (err error) {
	// Use defer/recover to handle race condition where channel is closed
	// between the check and the send
	defer func() {
		if r := recover(); r != nil {
			err = ErrClientClosed
		}
	}()

	c.closedMu.RLock()
	if c.closed {
		c.closedMu.RUnlock()
		return ErrClientClosed
	}
	c.closedMu.RUnlock()

	data, err := json.Marshal(msg)
	if err != nil {
		return err
	}

	select {
	case c.send <- data:
		return nil
	default:
		return ErrClientSendBufferFull
	}
}

// IsClosed returns whether the client connection is closed
func (c *Client) IsClosed() bool {
	c.closedMu.RLock()
	defer c.closedMu.RUnlock()
	return c.closed
}

// Close closes the client connection
func (c *Client) Close() {
	c.closedMu.Lock()
	if c.closed {
		c.closedMu.Unlock()
		return
	}
	c.closed = true
	c.closedMu.Unlock()

	c.cancel()
	close(c.send)
	c.conn.Close()
}

// ServeWS handles websocket requests from the peer.
func ServeWS(hub *Hub, conn *websocket.Conn, clientID, userID string) {
	client := NewClient(conn, hub, clientID, userID)
	client.hub.register <- client

	// Start goroutines for reading and writing
	go client.writePump()
	go client.readPump()
}
