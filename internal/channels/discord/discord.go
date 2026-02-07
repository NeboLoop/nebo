package discord

import (
	"context"
	"fmt"
	"sync"

	"github.com/nebolabs/nebo/internal/channels"
	"github.com/nebolabs/nebo/internal/plugin"

	"github.com/bwmarrin/discordgo"
)

// Compile-time check: Adapter implements plugin.Configurable
var _ plugin.Configurable = (*Adapter)(nil)

// Adapter implements the Channel interface for Discord
type Adapter struct {
	session *discordgo.Session
	handler func(channels.InboundMessage)
	mu      sync.RWMutex
}

// New creates a new Discord adapter
func New() *Adapter {
	return &Adapter{}
}

// ID returns the channel identifier
func (a *Adapter) ID() string {
	return "discord"
}

// Connect establishes connection to Discord
func (a *Adapter) Connect(ctx context.Context, cfg channels.ChannelConfig) error {
	if cfg.Token == "" {
		return fmt.Errorf("discord bot token is required")
	}

	session, err := discordgo.New("Bot " + cfg.Token)
	if err != nil {
		return fmt.Errorf("failed to create discord session: %w", err)
	}

	// Set intents
	session.Identify.Intents = discordgo.IntentsGuildMessages | discordgo.IntentsDirectMessages | discordgo.IntentsMessageContent

	// Register message handler
	session.AddHandler(a.messageHandler)

	// Open connection
	if err := session.Open(); err != nil {
		return fmt.Errorf("failed to open discord connection: %w", err)
	}

	a.session = session

	fmt.Println("[Discord] Bot connected and listening for messages")
	return nil
}

// Disconnect closes the connection
func (a *Adapter) Disconnect() error {
	if a.session != nil {
		return a.session.Close()
	}
	return nil
}

// Send sends a message to a Discord channel
func (a *Adapter) Send(ctx context.Context, msg channels.OutboundMessage) error {
	if a.session == nil {
		return fmt.Errorf("discord bot not connected")
	}

	// Build message send
	data := &discordgo.MessageSend{
		Content: msg.Text,
	}

	// Reply to a specific message
	if msg.ReplyToID != "" {
		data.Reference = &discordgo.MessageReference{
			MessageID: msg.ReplyToID,
		}
	}

	_, err := a.session.ChannelMessageSendComplex(msg.ChannelID, data)
	return err
}

// SetHandler sets the callback for incoming messages
func (a *Adapter) SetHandler(fn func(channels.InboundMessage)) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.handler = fn
}

// Manifest returns the settings schema for the Discord plugin.
func (a *Adapter) Manifest() plugin.SettingsManifest {
	return plugin.SettingsManifest{
		Groups: []plugin.SettingsGroup{
			{
				Title:       "Discord Bot",
				Description: "Credentials for your Discord bot",
				Fields: []plugin.SettingsField{
					{Key: "bot_token", Title: "Bot Token", Type: plugin.FieldPassword, Required: true, Secret: true, Placeholder: "Paste your Discord bot token"},
					{Key: "guild_id", Title: "Guild ID", Type: plugin.FieldText, Description: "Optional: restrict to a specific server"},
				},
			},
		},
	}
}

// OnSettingsChanged handles hot-reload when settings are updated via the UI.
func (a *Adapter) OnSettingsChanged(settings map[string]string) error {
	token := settings["bot_token"]
	if token == "" {
		return nil // Nothing to reconnect with
	}
	// Disconnect and reconnect with new credentials
	_ = a.Disconnect()
	return a.Connect(context.Background(), channels.ChannelConfig{
		Token: token,
	})
}

// messageHandler handles incoming Discord messages
func (a *Adapter) messageHandler(s *discordgo.Session, m *discordgo.MessageCreate) {
	// Ignore messages from the bot itself
	if m.Author.ID == s.State.User.ID {
		return
	}

	// Build inbound message
	inbound := channels.InboundMessage{
		ChannelType: "discord",
		ChannelID:   m.ChannelID,
		MessageID:   m.ID,
		Text:        m.Content,
		SenderID:    m.Author.ID,
		SenderName:  m.Author.Username,
		Raw:         m,
	}

	// Handle reply reference
	if m.ReferencedMessage != nil {
		inbound.ReplyToID = m.ReferencedMessage.ID
	}

	// Handle thread
	if m.Thread != nil {
		inbound.ThreadID = m.Thread.ID
	}

	// Call handler
	a.mu.RLock()
	handler := a.handler
	a.mu.RUnlock()

	if handler != nil {
		handler(inbound)
	}
}
