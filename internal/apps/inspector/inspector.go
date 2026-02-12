package inspector

import (
	"encoding/json"
	"sync"
	"sync/atomic"
	"time"
)

// Event represents a single captured gRPC call or stream message.
type Event struct {
	ID        uint64          `json:"id"`
	Timestamp time.Time       `json:"timestamp"`
	AppID     string          `json:"appId"`
	Method    string          `json:"method"`
	Type      string          `json:"type"`      // "unary", "stream_send", "stream_recv", "stream_open"
	Direction string          `json:"direction"` // "request" or "response"
	Payload   json.RawMessage `json:"payload"`
	DurationMs int64          `json:"durationMs,omitempty"`
	Error     string          `json:"error,omitempty"`
	StreamSeq int             `json:"streamSeq,omitempty"`
}

// Inspector is the central hub for gRPC traffic inspection.
// It stores events in a bounded ring buffer and fans out to SSE subscribers.
type Inspector struct {
	mu             sync.RWMutex
	ring           []*Event
	ringSize       int
	ringPos        int
	counter        atomic.Uint64
	subscribers    map[uint64]chan *Event
	subIDCounter   atomic.Uint64
	hasSubscribers atomic.Int32
}

// New creates a new Inspector with the given ring buffer size.
func New(ringSize int) *Inspector {
	if ringSize <= 0 {
		ringSize = 1024
	}
	return &Inspector{
		ring:        make([]*Event, ringSize),
		ringSize:    ringSize,
		subscribers: make(map[uint64]chan *Event),
	}
}

// Record adds an event to the ring buffer and notifies all subscribers.
func (ins *Inspector) Record(e *Event) {
	e.ID = ins.counter.Add(1)

	ins.mu.Lock()
	ins.ring[ins.ringPos%ins.ringSize] = e
	ins.ringPos++

	// Fan out to subscribers (non-blocking send)
	for _, ch := range ins.subscribers {
		select {
		case ch <- e:
		default:
			// subscriber too slow, drop event
		}
	}
	ins.mu.Unlock()
}

// Subscribe returns a channel that receives future events and an unsubscribe function.
func (ins *Inspector) Subscribe() (<-chan *Event, func()) {
	ch := make(chan *Event, 128)
	id := ins.subIDCounter.Add(1)

	ins.mu.Lock()
	ins.subscribers[id] = ch
	ins.hasSubscribers.Store(int32(len(ins.subscribers)))
	ins.mu.Unlock()

	return ch, func() {
		ins.mu.Lock()
		delete(ins.subscribers, id)
		ins.hasSubscribers.Store(int32(len(ins.subscribers)))
		close(ch)
		ins.mu.Unlock()
	}
}

// HasSubscribers returns true if anyone is listening. Used as a fast-path check
// by interceptors to avoid serialization overhead when no inspector is open.
func (ins *Inspector) HasSubscribers() bool {
	return ins.hasSubscribers.Load() > 0
}

// Recent returns up to n most recent events for the given appID (chronological order).
// If appID is empty, returns events for all apps.
func (ins *Inspector) Recent(appID string, n int) []*Event {
	ins.mu.RLock()
	defer ins.mu.RUnlock()

	result := make([]*Event, 0, n)
	start := ins.ringPos - 1
	for i := 0; i < ins.ringSize && len(result) < n; i++ {
		idx := (start - i) % ins.ringSize
		if idx < 0 {
			idx += ins.ringSize
		}
		e := ins.ring[idx]
		if e == nil {
			break
		}
		if appID == "" || e.AppID == appID {
			result = append(result, e)
		}
	}
	// Reverse to chronological order
	for i, j := 0, len(result)-1; i < j; i, j = i+1, j-1 {
		result[i], result[j] = result[j], result[i]
	}
	return result
}
