package agenthub

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// Lane types for command queue
const (
	LaneMain      = "main"      // Primary user interactions
	LaneEvents    = "events"    // Scheduled/triggered tasks (renamed from cron)
	LaneSubagent  = "subagent"  // Sub-agent operations
	LaneNested    = "nested"    // Nested tool calls
	LaneHeartbeat = "heartbeat" // Proactive heartbeat ticks
	LaneComm      = "comm"      // Inter-agent communication messages
)

// DefaultLaneConcurrency defines default max concurrent tasks per lane
// 0 = unlimited
var DefaultLaneConcurrency = map[string]int{
	LaneMain:      1,
	LaneEvents:    2,  // Scheduled/triggered tasks
	LaneSubagent:  0,  // Unlimited sub-agents
	LaneNested:    3,
	LaneHeartbeat: 1,  // Sequential heartbeat processing
	LaneComm:      5,  // Concurrent comm message processing
}

// MaxLaneConcurrency defines hard limits that cannot be exceeded
// Used to prevent runaway tool calls or resource exhaustion
var MaxLaneConcurrency = map[string]int{
	LaneNested: 3, // Hard cap on concurrent tool calls
}

// LaneTask represents a task to be executed in a lane
type LaneTask struct {
	ID          string
	Lane        string
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
	Active        int
	MaxConcurrent int
	draining      bool
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
	mu    sync.RWMutex
	lanes map[string]*LaneState
}

// NewLaneManager creates a new lane manager
func NewLaneManager() *LaneManager {
	return &LaneManager{
		lanes: make(map[string]*LaneState),
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
	}
	m.lanes[lane] = state
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
	m.drain(lane)
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

	taskCtx, cancel := context.WithCancel(ctx)
	entry := &laneEntry{
		task: &LaneTask{
			ID:          fmt.Sprintf("%s-%d", lane, time.Now().UnixNano()),
			Lane:        lane,
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
	queueSize := len(state.Queue) + state.Active
	state.mu.Unlock()

	fmt.Printf("[LaneManager] Enqueued task in lane=%s queueSize=%d\n", lane, queueSize)
	m.drain(lane)

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
	go func() {
		_ = m.Enqueue(ctx, lane, task, opts...)
	}()
}

// drain processes tasks in a lane
func (m *LaneManager) drain(lane string) {
	state := m.getLaneState(lane)

	state.mu.Lock()
	if state.draining {
		state.mu.Unlock()
		return
	}
	state.draining = true
	state.mu.Unlock()

	m.pump(state)
}

func (m *LaneManager) pump(state *LaneState) {
	for {
		state.mu.Lock()
		// MaxConcurrent of 0 means unlimited
		atCapacity := state.MaxConcurrent > 0 && state.Active >= state.MaxConcurrent
		if atCapacity || len(state.Queue) == 0 {
			state.draining = false
			state.mu.Unlock()
			return
		}

		entry := state.Queue[0]
		state.Queue = state.Queue[1:]
		waitedMs := time.Since(entry.task.EnqueuedAt).Milliseconds()

		if waitedMs >= entry.task.WarnAfterMs && entry.task.OnWait != nil {
			entry.task.OnWait(waitedMs, len(state.Queue))
			fmt.Printf("[LaneManager] Lane wait exceeded: lane=%s waitedMs=%d queueAhead=%d\n",
				state.Lane, waitedMs, len(state.Queue))
		}

		state.Active++
		state.mu.Unlock()

		fmt.Printf("[LaneManager] Dequeued task in lane=%s waitedMs=%d active=%d queued=%d\n",
			state.Lane, waitedMs, state.Active, len(state.Queue))

		go func(e *laneEntry) {
			startTime := time.Now()
			e.task.StartedAt = startTime

			var err error
			func() {
				defer func() {
					if r := recover(); r != nil {
						err = fmt.Errorf("panic in lane task: %v", r)
					}
				}()
				err = e.task.Task(e.ctx)
			}()

			e.task.CompletedAt = time.Now()
			e.task.Error = err

			state.mu.Lock()
			state.Active--
			durationMs := time.Since(startTime).Milliseconds()
			activeAfter := state.Active
			queuedAfter := len(state.Queue)
			state.mu.Unlock()

			if err != nil {
				fmt.Printf("[LaneManager] Lane task error: lane=%s durationMs=%d error=%q\n",
					state.Lane, durationMs, err.Error())
			} else {
				fmt.Printf("[LaneManager] Lane task done: lane=%s durationMs=%d active=%d queued=%d\n",
					state.Lane, durationMs, activeAfter, queuedAfter)
			}

			e.resolve <- err
			close(e.resolve)

			m.pump(state)
		}(entry)
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
	return len(state.Queue) + state.Active
}

// GetTotalQueueSize returns the total number of tasks across all lanes
func (m *LaneManager) GetTotalQueueSize() int {
	m.mu.RLock()
	defer m.mu.RUnlock()

	total := 0
	for _, state := range m.lanes {
		state.mu.Lock()
		total += len(state.Queue) + state.Active
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
		stats[lane] = LaneStats{
			Lane:          lane,
			Queued:        len(state.Queue),
			Active:        state.Active,
			MaxConcurrent: state.MaxConcurrent,
		}
		state.mu.Unlock()
	}
	return stats
}

// LaneStats contains statistics for a lane
type LaneStats struct {
	Lane          string `json:"lane"`
	Queued        int    `json:"queued"`
	Active        int    `json:"active"`
	MaxConcurrent int    `json:"max_concurrent"`
}

// EnqueueOption configures enqueue behavior
type EnqueueOption func(*enqueueConfig)

type enqueueConfig struct {
	warnAfterMs int64
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
