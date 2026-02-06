// Package plugins provides a hot-loadable plugin system using hashicorp/go-plugin.
// Plugins run as separate processes and communicate via RPC, enabling hot-reload
// without recompiling the main binary.
package plugins

import (
	"context"
	"encoding/json"
	"log"
	"net/rpc"
	"sync"
	"time"

	"github.com/nebolabs/nebo/internal/agent/comm"

	"github.com/hashicorp/go-plugin"
)

// Handshake is used to verify plugin compatibility
var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

// PluginMap is the map of plugins we can dispense
var PluginMap = map[string]plugin.Plugin{
	"tool":    &ToolPluginRPC{},
	"channel": &ChannelPluginRPC{},
	"comm":    &CommPluginRPC{},
}

// =============================================================================
// Tool Plugin Interface
// =============================================================================

// ToolResult represents the result of a tool execution
type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

// ToolPlugin is the interface that tool plugins must implement
type ToolPlugin interface {
	// Name returns the unique name of the tool
	Name() string

	// Description returns a human-readable description
	Description() string

	// Schema returns the JSON Schema for the tool's input
	Schema() json.RawMessage

	// Execute runs the tool with the given input
	Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error)

	// RequiresApproval indicates if this tool needs user approval
	RequiresApproval() bool
}

// ToolPluginRPC is the RPC implementation of the tool plugin
type ToolPluginRPC struct {
	Impl ToolPlugin
}

func (p *ToolPluginRPC) Server(*plugin.MuxBroker) (interface{}, error) {
	return &ToolRPCServer{Impl: p.Impl}, nil
}

func (p *ToolPluginRPC) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &ToolRPCClient{client: c}, nil
}

// ToolRPCServer is the server-side RPC handler
type ToolRPCServer struct {
	Impl ToolPlugin
}

func (s *ToolRPCServer) Name(_ struct{}, resp *string) error {
	*resp = s.Impl.Name()
	return nil
}

func (s *ToolRPCServer) Description(_ struct{}, resp *string) error {
	*resp = s.Impl.Description()
	return nil
}

func (s *ToolRPCServer) Schema(_ struct{}, resp *json.RawMessage) error {
	*resp = s.Impl.Schema()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

type ExecuteReply struct {
	Result *ToolResult
	Error  string
}

func (s *ToolRPCServer) Execute(args ExecuteArgs, reply *ExecuteReply) error {
	result, err := s.Impl.Execute(context.Background(), args.Input)
	reply.Result = result
	if err != nil {
		reply.Error = err.Error()
	}
	return nil
}

func (s *ToolRPCServer) RequiresApproval(_ struct{}, resp *bool) error {
	*resp = s.Impl.RequiresApproval()
	return nil
}

// ToolRPCClient is the client-side RPC implementation
type ToolRPCClient struct {
	client *rpc.Client
}

func (c *ToolRPCClient) Name() string {
	var resp string
	_ = c.client.Call("Plugin.Name", struct{}{}, &resp)
	return resp
}

func (c *ToolRPCClient) Description() string {
	var resp string
	_ = c.client.Call("Plugin.Description", struct{}{}, &resp)
	return resp
}

func (c *ToolRPCClient) Schema() json.RawMessage {
	var resp json.RawMessage
	_ = c.client.Call("Plugin.Schema", struct{}{}, &resp)
	return resp
}

func (c *ToolRPCClient) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var reply ExecuteReply
	err := c.client.Call("Plugin.Execute", ExecuteArgs{Input: input}, &reply)
	if err != nil {
		return nil, err
	}
	if reply.Error != "" {
		return reply.Result, &PluginError{Message: reply.Error}
	}
	return reply.Result, nil
}

func (c *ToolRPCClient) RequiresApproval() bool {
	var resp bool
	_ = c.client.Call("Plugin.RequiresApproval", struct{}{}, &resp)
	return resp
}

// =============================================================================
// Channel Plugin Interface
// =============================================================================

// InboundMessage represents a message received from a channel
type InboundMessage struct {
	ChannelID string `json:"channel_id"`
	UserID    string `json:"user_id"`
	Text      string `json:"text"`
	Metadata  string `json:"metadata"` // JSON-encoded metadata
}

// ChannelPlugin is the interface for channel plugins
type ChannelPlugin interface {
	// ID returns the unique identifier for this channel
	ID() string

	// Connect establishes connection to the channel
	Connect(ctx context.Context, config map[string]string) error

	// Disconnect closes the channel connection
	Disconnect(ctx context.Context) error

	// Send sends a message to the channel
	Send(ctx context.Context, channelID, text string) error

	// SetHandler sets the callback for incoming messages
	SetHandler(fn func(msg InboundMessage))
}

// ChannelPluginRPC is the RPC implementation of the channel plugin
type ChannelPluginRPC struct {
	Impl ChannelPlugin
}

func (p *ChannelPluginRPC) Server(*plugin.MuxBroker) (interface{}, error) {
	return &ChannelRPCServer{Impl: p.Impl}, nil
}

func (p *ChannelPluginRPC) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &ChannelRPCClient{client: c}, nil
}

// ChannelRPCServer is the server-side RPC handler
type ChannelRPCServer struct {
	Impl ChannelPlugin
}

func (s *ChannelRPCServer) ID(_ struct{}, resp *string) error {
	*resp = s.Impl.ID()
	return nil
}

type ConnectArgs struct {
	Config map[string]string
}

func (s *ChannelRPCServer) Connect(args ConnectArgs, reply *string) error {
	err := s.Impl.Connect(context.Background(), args.Config)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

func (s *ChannelRPCServer) Disconnect(_ struct{}, reply *string) error {
	err := s.Impl.Disconnect(context.Background())
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

type SendArgs struct {
	ChannelID string
	Text      string
}

func (s *ChannelRPCServer) Send(args SendArgs, reply *string) error {
	err := s.Impl.Send(context.Background(), args.ChannelID, args.Text)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

// ChannelRPCClient is the client-side RPC implementation
type ChannelRPCClient struct {
	client  *rpc.Client
	handler func(msg InboundMessage)
}

func (c *ChannelRPCClient) ID() string {
	var resp string
	_ = c.client.Call("Plugin.ID", struct{}{}, &resp)
	return resp
}

func (c *ChannelRPCClient) Connect(ctx context.Context, config map[string]string) error {
	var reply string
	err := c.client.Call("Plugin.Connect", ConnectArgs{Config: config}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *ChannelRPCClient) Disconnect(ctx context.Context) error {
	var reply string
	err := c.client.Call("Plugin.Disconnect", struct{}{}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *ChannelRPCClient) Send(ctx context.Context, channelID, text string) error {
	var reply string
	err := c.client.Call("Plugin.Send", SendArgs{ChannelID: channelID, Text: text}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *ChannelRPCClient) SetHandler(fn func(msg InboundMessage)) {
	c.handler = fn
}

// =============================================================================
// Comm Plugin Interface
// =============================================================================

// CommRPCMessage is the RPC wire format for comm messages.
// It mirrors comm.CommMessage but uses plain string for Type to keep the
// plugin SDK self-contained for external plugin authors.
type CommRPCMessage struct {
	ID             string            `json:"id"`
	From           string            `json:"from"`
	To             string            `json:"to"`
	Topic          string            `json:"topic"`
	ConversationID string            `json:"conversation_id"`
	Type           string            `json:"type"`
	Content        string            `json:"content"`
	Metadata       map[string]string `json:"metadata,omitempty"`
	Timestamp      int64             `json:"timestamp"`
	HumanInjected  bool              `json:"human_injected,omitempty"`
	HumanID        string            `json:"human_id,omitempty"`
}

// toRPCMessage converts comm.CommMessage to CommRPCMessage for RPC serialization
func toRPCMessage(msg comm.CommMessage) CommRPCMessage {
	return CommRPCMessage{
		ID:             msg.ID,
		From:           msg.From,
		To:             msg.To,
		Topic:          msg.Topic,
		ConversationID: msg.ConversationID,
		Type:           string(msg.Type),
		Content:        msg.Content,
		Metadata:       msg.Metadata,
		Timestamp:      msg.Timestamp,
		HumanInjected:  msg.HumanInjected,
		HumanID:        msg.HumanID,
	}
}

// fromRPCMessage converts CommRPCMessage back to comm.CommMessage
func fromRPCMessage(msg CommRPCMessage) comm.CommMessage {
	return comm.CommMessage{
		ID:             msg.ID,
		From:           msg.From,
		To:             msg.To,
		Topic:          msg.Topic,
		ConversationID: msg.ConversationID,
		Type:           comm.CommMessageType(msg.Type),
		Content:        msg.Content,
		Metadata:       msg.Metadata,
		Timestamp:      msg.Timestamp,
		HumanInjected:  msg.HumanInjected,
		HumanID:        msg.HumanID,
	}
}

// CommPluginRemote is the interface external comm plugin binaries implement.
// It is identical to comm.CommPlugin except SetMessageHandler is replaced
// with Receive(), which returns queued inbound messages. The host process
// polls Receive() and converts results to handler callbacks.
type CommPluginRemote interface {
	Name() string
	Version() string
	Connect(ctx context.Context, config map[string]string) error
	Disconnect(ctx context.Context) error
	IsConnected() bool
	Send(ctx context.Context, msg CommRPCMessage) error
	Subscribe(ctx context.Context, topic string) error
	Unsubscribe(ctx context.Context, topic string) error
	Register(ctx context.Context, agentID string, capabilities []string) error
	Deregister(ctx context.Context) error
	// Receive drains queued inbound messages. Returns empty slice when none.
	Receive(ctx context.Context) ([]CommRPCMessage, error)
}

// CommPluginRPC is the hashicorp/go-plugin.Plugin implementation for comm plugins
type CommPluginRPC struct {
	Impl CommPluginRemote
}

func (p *CommPluginRPC) Server(*plugin.MuxBroker) (interface{}, error) {
	return &CommRPCServer{Impl: p.Impl}, nil
}

func (p *CommPluginRPC) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &CommRPCClient{client: c}, nil
}

// CommRPCServer is the server-side RPC handler (runs in the external plugin process)
type CommRPCServer struct {
	Impl CommPluginRemote
}

func (s *CommRPCServer) Name(_ struct{}, resp *string) error {
	*resp = s.Impl.Name()
	return nil
}

func (s *CommRPCServer) Version(_ struct{}, resp *string) error {
	*resp = s.Impl.Version()
	return nil
}

// CommConnectArgs is used for the Connect RPC call
type CommConnectArgs struct {
	Config map[string]string
}

func (s *CommRPCServer) Connect(args CommConnectArgs, reply *string) error {
	err := s.Impl.Connect(context.Background(), args.Config)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

func (s *CommRPCServer) Disconnect(_ struct{}, reply *string) error {
	err := s.Impl.Disconnect(context.Background())
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

func (s *CommRPCServer) IsConnected(_ struct{}, resp *bool) error {
	*resp = s.Impl.IsConnected()
	return nil
}

// CommSendArgs is used for the Send RPC call
type CommSendArgs struct {
	Message CommRPCMessage
}

func (s *CommRPCServer) Send(args CommSendArgs, reply *string) error {
	err := s.Impl.Send(context.Background(), args.Message)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

// CommSubscribeArgs is used for Subscribe/Unsubscribe RPC calls
type CommSubscribeArgs struct {
	Topic string
}

func (s *CommRPCServer) Subscribe(args CommSubscribeArgs, reply *string) error {
	err := s.Impl.Subscribe(context.Background(), args.Topic)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

func (s *CommRPCServer) Unsubscribe(args CommSubscribeArgs, reply *string) error {
	err := s.Impl.Unsubscribe(context.Background(), args.Topic)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

// CommRegisterArgs is used for the Register RPC call
type CommRegisterArgs struct {
	AgentID      string
	Capabilities []string
}

func (s *CommRPCServer) Register(args CommRegisterArgs, reply *string) error {
	err := s.Impl.Register(context.Background(), args.AgentID, args.Capabilities)
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

func (s *CommRPCServer) Deregister(_ struct{}, reply *string) error {
	err := s.Impl.Deregister(context.Background())
	if err != nil {
		*reply = err.Error()
	}
	return nil
}

// CommReceiveReply holds the response from a Receive RPC call
type CommReceiveReply struct {
	Messages []CommRPCMessage
	Error    string
}

func (s *CommRPCServer) Receive(_ struct{}, reply *CommReceiveReply) error {
	msgs, err := s.Impl.Receive(context.Background())
	reply.Messages = msgs
	if err != nil {
		reply.Error = err.Error()
	}
	return nil
}

// CommRPCClient is the host-side RPC client that implements comm.CommPlugin.
// It translates RPC calls and runs a polling goroutine for inbound messages.
type CommRPCClient struct {
	client   *rpc.Client
	handler  func(comm.CommMessage)
	mu       sync.Mutex
	pollStop chan struct{}
	polling  bool
}

func (c *CommRPCClient) Name() string {
	var resp string
	_ = c.client.Call("Plugin.Name", struct{}{}, &resp)
	return resp
}

func (c *CommRPCClient) Version() string {
	var resp string
	_ = c.client.Call("Plugin.Version", struct{}{}, &resp)
	return resp
}

func (c *CommRPCClient) Connect(ctx context.Context, config map[string]string) error {
	var reply string
	err := c.client.Call("Plugin.Connect", CommConnectArgs{Config: config}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) Disconnect(ctx context.Context) error {
	c.stopPolling()
	var reply string
	err := c.client.Call("Plugin.Disconnect", struct{}{}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) IsConnected() bool {
	var resp bool
	_ = c.client.Call("Plugin.IsConnected", struct{}{}, &resp)
	return resp
}

func (c *CommRPCClient) Send(ctx context.Context, msg comm.CommMessage) error {
	rpcMsg := toRPCMessage(msg)
	var reply string
	err := c.client.Call("Plugin.Send", CommSendArgs{Message: rpcMsg}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) Subscribe(ctx context.Context, topic string) error {
	var reply string
	err := c.client.Call("Plugin.Subscribe", CommSubscribeArgs{Topic: topic}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) Unsubscribe(ctx context.Context, topic string) error {
	var reply string
	err := c.client.Call("Plugin.Unsubscribe", CommSubscribeArgs{Topic: topic}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) Register(ctx context.Context, agentID string, capabilities []string) error {
	var reply string
	err := c.client.Call("Plugin.Register", CommRegisterArgs{AgentID: agentID, Capabilities: capabilities}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

func (c *CommRPCClient) Deregister(ctx context.Context) error {
	var reply string
	err := c.client.Call("Plugin.Deregister", struct{}{}, &reply)
	if err != nil {
		return err
	}
	if reply != "" {
		return &PluginError{Message: reply}
	}
	return nil
}

// SetMessageHandler stores the callback and starts/stops the poll goroutine.
// This bridges the poll-based RPC and the callback-based comm.CommPlugin interface.
func (c *CommRPCClient) SetMessageHandler(handler func(msg comm.CommMessage)) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.handler = handler
	if handler != nil && !c.polling {
		c.startPolling()
	} else if handler == nil {
		c.stopPollLocked()
	}
}

// startPolling launches a goroutine that polls the plugin for inbound messages.
// Must hold c.mu when calling.
func (c *CommRPCClient) startPolling() {
	c.pollStop = make(chan struct{})
	c.polling = true
	stop := c.pollStop

	go func() {
		const minInterval = 50 * time.Millisecond
		const maxInterval = 500 * time.Millisecond
		interval := minInterval

		ticker := time.NewTicker(interval)
		defer ticker.Stop()

		for {
			select {
			case <-stop:
				return
			case <-ticker.C:
				var reply CommReceiveReply
				err := c.client.Call("Plugin.Receive", struct{}{}, &reply)
				if err != nil {
					log.Printf("[plugins] Comm poll error: %v", err)
					interval = maxInterval
					ticker.Reset(interval)
					continue
				}
				if reply.Error != "" {
					log.Printf("[plugins] Comm receive error: %s", reply.Error)
					interval = maxInterval
					ticker.Reset(interval)
					continue
				}

				if len(reply.Messages) > 0 {
					interval = minInterval
					ticker.Reset(interval)

					c.mu.Lock()
					handler := c.handler
					c.mu.Unlock()

					if handler != nil {
						for _, rpcMsg := range reply.Messages {
							handler(fromRPCMessage(rpcMsg))
						}
					}
				} else if interval < maxInterval {
					interval = min(interval*2, maxInterval)
					ticker.Reset(interval)
				}
			}
		}
	}()
}

// stopPolling signals the poll goroutine to exit.
func (c *CommRPCClient) stopPolling() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.stopPollLocked()
}

// stopPollLocked stops polling (must hold c.mu).
func (c *CommRPCClient) stopPollLocked() {
	if c.polling && c.pollStop != nil {
		close(c.pollStop)
		c.polling = false
	}
}

// =============================================================================
// Errors
// =============================================================================

// PluginError wraps errors from plugins
type PluginError struct {
	Message string
}

func (e *PluginError) Error() string {
	return e.Message
}
