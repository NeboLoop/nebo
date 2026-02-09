package tools

import (
	"context"
	"time"
)

// Scheduler is the interface for schedule management.
// Implemented by the built-in CronTool (wrapping robfig/cron) and by
// AppScheduleAdapter (gRPC proxy to a schedule app).
type Scheduler interface {
	// Create creates a new schedule. Returns the created item.
	Create(ctx context.Context, item ScheduleItem) (*ScheduleItem, error)

	// Get returns a schedule by name.
	Get(ctx context.Context, name string) (*ScheduleItem, error)

	// List returns schedules with optional pagination.
	// Returns items, total count, and error.
	List(ctx context.Context, limit, offset int, enabledOnly bool) ([]ScheduleItem, int64, error)

	// Update modifies an existing schedule.
	Update(ctx context.Context, item ScheduleItem) (*ScheduleItem, error)

	// Delete removes a schedule by name.
	Delete(ctx context.Context, name string) error

	// Enable activates a schedule.
	Enable(ctx context.Context, name string) (*ScheduleItem, error)

	// Disable deactivates a schedule without deleting it.
	Disable(ctx context.Context, name string) (*ScheduleItem, error)

	// Trigger manually fires a schedule immediately. Returns output.
	Trigger(ctx context.Context, name string) (string, error)

	// History returns execution history for a schedule.
	// Returns entries, total count, and error.
	History(ctx context.Context, name string, limit, offset int) ([]ScheduleHistoryEntry, int64, error)

	// SetTriggerHandler sets the callback invoked when a schedule fires.
	// The built-in scheduler calls this from robfig/cron's timer.
	// App schedulers call this from the gRPC trigger stream goroutine.
	SetTriggerHandler(fn func(ScheduleTriggerEvent))

	// Close stops the scheduler and cleans up resources.
	Close() error
}

// ScheduleItem represents a single scheduled task.
type ScheduleItem struct {
	ID         string            `json:"id"`
	Name       string            `json:"name"`
	Expression string            `json:"expression"`
	TaskType   string            `json:"task_type"`
	Command    string            `json:"command,omitempty"`
	Message    string            `json:"message,omitempty"`
	Deliver    string            `json:"deliver,omitempty"`
	Enabled    bool              `json:"enabled"`
	LastRun    time.Time         `json:"last_run,omitempty"`
	NextRun    time.Time         `json:"next_run,omitempty"`
	RunCount   int64             `json:"run_count"`
	LastError  string            `json:"last_error,omitempty"`
	CreatedAt  time.Time         `json:"created_at"`
	Metadata   map[string]string `json:"metadata,omitempty"`
}

// ScheduleHistoryEntry represents one execution of a schedule.
type ScheduleHistoryEntry struct {
	ID           string    `json:"id"`
	ScheduleName string    `json:"schedule_name"`
	StartedAt    time.Time `json:"started_at"`
	FinishedAt   time.Time `json:"finished_at,omitempty"`
	Success      bool      `json:"success"`
	Output       string    `json:"output,omitempty"`
	Error        string    `json:"error,omitempty"`
}

// ScheduleTriggerEvent is emitted when a schedule fires.
// Contains enough info for Nebo to route to the correct lane.
type ScheduleTriggerEvent struct {
	ScheduleID string            `json:"schedule_id"`
	Name       string            `json:"name"`
	TaskType   string            `json:"task_type"`
	Command    string            `json:"command,omitempty"`
	Message    string            `json:"message,omitempty"`
	Deliver    string            `json:"deliver,omitempty"`
	FiredAt    time.Time         `json:"fired_at"`
	Metadata   map[string]string `json:"metadata,omitempty"`
}
