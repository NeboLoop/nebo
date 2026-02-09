package tools

import (
	"context"
	"sync"
)

// CronScheduler adapts CronTool to the Scheduler interface.
// CronTool can't implement Scheduler directly because method names like
// Name() and Close() conflict with the Tool interface.
type CronScheduler struct {
	cron *CronTool
}

// NewCronScheduler wraps a CronTool as a Scheduler.
func NewCronScheduler(cron *CronTool) *CronScheduler {
	return &CronScheduler{cron: cron}
}

func (s *CronScheduler) Create(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	return s.cron.SchedulerCreate(ctx, item)
}

func (s *CronScheduler) Get(ctx context.Context, name string) (*ScheduleItem, error) {
	return s.cron.SchedulerGet(ctx, name)
}

func (s *CronScheduler) List(ctx context.Context, limit, offset int, enabledOnly bool) ([]ScheduleItem, int64, error) {
	return s.cron.SchedulerList(ctx, limit, offset, enabledOnly)
}

func (s *CronScheduler) Update(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	return s.cron.SchedulerUpdate(ctx, item)
}

func (s *CronScheduler) Delete(ctx context.Context, name string) error {
	return s.cron.SchedulerDelete(ctx, name)
}

func (s *CronScheduler) Enable(ctx context.Context, name string) (*ScheduleItem, error) {
	return s.cron.SchedulerEnable(ctx, name)
}

func (s *CronScheduler) Disable(ctx context.Context, name string) (*ScheduleItem, error) {
	return s.cron.SchedulerDisable(ctx, name)
}

func (s *CronScheduler) Trigger(ctx context.Context, name string) (string, error) {
	return s.cron.SchedulerTrigger(ctx, name)
}

func (s *CronScheduler) History(ctx context.Context, name string, limit, offset int) ([]ScheduleHistoryEntry, int64, error) {
	return s.cron.SchedulerHistory(ctx, name, limit, offset)
}

func (s *CronScheduler) SetTriggerHandler(fn func(ScheduleTriggerEvent)) {
	s.cron.SetTriggerHandler(fn)
}

func (s *CronScheduler) Close() error {
	return s.cron.Close()
}

// SchedulerManager delegates to the active Scheduler implementation.
// If an app provides scheduling, it is used. Otherwise, the built-in CronTool is the fallback.
type SchedulerManager struct {
	builtin Scheduler // always set (CronScheduler wrapping CronTool)
	app     Scheduler // set when a schedule app is installed; nil otherwise
	mu      sync.RWMutex
}

// NewSchedulerManager creates a manager with the built-in scheduler as fallback.
func NewSchedulerManager(builtin Scheduler) *SchedulerManager {
	return &SchedulerManager{builtin: builtin}
}

// SetAppScheduler sets the app-provided scheduler. Pass nil to revert to built-in.
func (m *SchedulerManager) SetAppScheduler(s Scheduler) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.app = s
}

// active returns the currently active scheduler (app if available, otherwise built-in).
func (m *SchedulerManager) active() Scheduler {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if m.app != nil {
		return m.app
	}
	return m.builtin
}

func (m *SchedulerManager) Create(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	return m.active().Create(ctx, item)
}

func (m *SchedulerManager) Get(ctx context.Context, name string) (*ScheduleItem, error) {
	return m.active().Get(ctx, name)
}

func (m *SchedulerManager) List(ctx context.Context, limit, offset int, enabledOnly bool) ([]ScheduleItem, int64, error) {
	return m.active().List(ctx, limit, offset, enabledOnly)
}

func (m *SchedulerManager) Update(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	return m.active().Update(ctx, item)
}

func (m *SchedulerManager) Delete(ctx context.Context, name string) error {
	return m.active().Delete(ctx, name)
}

func (m *SchedulerManager) Enable(ctx context.Context, name string) (*ScheduleItem, error) {
	return m.active().Enable(ctx, name)
}

func (m *SchedulerManager) Disable(ctx context.Context, name string) (*ScheduleItem, error) {
	return m.active().Disable(ctx, name)
}

func (m *SchedulerManager) Trigger(ctx context.Context, name string) (string, error) {
	return m.active().Trigger(ctx, name)
}

func (m *SchedulerManager) History(ctx context.Context, name string, limit, offset int) ([]ScheduleHistoryEntry, int64, error) {
	return m.active().History(ctx, name, limit, offset)
}

func (m *SchedulerManager) SetTriggerHandler(fn func(ScheduleTriggerEvent)) {
	m.active().SetTriggerHandler(fn)
}

func (m *SchedulerManager) Close() error {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if m.app != nil {
		m.app.Close()
	}
	return m.builtin.Close()
}
