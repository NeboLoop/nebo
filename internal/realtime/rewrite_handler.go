package realtime

import (
	"github.com/nebolabs/nebo/internal/svc"

	"github.com/nebolabs/nebo/internal/logging"
)

// RewriteHandler handles WebSocket messages
// This is a placeholder - implement your domain-specific logic here
type RewriteHandler struct {
	svcCtx *svc.ServiceContext
}

// NewRewriteHandler creates a new RewriteHandler
func NewRewriteHandler(svcCtx *svc.ServiceContext) *RewriteHandler {
	return &RewriteHandler{svcCtx: svcCtx}
}

// Register registers the handler with the realtime package
func (h *RewriteHandler) Register() {
	SetRewriteHandler(h.handleMessage)
}

// handleMessage processes incoming WebSocket messages
// Customize this for your application's needs
func (h *RewriteHandler) handleMessage(c *Client, msg *Message) {
	logging.Infof("[RewriteHandler] Received message from client %s: type=%s", c.ID, msg.Type)

	// Example: Echo the message back
	response := &Message{
		Type:      "echo",
		Channel:   msg.Channel,
		Data:      msg.Data,
		Timestamp: msg.Timestamp,
	}

	if err := c.SendMessage(response); err != nil {
		logging.Errorf("[RewriteHandler] Failed to send response: %v", err)
	}
}
