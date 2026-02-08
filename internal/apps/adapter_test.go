package apps

import (
	"encoding/json"
	"testing"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/comm"
	"github.com/nebolabs/nebo/internal/agent/session"
	pb "github.com/nebolabs/nebo/internal/apps/pb"
)

func TestConvertChatToGateway_Basic(t *testing.T) {
	req := &ai.ChatRequest{
		MaxTokens:   1024,
		Temperature: 0.7,
		System:      "You are a helpful assistant.",
		Messages: []session.Message{
			{Role: "user", Content: "Hello"},
			{Role: "assistant", Content: "Hi there!"},
		},
		Tools: []ai.ToolDefinition{
			{
				Name:        "search",
				Description: "Search the web",
				InputSchema: json.RawMessage(`{"type":"object","properties":{"q":{"type":"string"}}}`),
			},
		},
	}

	manifest := &AppManifest{
		ID:          "com.test.gateway",
		Permissions: []string{"network:api.example.com:443"},
	}

	gw := convertChatToGateway(req, manifest)

	if gw.MaxTokens != 1024 {
		t.Errorf("MaxTokens = %d, want 1024", gw.MaxTokens)
	}
	if gw.Temperature != 0.7 {
		t.Errorf("Temperature = %f, want 0.7", gw.Temperature)
	}
	if gw.System != "You are a helpful assistant." {
		t.Errorf("System = %q, want 'You are a helpful assistant.'", gw.System)
	}
	if len(gw.Messages) != 2 {
		t.Fatalf("len(Messages) = %d, want 2", len(gw.Messages))
	}
	if gw.Messages[0].Role != "user" || gw.Messages[0].Content != "Hello" {
		t.Errorf("Messages[0] = {%s, %s}, want {user, Hello}", gw.Messages[0].Role, gw.Messages[0].Content)
	}
	if len(gw.Tools) != 1 {
		t.Fatalf("len(Tools) = %d, want 1", len(gw.Tools))
	}
	if gw.Tools[0].Name != "search" {
		t.Errorf("Tools[0].Name = %q, want 'search'", gw.Tools[0].Name)
	}
}

func TestConvertChatToGateway_UserTokenFiltering(t *testing.T) {
	req := &ai.ChatRequest{
		Messages:  []session.Message{{Role: "user", Content: "hi"}},
		UserToken: "jwt-secret-token",
		UserID:    "user-123",
		UserPlan:  "pro",
	}

	t.Run("with user:token permission", func(t *testing.T) {
		manifest := &AppManifest{
			ID:          "com.test.gateway",
			Permissions: []string{"network:*", "user:token"},
		}
		gw := convertChatToGateway(req, manifest)

		if gw.User == nil {
			t.Fatal("User context should be set")
		}
		if gw.User.Token != "jwt-secret-token" {
			t.Errorf("Token = %q, want 'jwt-secret-token'", gw.User.Token)
		}
		if gw.User.UserId != "user-123" {
			t.Errorf("UserId = %q, want 'user-123'", gw.User.UserId)
		}
		if gw.User.Plan != "pro" {
			t.Errorf("Plan = %q, want 'pro'", gw.User.Plan)
		}
	})

	t.Run("without user:token permission", func(t *testing.T) {
		manifest := &AppManifest{
			ID:          "com.test.gateway",
			Permissions: []string{"network:*"},
		}
		gw := convertChatToGateway(req, manifest)

		if gw.User == nil {
			t.Fatal("User context should be set (with filtered token)")
		}
		if gw.User.Token != "" {
			t.Errorf("Token should be empty without user:token permission, got %q", gw.User.Token)
		}
		// user_id and plan should always be passed
		if gw.User.UserId != "user-123" {
			t.Errorf("UserId = %q, want 'user-123'", gw.User.UserId)
		}
		if gw.User.Plan != "pro" {
			t.Errorf("Plan = %q, want 'pro'", gw.User.Plan)
		}
	})

	t.Run("with user:* wildcard permission", func(t *testing.T) {
		manifest := &AppManifest{
			ID:          "com.test.gateway",
			Permissions: []string{"network:*", "user:*"},
		}
		gw := convertChatToGateway(req, manifest)

		if gw.User == nil {
			t.Fatal("User context should be set")
		}
		if gw.User.Token != "jwt-secret-token" {
			t.Errorf("Token = %q, want 'jwt-secret-token' (user:* should match user:token)", gw.User.Token)
		}
	})

	t.Run("nil manifest passes no token", func(t *testing.T) {
		gw := convertChatToGateway(req, nil)

		if gw.User == nil {
			t.Fatal("User context should be set")
		}
		if gw.User.Token != "" {
			t.Errorf("Token should be empty with nil manifest, got %q", gw.User.Token)
		}
	})
}

func TestConvertChatToGateway_NoUserContext(t *testing.T) {
	req := &ai.ChatRequest{
		Messages: []session.Message{{Role: "user", Content: "hi"}},
	}
	manifest := &AppManifest{ID: "test", Permissions: []string{"user:token"}}

	gw := convertChatToGateway(req, manifest)
	if gw.User != nil {
		t.Error("User context should be nil when no user info is provided")
	}
}

func TestConvertChatToGateway_ToolCallsAndResults(t *testing.T) {
	toolCalls := json.RawMessage(`[{"id":"tc1","name":"search","arguments":"{}"}]`)
	toolResults := json.RawMessage(`[{"tool_call_id":"tc1","content":"result"}]`)

	req := &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "assistant", Content: "", ToolCalls: toolCalls},
			{Role: "tool", Content: "", ToolResults: toolResults},
		},
	}

	gw := convertChatToGateway(req, &AppManifest{ID: "test"})

	if len(gw.Messages) != 2 {
		t.Fatalf("len(Messages) = %d, want 2", len(gw.Messages))
	}
	if gw.Messages[0].ToolCalls == "" {
		t.Error("assistant message should have tool_calls")
	}
	if gw.Messages[1].ToolCallId != "tc1" {
		t.Errorf("tool message ToolCallId = %q, want 'tc1'", gw.Messages[1].ToolCallId)
	}
	if gw.Messages[1].Content != "result" {
		t.Errorf("tool message Content = %q, want 'result'", gw.Messages[1].Content)
	}
}

func TestConvertGatewayToStream(t *testing.T) {
	tests := []struct {
		name     string
		event    *pb.GatewayEvent
		wantType ai.StreamEventType
		check    func(t *testing.T, se ai.StreamEvent)
	}{
		{
			name:     "text event",
			event:    &pb.GatewayEvent{Type: "text", Content: "Hello world"},
			wantType: ai.EventTypeText,
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.Text != "Hello world" {
					t.Errorf("Text = %q, want 'Hello world'", se.Text)
				}
			},
		},
		{
			name: "tool_call event",
			event: &pb.GatewayEvent{
				Type:    "tool_call",
				Content: `{"id":"tc1","name":"search","arguments":"{\"q\":\"test\"}"}`,
			},
			wantType: ai.EventTypeToolCall,
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.ToolCall == nil {
					t.Fatal("ToolCall should not be nil")
				}
				if se.ToolCall.ID != "tc1" {
					t.Errorf("ToolCall.ID = %q, want 'tc1'", se.ToolCall.ID)
				}
				if se.ToolCall.Name != "search" {
					t.Errorf("ToolCall.Name = %q, want 'search'", se.ToolCall.Name)
				}
			},
		},
		{
			name:     "tool_call invalid JSON",
			event:    &pb.GatewayEvent{Type: "tool_call", Content: "not json"},
			wantType: ai.EventTypeError,
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.Error == nil {
					t.Error("Error should be set for invalid tool_call JSON")
				}
			},
		},
		{
			name:     "thinking event",
			event:    &pb.GatewayEvent{Type: "thinking", Content: "Let me think..."},
			wantType: ai.EventTypeThinking,
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.Text != "Let me think..." {
					t.Errorf("Text = %q, want 'Let me think...'", se.Text)
				}
			},
		},
		{
			name:     "error event",
			event:    &pb.GatewayEvent{Type: "error", Content: "something went wrong"},
			wantType: ai.EventTypeError,
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.Error == nil {
					t.Fatal("Error should be set")
				}
			},
		},
		{
			name:     "done event",
			event:    &pb.GatewayEvent{Type: "done"},
			wantType: ai.EventTypeDone,
		},
		{
			name:  "unknown event type",
			event: &pb.GatewayEvent{Type: "unknown"},
			check: func(t *testing.T, se ai.StreamEvent) {
				if se.Type != "" {
					t.Errorf("unknown event should return empty type, got %q", se.Type)
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			se := convertGatewayToStream(tt.event)
			if tt.wantType != "" && se.Type != tt.wantType {
				t.Errorf("Type = %q, want %q", se.Type, tt.wantType)
			}
			if tt.check != nil {
				tt.check(t, se)
			}
		})
	}
}

func TestProtoCommMessageRoundTrip(t *testing.T) {
	original := comm.CommMessage{
		ID:             "msg-123",
		From:           "agent-a",
		To:             "agent-b",
		Topic:          "tasks",
		ConversationID: "conv-456",
		Type:           comm.CommMessageType("text"),
		Content:        "Hello from A",
		Metadata:       map[string]string{"key": "value"},
		Timestamp:      1704067200,
		HumanInjected:  true,
		HumanID:        "human-789",
	}

	proto := toProtoCommMessage(original)
	roundtrip := fromProtoCommMessage(proto)

	if roundtrip.ID != original.ID {
		t.Errorf("ID = %q, want %q", roundtrip.ID, original.ID)
	}
	if roundtrip.From != original.From {
		t.Errorf("From = %q, want %q", roundtrip.From, original.From)
	}
	if roundtrip.To != original.To {
		t.Errorf("To = %q, want %q", roundtrip.To, original.To)
	}
	if roundtrip.Topic != original.Topic {
		t.Errorf("Topic = %q, want %q", roundtrip.Topic, original.Topic)
	}
	if roundtrip.ConversationID != original.ConversationID {
		t.Errorf("ConversationID = %q, want %q", roundtrip.ConversationID, original.ConversationID)
	}
	if roundtrip.Type != original.Type {
		t.Errorf("Type = %q, want %q", roundtrip.Type, original.Type)
	}
	if roundtrip.Content != original.Content {
		t.Errorf("Content = %q, want %q", roundtrip.Content, original.Content)
	}
	if roundtrip.Timestamp != original.Timestamp {
		t.Errorf("Timestamp = %d, want %d", roundtrip.Timestamp, original.Timestamp)
	}
	if roundtrip.HumanInjected != original.HumanInjected {
		t.Errorf("HumanInjected = %v, want %v", roundtrip.HumanInjected, original.HumanInjected)
	}
	if roundtrip.HumanID != original.HumanID {
		t.Errorf("HumanID = %q, want %q", roundtrip.HumanID, original.HumanID)
	}
	if roundtrip.Metadata["key"] != "value" {
		t.Errorf("Metadata[key] = %q, want 'value'", roundtrip.Metadata["key"])
	}
}
