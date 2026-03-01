package apps

import (
	"context"
	"testing"
	"time"

	pb "github.com/neboloop/nebo/internal/apps/pb"
	"google.golang.org/grpc"
)

// mockHookClient implements pb.HookServiceClient for testing.
type mockHookClient struct {
	applyFilterFn func(ctx context.Context, in *pb.HookRequest) (*pb.HookResponse, error)
	doActionFn    func(ctx context.Context, in *pb.HookRequest) (*pb.Empty, error)
}

func (m *mockHookClient) ApplyFilter(ctx context.Context, in *pb.HookRequest, opts ...grpc.CallOption) (*pb.HookResponse, error) {
	if m.applyFilterFn != nil {
		return m.applyFilterFn(ctx, in)
	}
	return &pb.HookResponse{Payload: in.Payload}, nil
}

func (m *mockHookClient) DoAction(ctx context.Context, in *pb.HookRequest, opts ...grpc.CallOption) (*pb.Empty, error) {
	if m.doActionFn != nil {
		return m.doActionFn(ctx, in)
	}
	return &pb.Empty{}, nil
}

func (m *mockHookClient) ListHooks(ctx context.Context, in *pb.Empty, opts ...grpc.CallOption) (*pb.HookList, error) {
	return &pb.HookList{}, nil
}

func (m *mockHookClient) HealthCheck(ctx context.Context, in *pb.HealthCheckRequest, opts ...grpc.CallOption) (*pb.HealthCheckResponse, error) {
	return &pb.HealthCheckResponse{Healthy: true}, nil
}

func TestHookDispatcher_PriorityOrdering(t *testing.T) {
	d := NewHookDispatcher()

	var callOrder []string

	// Register three apps with different priorities
	clientA := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callOrder = append(callOrder, "A")
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}
	clientB := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callOrder = append(callOrder, "B")
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}
	clientC := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callOrder = append(callOrder, "C")
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}

	d.Register("app-c", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 20}, clientC)
	d.Register("app-a", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 5}, clientA)
	d.Register("app-b", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 10}, clientB)

	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))

	if len(callOrder) != 3 {
		t.Fatalf("expected 3 calls, got %d", len(callOrder))
	}
	if callOrder[0] != "A" || callOrder[1] != "B" || callOrder[2] != "C" {
		t.Errorf("expected call order [A, B, C], got %v", callOrder)
	}
}

func TestHookDispatcher_FilterChaining(t *testing.T) {
	d := NewHookDispatcher()

	// First filter adds "step1" to payload
	client1 := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			return &pb.HookResponse{Payload: []byte(`{"data":"step1"}`)}, nil
		},
	}
	// Second filter receives output of first
	var receivedPayload string
	client2 := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			receivedPayload = string(in.Payload)
			return &pb.HookResponse{Payload: []byte(`{"data":"step2"}`)}, nil
		},
	}

	d.Register("app-1", &pb.HookRegistration{Hook: "tool.post_execute", Type: "filter", Priority: 5}, client1)
	d.Register("app-2", &pb.HookRegistration{Hook: "tool.post_execute", Type: "filter", Priority: 10}, client2)

	result, handled := d.ApplyFilter(context.Background(), "tool.post_execute", []byte(`{"data":"original"}`))
	if handled {
		t.Error("expected handled=false")
	}
	if receivedPayload != `{"data":"step1"}` {
		t.Errorf("second filter should receive first filter's output, got: %s", receivedPayload)
	}
	if string(result) != `{"data":"step2"}` {
		t.Errorf("expected final output from second filter, got: %s", string(result))
	}
}

func TestHookDispatcher_Override(t *testing.T) {
	d := NewHookDispatcher()

	// First filter returns handled=true (override)
	client1 := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			return &pb.HookResponse{Payload: []byte(`{"overridden":true}`), Handled: true}, nil
		},
	}
	// Second filter should NOT be called
	called := false
	client2 := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			called = true
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}

	d.Register("app-1", &pb.HookRegistration{Hook: "memory.pre_store", Type: "filter", Priority: 5}, client1)
	d.Register("app-2", &pb.HookRegistration{Hook: "memory.pre_store", Type: "filter", Priority: 10}, client2)

	result, handled := d.ApplyFilter(context.Background(), "memory.pre_store", []byte(`{"key":"test"}`))
	if !handled {
		t.Error("expected handled=true")
	}
	if string(result) != `{"overridden":true}` {
		t.Errorf("expected override result, got: %s", string(result))
	}
	if called {
		t.Error("second filter should not be called after override")
	}
}

func TestHookDispatcher_CircuitBreaker(t *testing.T) {
	d := NewHookDispatcher()

	callCount := 0
	client := &mockHookClient{
		applyFilterFn: func(ctx context.Context, _ *pb.HookRequest) (*pb.HookResponse, error) {
			callCount++
			// Simulate timeout by blocking until context expires
			<-ctx.Done()
			return nil, ctx.Err()
		},
	}

	d.Register("app-slow", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 10}, client)

	// First 3 calls should fail with timeout (circuit breaker threshold)
	for i := 0; i < circuitBreakerThreshold; i++ {
		d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))
	}

	if !d.isDisabled("app-slow") {
		t.Error("expected app to be disabled after threshold failures")
	}

	// Reset call count
	callCount = 0

	// Next call should be skipped (circuit breaker tripped)
	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))
	if callCount != 0 {
		t.Errorf("expected 0 calls after circuit breaker, got %d", callCount)
	}
}

func TestHookDispatcher_Deregistration(t *testing.T) {
	d := NewHookDispatcher()

	client := &mockHookClient{}
	d.Register("app-1", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 10}, client)
	d.Register("app-1", &pb.HookRegistration{Hook: "memory.pre_store", Type: "action", Priority: 10}, client)
	d.Register("app-2", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 5}, client)

	if !d.HasSubscribers("tool.pre_execute") {
		t.Error("expected subscribers for tool.pre_execute")
	}
	if !d.HasSubscribers("memory.pre_store") {
		t.Error("expected subscribers for memory.pre_store")
	}

	d.UnregisterApp("app-1")

	// tool.pre_execute should still have app-2
	if !d.HasSubscribers("tool.pre_execute") {
		t.Error("expected subscribers for tool.pre_execute after removing app-1 (app-2 still there)")
	}
	// memory.pre_store should have no subscribers
	if d.HasSubscribers("memory.pre_store") {
		t.Error("expected no subscribers for memory.pre_store after removing app-1")
	}
}

func TestHookDispatcher_NoSubscribers(t *testing.T) {
	d := NewHookDispatcher()

	result, handled := d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{"original":true}`))
	if handled {
		t.Error("expected handled=false with no subscribers")
	}
	if string(result) != `{"original":true}` {
		t.Errorf("expected original payload unchanged, got: %s", string(result))
	}
}

func TestHookDispatcher_ActionDoesNotReturnResult(t *testing.T) {
	d := NewHookDispatcher()

	actionCalled := false
	client := &mockHookClient{
		doActionFn: func(_ context.Context, _ *pb.HookRequest) (*pb.Empty, error) {
			actionCalled = true
			return &pb.Empty{}, nil
		},
	}

	d.Register("app-1", &pb.HookRegistration{Hook: "session.message_append", Type: "action", Priority: 10}, client)

	d.DoAction(context.Background(), "session.message_append", []byte(`{"session_id":"test"}`))
	if !actionCalled {
		t.Error("expected action to be called")
	}
}

func TestHookDispatcher_SuccessResetsFailureCount(t *testing.T) {
	d := NewHookDispatcher()

	callNum := 0
	client := &mockHookClient{
		applyFilterFn: func(ctx context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callNum++
			if callNum <= 2 {
				// First two calls timeout
				<-ctx.Done()
				return nil, ctx.Err()
			}
			// Third call succeeds
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}

	d.Register("app-1", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 10}, client)

	// Two failures
	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))
	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))

	// Failure count should be 2, not yet tripped
	if d.isDisabled("app-1") {
		t.Error("should not be disabled after 2 failures")
	}

	// Third call succeeds — should reset counter
	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))

	d.mu.RLock()
	count := d.failures["app-1"]
	d.mu.RUnlock()
	if count != 0 {
		t.Errorf("expected failure count reset to 0 after success, got %d", count)
	}
}

func TestHookDispatcher_InvalidHookName(t *testing.T) {
	d := NewHookDispatcher()
	client := &mockHookClient{}

	// Should be silently ignored
	d.Register("app-1", &pb.HookRegistration{Hook: "invalid.hook.name", Type: "filter", Priority: 10}, client)

	if d.HasSubscribers("invalid.hook.name") {
		t.Error("invalid hook name should not be registered")
	}
}

func TestHookDispatcher_DefaultPriority(t *testing.T) {
	d := NewHookDispatcher()

	var callOrder []string
	clientA := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callOrder = append(callOrder, "A")
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}
	clientB := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			callOrder = append(callOrder, "B")
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}

	// Priority 0 should default to 10
	d.Register("app-a", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 0}, clientA)
	d.Register("app-b", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 5}, clientB)

	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))

	if len(callOrder) != 2 {
		t.Fatalf("expected 2 calls, got %d", len(callOrder))
	}
	// B (priority 5) should run before A (default priority 10)
	if callOrder[0] != "B" || callOrder[1] != "A" {
		t.Errorf("expected call order [B, A], got %v", callOrder)
	}
}

func TestHookDispatcher_FilterSkipsActionEntries(t *testing.T) {
	d := NewHookDispatcher()

	filterCalled := false
	actionCalled := false

	filterClient := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			filterCalled = true
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}
	actionClient := &mockHookClient{
		applyFilterFn: func(_ context.Context, in *pb.HookRequest) (*pb.HookResponse, error) {
			actionCalled = true
			return &pb.HookResponse{Payload: in.Payload}, nil
		},
	}

	d.Register("app-filter", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 5}, filterClient)
	d.Register("app-action", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "action", Priority: 10}, actionClient)

	d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{}`))

	if !filterCalled {
		t.Error("filter should be called by ApplyFilter")
	}
	if actionCalled {
		t.Error("action entries should be skipped by ApplyFilter")
	}
}

func TestHookDispatcher_ErroredHookIsSkipped(t *testing.T) {
	d := NewHookDispatcher()

	client := &mockHookClient{
		applyFilterFn: func(_ context.Context, _ *pb.HookRequest) (*pb.HookResponse, error) {
			return &pb.HookResponse{Error: "something went wrong"}, nil
		},
	}

	d.Register("app-1", &pb.HookRegistration{Hook: "tool.pre_execute", Type: "filter", Priority: 10}, client)

	result, handled := d.ApplyFilter(context.Background(), "tool.pre_execute", []byte(`{"original":true}`))
	if handled {
		t.Error("expected handled=false when hook returns error")
	}
	// Original payload should be returned since the hook errored
	if string(result) != `{"original":true}` {
		t.Errorf("expected original payload on error, got: %s", string(result))
	}
}

// Ensure hookTimeout is reasonable
func TestHookTimeout(t *testing.T) {
	if hookTimeout != 500*time.Millisecond {
		t.Errorf("expected hook timeout to be 500ms, got %v", hookTimeout)
	}
}
