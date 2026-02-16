package apps

import (
	"context"
	"encoding/json"
	"fmt"
	"io"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/comm"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agent/tools"
	pb "github.com/neboloop/nebo/internal/apps/pb"
)

// --- Gateway-to-Provider Adapter ---

// GatewayProviderAdapter bridges a gateway app's gRPC client to Nebo's ai.Provider interface.
type GatewayProviderAdapter struct {
	client    pb.GatewayServiceClient
	manifest  *AppManifest
	appID     string
	profileID string
}

// NewGatewayProviderAdapter creates a new adapter that makes a gateway app a first-class provider.
// The manifest is used for runtime permission enforcement (e.g., filtering user tokens).
func NewGatewayProviderAdapter(client pb.GatewayServiceClient, manifest *AppManifest, profileID string) *GatewayProviderAdapter {
	return &GatewayProviderAdapter{
		client:    client,
		manifest:  manifest,
		appID:     manifest.ID,
		profileID: profileID,
	}
}

func (g *GatewayProviderAdapter) ID() string        { return "gateway-" + g.appID }
func (g *GatewayProviderAdapter) ProfileID() string  { return g.profileID }
func (g *GatewayProviderAdapter) HandlesTools() bool { return false }

func (g *GatewayProviderAdapter) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error) {
	gatewayReq := convertChatToGateway(req, g.manifest)

	stream, err := g.client.Stream(ctx, gatewayReq)
	if err != nil {
		return nil, fmt.Errorf("gateway stream: %w", err)
	}

	ch := make(chan ai.StreamEvent, 32)
	go func() {
		defer close(ch)
		for {
			event, err := stream.Recv()
			if err != nil {
				if err != io.EOF {
					ch <- ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("gateway recv: %w", err)}
				}
				ch <- ai.StreamEvent{Type: ai.EventTypeDone}
				return
			}
			se := convertGatewayToStream(event)
			if se.Type != "" {
				ch <- se
			}
		}
	}()

	return ch, nil
}

func convertChatToGateway(req *ai.ChatRequest, manifest *AppManifest) *pb.GatewayRequest {
	gatewayReq := &pb.GatewayRequest{
		MaxTokens:   int32(req.MaxTokens),
		Temperature: req.Temperature,
		System:      req.System,
	}

	// Pass user context if available.
	// Per spec: all apps receive user_id and plan as convenience fields.
	// Only apps with "user:token" permission receive the full JWT.
	if req.UserToken != "" || req.UserID != "" {
		uc := &pb.UserContext{
			UserId: req.UserID,
			Plan:   req.UserPlan,
		}
		if manifest != nil && CheckPermission(manifest, "user:token") {
			uc.Token = req.UserToken
		}
		gatewayReq.User = uc
	}

	for _, msg := range req.Messages {
		gm := &pb.GatewayMessage{
			Role:    msg.Role,
			Content: msg.Content,
		}
		if len(msg.ToolCalls) > 0 {
			gm.ToolCalls = string(msg.ToolCalls)
		}
		if len(msg.ToolResults) > 0 {
			// Tool results are sent as role="tool" messages
			var results []session.ToolResult
			if json.Unmarshal(msg.ToolResults, &results) == nil && len(results) > 0 {
				gm.ToolCallId = results[0].ToolCallID
				gm.Content = results[0].Content
			}
		}
		gatewayReq.Messages = append(gatewayReq.Messages, gm)
	}

	for _, tool := range req.Tools {
		gatewayReq.Tools = append(gatewayReq.Tools, &pb.GatewayToolDef{
			Name:        tool.Name,
			Description: tool.Description,
			InputSchema: tool.InputSchema,
		})
	}

	return gatewayReq
}

func convertGatewayToStream(event *pb.GatewayEvent) ai.StreamEvent {
	switch event.Type {
	case "text":
		return ai.StreamEvent{Type: ai.EventTypeText, Text: event.Content}

	case "tool_call":
		// Content is a JSON blob: {"id":"...","name":"...","arguments":"..."}
		var tc struct {
			ID        string `json:"id"`
			Name      string `json:"name"`
			Arguments string `json:"arguments"`
		}
		if err := json.Unmarshal([]byte(event.Content), &tc); err != nil {
			return ai.StreamEvent{
				Type:  ai.EventTypeError,
				Error: fmt.Errorf("invalid tool_call JSON: %w", err),
			}
		}
		return ai.StreamEvent{
			Type: ai.EventTypeToolCall,
			ToolCall: &ai.ToolCall{
				ID:    tc.ID,
				Name:  tc.Name,
				Input: json.RawMessage(tc.Arguments),
			},
		}

	case "thinking":
		return ai.StreamEvent{Type: ai.EventTypeThinking, Text: event.Content}

	case "error":
		return ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("gateway: %s", event.Content)}

	case "done":
		return ai.StreamEvent{Type: ai.EventTypeDone}

	default:
		return ai.StreamEvent{}
	}
}

// --- Tool Adapter ---

// AppToolAdapter bridges a tool app's gRPC client to Nebo's tools.Tool interface.
type AppToolAdapter struct {
	client   pb.ToolServiceClient
	name     string
	desc     string
	schema   json.RawMessage
	approval bool
}

// NewAppToolAdapter creates a tool adapter by querying the app for its metadata.
func NewAppToolAdapter(ctx context.Context, client pb.ToolServiceClient) (*AppToolAdapter, error) {
	nameResp, err := client.Name(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("tool name: %w", err)
	}
	descResp, err := client.Description(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("tool description: %w", err)
	}
	schemaResp, err := client.Schema(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("tool schema: %w", err)
	}
	approvalResp, err := client.RequiresApproval(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("tool requires_approval: %w", err)
	}

	return &AppToolAdapter{
		client:   client,
		name:     nameResp.Name,
		desc:     descResp.Description,
		schema:   json.RawMessage(schemaResp.Schema),
		approval: approvalResp.RequiresApproval,
	}, nil
}

func (a *AppToolAdapter) Name() string                { return a.name }
func (a *AppToolAdapter) Description() string         { return a.desc }
func (a *AppToolAdapter) Schema() json.RawMessage     { return a.schema }
func (a *AppToolAdapter) RequiresApproval() bool      { return a.approval }

func (a *AppToolAdapter) Execute(ctx context.Context, input json.RawMessage) (*tools.ToolResult, error) {
	resp, err := a.client.Execute(ctx, &pb.ExecuteRequest{Input: input})
	if err != nil {
		return nil, fmt.Errorf("tool execute: %w", err)
	}
	return &tools.ToolResult{
		Content: resp.Content,
		IsError: resp.IsError,
	}, nil
}

// --- Comm Adapter ---

// AppCommAdapter bridges a comm app's gRPC client to Nebo's comm.CommPlugin interface.
type AppCommAdapter struct {
	client  pb.CommServiceClient
	name    string
	version string
	handler func(comm.CommMessage)
	cancel  context.CancelFunc
}

// NewAppCommAdapter creates a comm adapter by querying the app for its metadata.
func NewAppCommAdapter(ctx context.Context, client pb.CommServiceClient) (*AppCommAdapter, error) {
	nameResp, err := client.Name(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("comm name: %w", err)
	}
	verResp, err := client.Version(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("comm version: %w", err)
	}
	return &AppCommAdapter{
		client:  client,
		name:    nameResp.Name,
		version: verResp.Version,
	}, nil
}

func (a *AppCommAdapter) Name() string    { return a.name }
func (a *AppCommAdapter) Version() string { return a.version }

func (a *AppCommAdapter) Connect(ctx context.Context, config map[string]string) error {
	resp, err := a.client.Connect(ctx, &pb.CommConnectRequest{Config: config})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}

	// Start receiving messages via gRPC server streaming
	recvCtx, cancel := context.WithCancel(context.Background())
	a.cancel = cancel

	go func() {
		stream, err := a.client.Receive(recvCtx, &pb.Empty{})
		if err != nil {
			return
		}
		for {
			msg, err := stream.Recv()
			if err != nil {
				return
			}
			if a.handler != nil {
				a.handler(fromProtoCommMessage(msg))
			}
		}
	}()

	return nil
}

func (a *AppCommAdapter) Disconnect(ctx context.Context) error {
	if a.cancel != nil {
		a.cancel()
	}
	resp, err := a.client.Disconnect(ctx, &pb.Empty{})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) IsConnected() bool {
	resp, err := a.client.IsConnected(context.Background(), &pb.Empty{})
	if err != nil {
		return false
	}
	return resp.Connected
}

func (a *AppCommAdapter) Send(ctx context.Context, msg comm.CommMessage) error {
	resp, err := a.client.Send(ctx, &pb.CommSendRequest{Message: toProtoCommMessage(msg)})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) Subscribe(ctx context.Context, topic string) error {
	resp, err := a.client.Subscribe(ctx, &pb.CommSubscribeRequest{Topic: topic})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) Unsubscribe(ctx context.Context, topic string) error {
	resp, err := a.client.Unsubscribe(ctx, &pb.CommUnsubscribeRequest{Topic: topic})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) Register(ctx context.Context, agentID string, card *comm.AgentCard) error {
	caps := make([]string, len(card.Skills))
	for i, s := range card.Skills {
		caps[i] = s.ID
	}
	resp, err := a.client.Register(ctx, &pb.CommRegisterRequest{
		AgentId:      agentID,
		Capabilities: caps,
	})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) Deregister(ctx context.Context) error {
	resp, err := a.client.Deregister(ctx, &pb.Empty{})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppCommAdapter) SetMessageHandler(handler func(comm.CommMessage)) {
	a.handler = handler
}

// --- Channel Adapter ---

// AppChannelAdapter bridges a channel app's gRPC client to Nebo's channel interface.
type AppChannelAdapter struct {
	client  pb.ChannelServiceClient
	id      string
	handler func(channelID, userID, text, metadata string)
	cancel  context.CancelFunc
}

// NewAppChannelAdapter creates a channel adapter by querying the app for its ID.
func NewAppChannelAdapter(ctx context.Context, client pb.ChannelServiceClient) (*AppChannelAdapter, error) {
	idResp, err := client.ID(ctx, &pb.Empty{})
	if err != nil {
		return nil, fmt.Errorf("channel id: %w", err)
	}
	return &AppChannelAdapter{
		client: client,
		id:     idResp.Id,
	}, nil
}

func (a *AppChannelAdapter) ID() string { return a.id }

func (a *AppChannelAdapter) Connect(ctx context.Context, config map[string]string) error {
	resp, err := a.client.Connect(ctx, &pb.ChannelConnectRequest{Config: config})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}

	// Start receiving inbound messages via gRPC server streaming
	recvCtx, cancel := context.WithCancel(context.Background())
	a.cancel = cancel

	go func() {
		stream, err := a.client.Receive(recvCtx, &pb.Empty{})
		if err != nil {
			fmt.Printf("[apps:channel:%s] Receive stream failed: %v\n", a.id, err)
			return
		}
		for {
			msg, err := stream.Recv()
			if err != nil {
				if err != io.EOF && recvCtx.Err() == nil {
					fmt.Printf("[apps:channel:%s] Receive error: %v\n", a.id, err)
				}
				return
			}
			if a.handler != nil {
				a.handler(msg.ChannelId, msg.UserId, msg.Text, msg.Metadata)
			}
		}
	}()

	return nil
}

func (a *AppChannelAdapter) Disconnect(ctx context.Context) error {
	if a.cancel != nil {
		a.cancel()
	}
	resp, err := a.client.Disconnect(ctx, &pb.Empty{})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppChannelAdapter) Send(ctx context.Context, channelID, text string) error {
	resp, err := a.client.Send(ctx, &pb.ChannelSendRequest{
		ChannelId: channelID,
		Text:      text,
	})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

// SetMessageHandler sets the callback for inbound messages from this channel.
func (a *AppChannelAdapter) SetMessageHandler(handler func(channelID, userID, text, metadata string)) {
	a.handler = handler
}

// --- Proto conversion helpers ---

func toProtoCommMessage(msg comm.CommMessage) *pb.CommMessage {
	return &pb.CommMessage{
		Id:             msg.ID,
		From:           msg.From,
		To:             msg.To,
		Topic:          msg.Topic,
		ConversationId: msg.ConversationID,
		Type:           string(msg.Type),
		Content:        msg.Content,
		Metadata:       msg.Metadata,
		Timestamp:      msg.Timestamp,
		HumanInjected:  msg.HumanInjected,
		HumanId:        msg.HumanID,
	}
}

func fromProtoCommMessage(msg *pb.CommMessage) comm.CommMessage {
	return comm.CommMessage{
		ID:             msg.Id,
		From:           msg.From,
		To:             msg.To,
		Topic:          msg.Topic,
		ConversationID: msg.ConversationId,
		Type:           comm.CommMessageType(msg.Type),
		Content:        msg.Content,
		Metadata:       msg.Metadata,
		Timestamp:      msg.Timestamp,
		HumanInjected:  msg.HumanInjected,
		HumanID:        msg.HumanId,
	}
}
