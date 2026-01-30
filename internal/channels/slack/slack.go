package slack

import (
	"context"
	"fmt"
	"sync"

	"nebo/internal/channels"

	"github.com/slack-go/slack"
	"github.com/slack-go/slack/slackevents"
	"github.com/slack-go/slack/socketmode"
)

// Adapter implements the Channel interface for Slack
type Adapter struct {
	client  *slack.Client
	socket  *socketmode.Client
	handler func(channels.InboundMessage)
	mu      sync.RWMutex
	cancel  context.CancelFunc
	botID   string
}

// New creates a new Slack adapter
func New() *Adapter {
	return &Adapter{}
}

// ID returns the channel identifier
func (a *Adapter) ID() string {
	return "slack"
}

// Connect establishes connection to Slack
func (a *Adapter) Connect(ctx context.Context, cfg channels.ChannelConfig) error {
	if cfg.Token == "" {
		return fmt.Errorf("slack bot token is required")
	}

	// Create Slack client
	a.client = slack.New(
		cfg.Token,
		slack.OptionAppLevelToken(cfg.Token),
	)

	// Create Socket Mode client
	a.socket = socketmode.New(
		a.client,
		socketmode.OptionDebug(false),
	)

	// Get bot identity
	authResp, err := a.client.AuthTest()
	if err != nil {
		return fmt.Errorf("failed to authenticate with slack: %w", err)
	}
	a.botID = authResp.BotID

	// Start listening in a goroutine
	ctx, cancel := context.WithCancel(ctx)
	a.cancel = cancel

	go a.listen(ctx)
	go a.socket.Run()

	fmt.Println("[Slack] Bot connected and listening for messages")
	return nil
}

// Disconnect closes the connection
func (a *Adapter) Disconnect() error {
	if a.cancel != nil {
		a.cancel()
	}
	return nil
}

// Send sends a message to a Slack channel
func (a *Adapter) Send(ctx context.Context, msg channels.OutboundMessage) error {
	if a.client == nil {
		return fmt.Errorf("slack bot not connected")
	}

	opts := []slack.MsgOption{
		slack.MsgOptionText(msg.Text, false),
	}

	// Reply in thread
	if msg.ThreadID != "" {
		opts = append(opts, slack.MsgOptionTS(msg.ThreadID))
	}

	_, _, err := a.client.PostMessage(msg.ChannelID, opts...)
	return err
}

// SetHandler sets the callback for incoming messages
func (a *Adapter) SetHandler(fn func(channels.InboundMessage)) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.handler = fn
}

// listen handles incoming events from Socket Mode
func (a *Adapter) listen(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case event := <-a.socket.Events:
			a.handleEvent(event)
		}
	}
}

// handleEvent processes a Socket Mode event
func (a *Adapter) handleEvent(event socketmode.Event) {
	switch event.Type {
	case socketmode.EventTypeEventsAPI:
		eventsAPIEvent, ok := event.Data.(slackevents.EventsAPIEvent)
		if !ok {
			return
		}

		// Acknowledge the event
		a.socket.Ack(*event.Request)

		// Handle message events
		switch innerEvent := eventsAPIEvent.InnerEvent.Data.(type) {
		case *slackevents.MessageEvent:
			a.handleMessage(innerEvent)
		}
	}
}

// handleMessage processes an incoming message
func (a *Adapter) handleMessage(msg *slackevents.MessageEvent) {
	// Ignore messages from the bot itself
	if msg.BotID == a.botID || msg.User == "" {
		return
	}

	// Ignore message updates/deletes
	if msg.SubType != "" {
		return
	}

	// Get user info for name
	userName := msg.User
	userInfo, err := a.client.GetUserInfo(msg.User)
	if err == nil {
		userName = userInfo.RealName
	}

	// Build inbound message
	inbound := channels.InboundMessage{
		ChannelType: "slack",
		ChannelID:   msg.Channel,
		MessageID:   msg.TimeStamp,
		Text:        msg.Text,
		SenderID:    msg.User,
		SenderName:  userName,
		ThreadID:    msg.ThreadTimeStamp,
		Raw:         msg,
	}

	// Call handler
	a.mu.RLock()
	handler := a.handler
	a.mu.RUnlock()

	if handler != nil {
		handler(inbound)
	}
}
