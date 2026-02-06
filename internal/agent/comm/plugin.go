package comm

import "context"

// CommPlugin defines the interface for communication transport plugins.
// Plugins run in-process (not via hashicorp/go-plugin RPC).
// Implementations include loopback (testing), MQTT, NATS, neboloop, etc.
type CommPlugin interface {
	// Identity
	Name() string
	Version() string

	// Lifecycle
	Connect(ctx context.Context, config map[string]string) error
	Disconnect(ctx context.Context) error
	IsConnected() bool

	// Messaging
	Send(ctx context.Context, msg CommMessage) error
	Subscribe(ctx context.Context, topic string) error
	Unsubscribe(ctx context.Context, topic string) error

	// Registration with the comm network
	Register(ctx context.Context, agentID string, card *AgentCard) error
	Deregister(ctx context.Context) error

	// Message handler (set by CommPluginManager)
	SetMessageHandler(handler func(msg CommMessage))
}
