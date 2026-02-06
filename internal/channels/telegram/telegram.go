package telegram

import (
	"context"
	"fmt"
	"strconv"
	"sync"

	"github.com/nebolabs/nebo/internal/channels"

	"github.com/go-telegram/bot"
	"github.com/go-telegram/bot/models"
)

// Adapter implements the Channel interface for Telegram
type Adapter struct {
	bot     *bot.Bot
	handler func(channels.InboundMessage)
	mu      sync.RWMutex
	cancel  context.CancelFunc
}

// New creates a new Telegram adapter
func New() *Adapter {
	return &Adapter{}
}

// ID returns the channel identifier
func (a *Adapter) ID() string {
	return "telegram"
}

// Connect establishes connection to Telegram
func (a *Adapter) Connect(ctx context.Context, cfg channels.ChannelConfig) error {
	if cfg.Token == "" {
		return fmt.Errorf("telegram bot token is required")
	}

	opts := []bot.Option{
		bot.WithDefaultHandler(a.defaultHandler),
	}

	b, err := bot.New(cfg.Token, opts...)
	if err != nil {
		return fmt.Errorf("failed to create telegram bot: %w", err)
	}

	a.bot = b

	// Start the bot in a goroutine
	ctx, cancel := context.WithCancel(ctx)
	a.cancel = cancel
	go b.Start(ctx)

	fmt.Println("[Telegram] Bot connected and listening for messages")
	return nil
}

// Disconnect closes the connection
func (a *Adapter) Disconnect() error {
	if a.cancel != nil {
		a.cancel()
	}
	return nil
}

// Send sends a message to a Telegram chat
func (a *Adapter) Send(ctx context.Context, msg channels.OutboundMessage) error {
	if a.bot == nil {
		return fmt.Errorf("telegram bot not connected")
	}

	chatID, err := strconv.ParseInt(msg.ChannelID, 10, 64)
	if err != nil {
		return fmt.Errorf("invalid chat ID: %w", err)
	}

	params := &bot.SendMessageParams{
		ChatID: chatID,
		Text:   msg.Text,
	}

	// Set parse mode
	if msg.ParseMode != "" {
		switch msg.ParseMode {
		case "markdown":
			params.ParseMode = models.ParseModeMarkdown
		case "html":
			params.ParseMode = models.ParseModeHTML
		}
	}

	// Reply to a specific message
	if msg.ReplyToID != "" {
		replyID, err := strconv.ParseInt(msg.ReplyToID, 10, 64)
		if err == nil {
			params.ReplyParameters = &models.ReplyParameters{
				MessageID: int(replyID),
			}
		}
	}

	_, err = a.bot.SendMessage(ctx, params)
	return err
}

// SetHandler sets the callback for incoming messages
func (a *Adapter) SetHandler(fn func(channels.InboundMessage)) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.handler = fn
}

// defaultHandler handles all incoming updates
func (a *Adapter) defaultHandler(ctx context.Context, b *bot.Bot, update *models.Update) {
	if update.Message == nil {
		return
	}

	msg := update.Message

	// Build inbound message
	inbound := channels.InboundMessage{
		ChannelType: "telegram",
		ChannelID:   strconv.FormatInt(msg.Chat.ID, 10),
		MessageID:   strconv.Itoa(msg.ID),
		Text:        msg.Text,
		SenderID:    strconv.FormatInt(msg.From.ID, 10),
		SenderName:  msg.From.FirstName,
		Raw:         update,
	}

	if msg.From.LastName != "" {
		inbound.SenderName += " " + msg.From.LastName
	}

	if msg.ReplyToMessage != nil {
		inbound.ReplyToID = strconv.Itoa(msg.ReplyToMessage.ID)
	}

	// Thread ID for topic messages
	if msg.MessageThreadID != 0 {
		inbound.ThreadID = strconv.Itoa(msg.MessageThreadID)
	}

	// Call handler
	a.mu.RLock()
	handler := a.handler
	a.mu.RUnlock()

	if handler != nil {
		handler(inbound)
	}
}
