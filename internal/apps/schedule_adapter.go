package apps

import (
	"context"
	"fmt"
	"io"
	"time"

	"github.com/neboloop/nebo/internal/agent/tools"
	pb "github.com/neboloop/nebo/internal/apps/pb"
)

// AppScheduleAdapter bridges a schedule app's gRPC client to Nebo's tools.Scheduler interface.
type AppScheduleAdapter struct {
	client  pb.ScheduleServiceClient
	handler func(tools.ScheduleTriggerEvent)
	cancel  context.CancelFunc
}

// NewAppScheduleAdapter creates a schedule adapter by health-checking the app.
func NewAppScheduleAdapter(ctx context.Context, client pb.ScheduleServiceClient) (*AppScheduleAdapter, error) {
	resp, err := client.HealthCheck(ctx, &pb.HealthCheckRequest{})
	if err != nil {
		return nil, fmt.Errorf("schedule health check: %w", err)
	}
	if !resp.Healthy {
		return nil, fmt.Errorf("schedule app is unhealthy")
	}
	return &AppScheduleAdapter{client: client}, nil
}

func (a *AppScheduleAdapter) Create(ctx context.Context, item tools.ScheduleItem) (*tools.ScheduleItem, error) {
	resp, err := a.client.Create(ctx, &pb.CreateScheduleRequest{
		Name:       item.Name,
		Expression: item.Expression,
		TaskType:   item.TaskType,
		Command:    item.Command,
		Message:    item.Message,
		Deliver:    item.Deliver,
		Metadata:   item.Metadata,
	})
	if err != nil {
		return nil, fmt.Errorf("schedule create: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("%s", resp.Error)
	}
	return protoToScheduleItem(resp.Schedule), nil
}

func (a *AppScheduleAdapter) Get(ctx context.Context, name string) (*tools.ScheduleItem, error) {
	resp, err := a.client.Get(ctx, &pb.GetScheduleRequest{Name: name})
	if err != nil {
		return nil, fmt.Errorf("schedule get: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("%s", resp.Error)
	}
	return protoToScheduleItem(resp.Schedule), nil
}

func (a *AppScheduleAdapter) List(ctx context.Context, limit, offset int, enabledOnly bool) ([]tools.ScheduleItem, int64, error) {
	resp, err := a.client.List(ctx, &pb.ListSchedulesRequest{
		Limit:       int32(limit),
		Offset:      int32(offset),
		EnabledOnly: enabledOnly,
	})
	if err != nil {
		return nil, 0, fmt.Errorf("schedule list: %w", err)
	}
	items := make([]tools.ScheduleItem, len(resp.Schedules))
	for i, s := range resp.Schedules {
		items[i] = *protoToScheduleItem(s)
	}
	return items, resp.Total, nil
}

func (a *AppScheduleAdapter) Update(ctx context.Context, item tools.ScheduleItem) (*tools.ScheduleItem, error) {
	resp, err := a.client.Update(ctx, &pb.UpdateScheduleRequest{
		Name:       item.Name,
		Expression: item.Expression,
		TaskType:   item.TaskType,
		Command:    item.Command,
		Message:    item.Message,
		Deliver:    item.Deliver,
		Metadata:   item.Metadata,
	})
	if err != nil {
		return nil, fmt.Errorf("schedule update: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("%s", resp.Error)
	}
	return protoToScheduleItem(resp.Schedule), nil
}

func (a *AppScheduleAdapter) Delete(ctx context.Context, name string) error {
	resp, err := a.client.Delete(ctx, &pb.DeleteScheduleRequest{Name: name})
	if err != nil {
		return fmt.Errorf("schedule delete: %w", err)
	}
	if resp.Error != "" {
		return fmt.Errorf("%s", resp.Error)
	}
	return nil
}

func (a *AppScheduleAdapter) Enable(ctx context.Context, name string) (*tools.ScheduleItem, error) {
	resp, err := a.client.Enable(ctx, &pb.ScheduleNameRequest{Name: name})
	if err != nil {
		return nil, fmt.Errorf("schedule enable: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("%s", resp.Error)
	}
	return protoToScheduleItem(resp.Schedule), nil
}

func (a *AppScheduleAdapter) Disable(ctx context.Context, name string) (*tools.ScheduleItem, error) {
	resp, err := a.client.Disable(ctx, &pb.ScheduleNameRequest{Name: name})
	if err != nil {
		return nil, fmt.Errorf("schedule disable: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("%s", resp.Error)
	}
	return protoToScheduleItem(resp.Schedule), nil
}

func (a *AppScheduleAdapter) Trigger(ctx context.Context, name string) (string, error) {
	resp, err := a.client.Trigger(ctx, &pb.ScheduleNameRequest{Name: name})
	if err != nil {
		return "", fmt.Errorf("schedule trigger: %w", err)
	}
	if resp.Error != "" {
		return "", fmt.Errorf("%s", resp.Error)
	}
	return resp.Output, nil
}

func (a *AppScheduleAdapter) History(ctx context.Context, name string, limit, offset int) ([]tools.ScheduleHistoryEntry, int64, error) {
	resp, err := a.client.History(ctx, &pb.ScheduleHistoryRequest{
		Name:   name,
		Limit:  int32(limit),
		Offset: int32(offset),
	})
	if err != nil {
		return nil, 0, fmt.Errorf("schedule history: %w", err)
	}
	entries := make([]tools.ScheduleHistoryEntry, len(resp.Entries))
	for i, e := range resp.Entries {
		entry := tools.ScheduleHistoryEntry{
			ID:           e.Id,
			ScheduleName: e.ScheduleName,
			Success:      e.Success,
			Output:       e.Output,
			Error:        e.Error,
		}
		if e.StartedAt != "" {
			entry.StartedAt, _ = time.Parse(time.RFC3339, e.StartedAt)
		}
		if e.FinishedAt != "" {
			entry.FinishedAt, _ = time.Parse(time.RFC3339, e.FinishedAt)
		}
		entries[i] = entry
	}
	return entries, resp.Total, nil
}

// SetTriggerHandler sets the callback and starts reading the trigger stream.
func (a *AppScheduleAdapter) SetTriggerHandler(fn func(tools.ScheduleTriggerEvent)) {
	a.handler = fn

	// Start background goroutine to read trigger stream
	ctx, cancel := context.WithCancel(context.Background())
	a.cancel = cancel

	go func() {
		stream, err := a.client.Triggers(ctx, &pb.Empty{})
		if err != nil {
			fmt.Printf("[apps:schedule] Trigger stream failed: %v\n", err)
			return
		}
		for {
			trigger, err := stream.Recv()
			if err != nil {
				if err != io.EOF && ctx.Err() == nil {
					fmt.Printf("[apps:schedule] Trigger stream error: %v\n", err)
				}
				return
			}
			if a.handler != nil {
				event := tools.ScheduleTriggerEvent{
					ScheduleID: trigger.ScheduleId,
					Name:       trigger.Name,
					TaskType:   trigger.TaskType,
					Command:    trigger.Command,
					Message:    trigger.Message,
					Deliver:    trigger.Deliver,
					Metadata:   trigger.Metadata,
				}
				if trigger.FiredAt != "" {
					event.FiredAt, _ = time.Parse(time.RFC3339, trigger.FiredAt)
				} else {
					event.FiredAt = time.Now()
				}
				a.handler(event)
			}
		}
	}()
}

func (a *AppScheduleAdapter) Close() error {
	if a.cancel != nil {
		a.cancel()
	}
	return nil
}

// protoToScheduleItem converts a proto Schedule to a tools.ScheduleItem.
func protoToScheduleItem(s *pb.Schedule) *tools.ScheduleItem {
	if s == nil {
		return &tools.ScheduleItem{}
	}
	item := &tools.ScheduleItem{
		ID:         s.Id,
		Name:       s.Name,
		Expression: s.Expression,
		TaskType:   s.TaskType,
		Command:    s.Command,
		Message:    s.Message,
		Deliver:    s.Deliver,
		Enabled:    s.Enabled,
		RunCount:   s.RunCount,
		LastError:  s.LastError,
		Metadata:   s.Metadata,
	}
	if s.LastRun != "" {
		item.LastRun, _ = time.Parse(time.RFC3339, s.LastRun)
	}
	if s.NextRun != "" {
		item.NextRun, _ = time.Parse(time.RFC3339, s.NextRun)
	}
	if s.CreatedAt != "" {
		item.CreatedAt, _ = time.Parse(time.RFC3339, s.CreatedAt)
	}
	return item
}
