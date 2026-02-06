package comm

import (
	"context"
	"fmt"
	"sync"
)

// LoopbackPlugin is an in-memory comm plugin for testing and development.
// It delivers sent messages back to the handler, enabling local testing
// of the full comm pipeline without external infrastructure.
type LoopbackPlugin struct {
	handler   func(CommMessage)
	connected bool
	topics    map[string]bool
	agentID   string
	mu        sync.RWMutex
}

// NewLoopbackPlugin creates a new loopback plugin
func NewLoopbackPlugin() *LoopbackPlugin {
	return &LoopbackPlugin{
		topics: make(map[string]bool),
	}
}

func (p *LoopbackPlugin) Name() string    { return "loopback" }
func (p *LoopbackPlugin) Version() string { return "1.0.0" }

func (p *LoopbackPlugin) Connect(ctx context.Context, config map[string]string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.connected = true
	fmt.Printf("[Comm:loopback] Connected\n")
	return nil
}

func (p *LoopbackPlugin) Disconnect(ctx context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.connected = false
	p.topics = make(map[string]bool)
	fmt.Printf("[Comm:loopback] Disconnected\n")
	return nil
}

func (p *LoopbackPlugin) IsConnected() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.connected
}

func (p *LoopbackPlugin) Send(ctx context.Context, msg CommMessage) error {
	p.mu.RLock()
	connected := p.connected
	p.mu.RUnlock()

	if !connected {
		return fmt.Errorf("loopback plugin not connected")
	}

	fmt.Printf("[Comm:loopback] Message sent: from=%s to=%s topic=%s\n", msg.From, msg.To, msg.Topic)
	return nil
}

func (p *LoopbackPlugin) Subscribe(ctx context.Context, topic string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.connected {
		return fmt.Errorf("loopback plugin not connected")
	}
	p.topics[topic] = true
	fmt.Printf("[Comm:loopback] Subscribed to: %s\n", topic)
	return nil
}

func (p *LoopbackPlugin) Unsubscribe(ctx context.Context, topic string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.connected {
		return fmt.Errorf("loopback plugin not connected")
	}
	delete(p.topics, topic)
	fmt.Printf("[Comm:loopback] Unsubscribed from: %s\n", topic)
	return nil
}

func (p *LoopbackPlugin) Register(ctx context.Context, agentID string, card *AgentCard) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.agentID = agentID
	if card != nil {
		fmt.Printf("[Comm:loopback] Registered agent: %s skills=%d\n", agentID, len(card.Skills))
	} else {
		fmt.Printf("[Comm:loopback] Registered agent: %s\n", agentID)
	}
	return nil
}

func (p *LoopbackPlugin) Deregister(ctx context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.agentID = ""
	return nil
}

func (p *LoopbackPlugin) SetMessageHandler(handler func(msg CommMessage)) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.handler = handler
}

// InjectMessage simulates receiving a message from the network.
// Used for testing — delivers the message to the handler as if it arrived externally.
func (p *LoopbackPlugin) InjectMessage(msg CommMessage) {
	p.mu.RLock()
	handler := p.handler
	subscribed := p.topics[msg.Topic]
	p.mu.RUnlock()

	if handler == nil {
		fmt.Printf("[Comm:loopback] No handler set, dropping message\n")
		return
	}
	if !subscribed {
		fmt.Printf("[Comm:loopback] Not subscribed to topic %q, dropping message\n", msg.Topic)
		return
	}
	handler(msg)
}
