package agenthub

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/crashlog"
)

// Lane types for command queue
const (
	LaneMain      = "main"      // Primary user interactions (phone + desktop + voice)
	LaneEvents    = "events"    // Scheduled/triggered tasks (renamed from cron)
	LaneSubagent  = "subagent"  // Sub-agent operations
	LaneNested    = "nested"    // Nested tool calls
	LaneHeartbeat = "heartbeat" // Proactive heartbeat ticks
	LaneComm      = "comm"      // Inter-agent communication messages
	LaneDev       = "dev"       // Developer assistant (independent of main lane)
	LaneDesktop   = "desktop"   // Serialized desktop automation (one screen, one mouse)
)

// laneContextKey is an unexported type for the lane context key.
type laneContextKey struct{}

// WithLane returns a new context carrying the lane name.
func WithLane(ctx context.Context, lane string) context.Context {
	return context.WithValue(ctx, laneContextKey{}, lane)
}

// GetLane extracts the lane name from a context.
// Returns empty string if no lane is set.
func GetLane(ctx context.Context) string {
	if lane, ok := ctx.Value(laneContextKey{}).(string); ok {
		return lane
	}
	return ""
}

// DefaultLaneConcurrency defines default max concurrent tasks per lane
// 0 = unlimited
var DefaultLaneConcurrency = map[string]int{
	LaneMain:      2,  // Concurrent phone + desktop + voice input on same session
	LaneEvents:    0,  // Scheduled/triggered tasks (unlimited — each gets own session)
	LaneSubagent:  5,  // Max 5 concurrent sub-agents (backpressure)
	LaneNested:    3,
	LaneHeartbeat: 1,  // Sequential heartbeat processing
	LaneComm:      5,  // Concurrent comm message processing
	LaneDev:       1,  // Developer assistant (serialized per project)
	LaneDesktop:   1,  // One screen, one mouse, one keyboard — strictly serialized
}

// MaxLaneConcurrency defines hard limits that cannot be exceeded
// Used to prevent runaway tool calls or resource exhaustion
var MaxLaneConcurrency = map[string]int{
	LaneNested:   3,  // Hard cap on concurrent tool calls
	LaneSubagent: 10, // Hard cap on concurrent sub-agents (prevents runaway spawning)
}

// LaneTask represents a task to be executed in a lane
type LaneTask struct {
	ID          string
	Lane        string
	Description string
	Task        func(ctx context.Context) error
	EnqueuedAt  time.Time
	StartedAt   time.Time
	CompletedAt time.Time
	Error       error
	OnWait      func(waitMs int64, queuedAhead int)
	WarnAfterMs int64
}

// LaneState tracks the state of a single lane
type LaneState struct {
	Lane          string
	Queue         []*laneEntry
	active        []*laneEntry
	MaxConcurrent int
	notify        chan struct{} // buffered(1) — wakeup signal for pump goroutine
	stopCh        chan struct{} // close to stop pump goroutine
	mu            sync.Mutex
}

type laneEntry struct {
	task    *LaneTask
	resolve chan error
	ctx     context.Context
	cancel  context.CancelFunc
}

// LaneManager manages multiple lanes for command execution
type LaneManager struct {
	mu      sync.RWMutex
	lanes   map[string]*LaneState
	onEvent func(LaneEvent)
}

// NewLaneManager creates a new lane manager
func NewLaneManager() *LaneManager {
	return &LaneManager{
		lanes: make(map[string]*LaneState),
	}
}

// OnEvent registers a callback for lane lifecycle events
func (m *LaneManager) OnEvent(fn func(LaneEvent)) {
	m.onEvent = fn
}

func (m *LaneManager) emit(event LaneEvent) {
	if fn := m.onEvent; fn != nil {
		go fn(event)
	}
}

// getLaneState returns or creates a lane state
func (m *LaneManager) getLaneState(lane string) *LaneState {
	m.mu.Lock()
	defer m.mu.Unlock()

	if state, ok := m.lanes[lane]; ok {
		return state
	}

	maxConcurrent := 1
	if mc, ok := DefaultLaneConcurrency[lane]; ok {
		maxConcurrent = mc
	}

	state := &LaneState{
		Lane:          lane,
		Queue:         make([]*laneEntry, 0),
		MaxConcurrent: maxConcurrent,
		notify:        make(chan struct{}, 1),
		stopCh:        make(chan struct{}),
	}
	m.lanes[lane] = state
	go state.run(m)
	return state
}

// SetConcurrency sets the max concurrency for a lane
// 0 = unlimited, any positive number = max concurrent tasks
// Hard limits in MaxLaneConcurrency cannot be exceeded
func (m *LaneManager) SetConcurrency(lane string, maxConcurrent int) {
	state := m.getLaneState(lane)
	state.mu.Lock()
	if maxConcurrent < 0 {
		maxConcurrent = 0 // Treat negative as unlimited
	}
	// Enforce hard caps for lanes that have them
	if hardCap, ok := MaxLaneConcurrency[lane]; ok {
		if maxConcurrent == 0 || maxConcurrent > hardCap {
			maxConcurrent = hardCap
		}
	}
	state.MaxConcurrent = maxConcurrent
	state.mu.Unlock()
	select {
	case state.notify <- struct{}{}:
	default:
	}
}

// Enqueue adds a task to a lane and returns when it completes
func (m *LaneManager) Enqueue(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption) error {
	if lane == "" {
		lane = LaneMain
	}

	cfg := &enqueueConfig{
		warnAfterMs: 2000,
	}
	for _, opt := range opts {
		opt(cfg)
	}

	state := m.getLaneState(lane)

	taskCtx, cancel := context.WithCancel(WithLane(ctx, lane))
	entry := &laneEntry{
		task: &LaneTask{
			ID:          fmt.Sprintf("%s-%d", lane, time.Now().UnixNano()),
			Lane:        lane,
			Description: cfg.description,
			Task:        task,
			EnqueuedAt:  time.Now(),
			OnWait:      cfg.onWait,
			WarnAfterMs: cfg.warnAfterMs,
		},
		resolve: make(chan error, 1),
		ctx:     taskCtx,
		cancel:  cancel,
	}

	state.mu.Lock()
	state.Queue = append(state.Queue, entry)
	queueSize := len(state.Queue) + len(state.active)
	info := entryToInfo(entry)
	state.mu.Unlock()

	fmt.Printf("[LaneManager] Enqueued task in lane=%s queueSize=%d\n", lane, queueSize)
	m.emit(LaneEvent{Type: "task_enqueued", Lane: lane, Task: info})
	select {
	case state.notify <- struct{}{}:
	default:
	}

	select {
	case err := <-entry.resolve:
		return err
	case <-ctx.Done():
		cancel()
		return ctx.Err()
	}
}

// EnqueueAsync adds a task to a lane without waiting for completion
func (m *LaneManager) EnqueueAsync(ctx context.Context, lane string, task func(ctx context.Context) error, opts ...EnqueueOption) {
	if lane == "" {
		lane = LaneMain
	}

	cfg := &enqueueConfig{
		warnAfterMs: 2000,
	}
	for _, opt := range opts {
		opt(cfg)
	}

	state := m.getLaneState(lane)

	taskCtx, cancel := context.WithCancel(WithLane(ctx, lane))
	entry := &laneEntry{
		task: &LaneTask{
			ID:          fmt.Sprintf("%s-%d", lane, time.Now().UnixNano()),
			Lane:        lane,
			Description: cfg.description,
			Task:        task,
			EnqueuedAt:  time.Now(),
			OnWait:      cfg.onWait,
			WarnAfterMs: cfg.warnAfterMs,
		},
		resolve: make(chan error, 1),
		ctx:     taskCtx,
		cancel:  cancel,
	}

	state.mu.Lock()
	state.Queue = append(state.Queue, entry)
	queueSize := len(state.Queue) + len(state.active)
	info := entryToInfo(entry)
	state.mu.Unlock()

	fmt.Printf("[LaneManager] EnqueueAsync task in lane=%s queueSize=%d\n", lane, queueSize)
	m.emit(LaneEvent{Type: "task_enqueued", Lane: lane, Task: info})
	select {
	case state.notify <- struct{}{}:
	default:
	}
}

// run is the pump goroutine for a lane. It waits for wakeup signals
// on the notify channel and processes available tasks.
func (s *LaneState) run(mgr *LaneManager) {
	for {
		select {
		case <-s.notify:
			s.processAvailable(mgr)
		case <-s.stopCh:
			return
		}
	}
}

// processAvailable dequeues and starts all runnable tasks (up to capacity).
func (s *LaneState) processAvailable(mgr *LaneManager) {
	for {
		s.mu.Lock()
		// MaxConcurrent of 0 means unlimited
		atCapacity := s.MaxConcurrent > 0 && len(s.active) >= s.MaxConcurrent
		if atCapacity || len(s.Queue) == 0 {
			s.mu.Unlock()
			return
		}

		entry := s.Queue[0]
		s.Queue = s.Queue[1:]
		waitedMs := time.Since(entry.task.EnqueuedAt).Milliseconds()

		if waitedMs >= entry.task.WarnAfterMs && entry.task.OnWait != nil {
			entry.task.OnWait(waitedMs, len(s.Queue))
			fmt.Printf("[LaneManager] Lane wait exceeded: lane=%s waitedMs=%d queueAhead=%d\n",
				s.Lane, waitedMs, len(s.Queue))
		}

		s.active = append(s.active, entry)
		entry.task.StartedAt = time.Now()
		startedInfo := entryToInfo(entry)
		activeCount := len(s.active)
		queuedCount := len(s.Queue)
		s.mu.Unlock()

		fmt.Printf("[LaneManager] Dequeued task in lane=%s waitedMs=%d active=%d queued=%d\n",
			s.Lane, waitedMs, activeCount, queuedCount)

		go func(e *laneEntry, startInfo LaneTaskInfo) {
			startTime := e.task.StartedAt
			mgr.emit(LaneEvent{Type: "task_started", Lane: s.Lane, Task: startInfo})

			var err error
			func() {
				defer func() {
					if r := recover(); r != nil {
						crashlog.LogPanic("lane", r, map[string]string{"lane": string(s.Lane)})
						err = fmt.Errorf("panic in lane task: %v", r)
					}
				}()
				err = e.task.Task(e.ctx)
			}()

			s.mu.Lock()
			e.task.CompletedAt = time.Now()
			e.task.Error = err
			// Remove this entry from the active slice
			for i, a := range s.active {
				if a == e {
					s.active = append(s.active[:i], s.active[i+1:]...)
					break
				}
			}
			durationMs := time.Since(startTime).Milliseconds()
			activeAfter := len(s.active)
			queuedAfter := len(s.Queue)
			completedInfo := entryToInfo(e)
			s.mu.Unlock()

			if err != nil {
				fmt.Printf("[LaneManager] Lane task error: lane=%s durationMs=%d error=%q\n",
					s.Lane, durationMs, err.Error())
			} else {
				fmt.Printf("[LaneManager] Lane task done: lane=%s durationMs=%d active=%d queued=%d\n",
					s.Lane, durationMs, activeAfter, queuedAfter)
			}
			mgr.emit(LaneEvent{Type: "task_completed", Lane: s.Lane, Task: completedInfo})

			e.resolve <- err
			close(e.resolve)

			// Signal pump that capacity freed up
			select {
			case s.notify <- struct{}{}:
			default:
			}
		}(entry, startedInfo)
	}
}

// GetQueueSize returns the number of tasks in a lane (queued + active)
func (m *LaneManager) GetQueueSize(lane string) int {
	if lane == "" {
		lane = LaneMain
	}
	m.mu.RLock()
	state, ok := m.lanes[lane]
	m.mu.RUnlock()
	if !ok {
		return 0
	}
	state.mu.Lock()
	defer state.mu.Unlock()
	return len(state.Queue) + len(state.active)
}

// GetTotalQueueSize returns the total number of tasks across all lanes
func (m *LaneManager) GetTotalQueueSize() int {
	m.mu.RLock()
	defer m.mu.RUnlock()

	total := 0
	for _, state := range m.lanes {
		state.mu.Lock()
		total += len(state.Queue) + len(state.active)
		state.mu.Unlock()
	}
	return total
}

// ClearLane removes all queued (not active) tasks from a lane
func (m *LaneManager) ClearLane(lane string) int {
	if lane == "" {
		lane = LaneMain
	}
	m.mu.RLock()
	state, ok := m.lanes[lane]
	m.mu.RUnlock()
	if !ok {
		return 0
	}

	state.mu.Lock()
	defer state.mu.Unlock()

	removed := len(state.Queue)
	for _, entry := range state.Queue {
		entry.cancel()
		entry.resolve <- context.Canceled
		close(entry.resolve)
	}
	state.Queue = make([]*laneEntry, 0)
	return removed
}

// GetLaneStats returns statistics for all lanes
func (m *LaneManager) GetLaneStats() map[string]LaneStats {
	m.mu.RLock()
	defer m.mu.RUnlock()

	stats := make(map[string]LaneStats)
	for lane, state := range m.lanes {
		state.mu.Lock()
		ls := LaneStats{
			Lane:          lane,
			Queued:        len(state.Queue),
			Active:        len(state.active),
			MaxConcurrent: state.MaxConcurrent,
		}
		for _, e := range state.active {
			ls.ActiveTasks = append(ls.ActiveTasks, entryToInfo(e))
		}
		for _, e := range state.Queue {
			ls.QueuedTasks = append(ls.QueuedTasks, entryToInfo(e))
		}
		stats[lane] = ls
		state.mu.Unlock()
	}
	return stats
}

func entryToInfo(e *laneEntry) LaneTaskInfo {
	info := LaneTaskInfo{
		ID:          e.task.ID,
		Description: e.task.Description,
		EnqueuedAt:  e.task.EnqueuedAt.UnixMilli(),
	}
	if !e.task.StartedAt.IsZero() {
		info.StartedAt = e.task.StartedAt.UnixMilli()
	}
	return info
}

// LaneStats contains statistics for a lane
type LaneStats struct {
	Lane          string         `json:"lane"`
	Queued        int            `json:"queued"`
	Active        int            `json:"active"`
	MaxConcurrent int            `json:"max_concurrent"`
	ActiveTasks   []LaneTaskInfo `json:"active_tasks,omitempty"`
	QueuedTasks   []LaneTaskInfo `json:"queued_tasks,omitempty"`
}

// LaneTaskInfo represents a summary of a task in a lane
type LaneTaskInfo struct {
	ID          string `json:"id"`
	Description string `json:"description"`
	EnqueuedAt  int64  `json:"enqueued_at"`
	StartedAt   int64  `json:"started_at,omitempty"`
}

// LaneEvent represents a lifecycle event for a lane task
type LaneEvent struct {
	Type string       `json:"type"` // task_enqueued, task_started, task_completed, task_cancelled
	Lane string       `json:"lane"`
	Task LaneTaskInfo `json:"task"`
}

// CancelActive cancels all active tasks in a lane by calling their cancel functions.
// Returns the number of tasks cancelled.
func (m *LaneManager) CancelActive(lane string) int {
	if lane == "" {
		lane = LaneMain
	}
	m.mu.RLock()
	state, ok := m.lanes[lane]
	m.mu.RUnlock()
	if !ok {
		return 0
	}

	state.mu.Lock()
	cancelled := len(state.active)
	for _, entry := range state.active {
		m.emit(LaneEvent{Type: "task_cancelled", Lane: lane, Task: entryToInfo(entry)})
		entry.cancel()
	}
	state.mu.Unlock()

	return cancelled
}

// Shutdown stops all pump goroutines. Call on application shutdown.
func (m *LaneManager) Shutdown() {
	m.mu.RLock()
	defer m.mu.RUnlock()
	for _, state := range m.lanes {
		select {
		case <-state.stopCh:
			// already closed
		default:
			close(state.stopCh)
		}
	}
}

// EnqueueOption configures enqueue behavior
type EnqueueOption func(*enqueueConfig)

type enqueueConfig struct {
	warnAfterMs int64
	description string
	onWait      func(waitMs int64, queuedAhead int)
}

// WithWarnAfter sets the time after which to warn about waiting
func WithWarnAfter(ms int64) EnqueueOption {
	return func(c *enqueueConfig) {
		c.warnAfterMs = ms
	}
}

// WithOnWait sets a callback for when a task has to wait
func WithOnWait(fn func(waitMs int64, queuedAhead int)) EnqueueOption {
	return func(c *enqueueConfig) {
		c.onWait = fn
	}
}

// WithDescription sets a human-readable description for the task
func WithDescription(desc string) EnqueueOption {
	return func(c *enqueueConfig) {
		c.description = desc
	}
}
