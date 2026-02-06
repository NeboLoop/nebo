package events

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"
	"time"
)

// HandlerFunc is the function called when an event is emitted.
type HandlerFunc func(context.Context, any) error

// SubjectOption configures a Subject
type SubjectOption func(*subjectConfig)

type subjectConfig struct {
	replayEnabled bool
	cacheSize     int
	bufferSize    int
	syncDelivery  bool
	logger        *slog.Logger
}

// WithBufferSize sets the event channel buffer size
func WithBufferSize(size int) SubjectOption {
	return func(cfg *subjectConfig) {
		cfg.bufferSize = size
	}
}

// WithReplay enables replay functionality with specified cache size
func WithReplay(cacheSize int) SubjectOption {
	return func(cfg *subjectConfig) {
		cfg.replayEnabled = true
		cfg.cacheSize = cacheSize
	}
}

// WithLogger sets a structured logger for event system errors
func WithLogger(logger *slog.Logger) SubjectOption {
	return func(cfg *subjectConfig) {
		cfg.logger = logger
	}
}

// WithSyncDelivery forces synchronous (inline) event delivery.
// This serializes all handler calls within the single eventLoop goroutine,
// which is useful when handlers must not be called concurrently (e.g. WebSocket writes).
func WithSyncDelivery() SubjectOption {
	return func(cfg *subjectConfig) {
		cfg.syncDelivery = true
	}
}

// Emit emits an event to the given topic.
func Emit[T any](subject *Subject, topic string, value T) error {
	evt := event{
		topic:   topic,
		message: value,
	}

	select {
	case subject.events <- evt:
		return nil
	case <-time.After(5 * time.Second):
		return fmt.Errorf("failed to emit event: %v", value)
	}
}

// Subscribe subscribes a typed handler to the given topic.
// A Subscription is returned that can be used to unsubscribe from the topic.
func Subscribe[T any](subject *Subject, topic string, handler func(context.Context, T) error, replay ...bool) Subscription {
	wantsReplay := false
	if len(replay) > 0 {
		wantsReplay = replay[0]
	}

	wrappedHandler := HandlerFunc(func(ctx context.Context, data any) error {
		if typed, ok := data.(T); ok {
			return handler(ctx, typed)
		}
		return fmt.Errorf("type assertion failed for %T, expected %T", data, *new(T))
	})

	subID := atomic.AddInt64(&subject.nextSubID, 1)
	createdAt := time.Now().UnixNano()

	sub := Subscription{
		Topic:       topic,
		CreatedAt:   createdAt,
		Handler:     wrappedHandler,
		ID:          fmt.Sprintf("%s-%d", topic, subID),
		WantsReplay: wantsReplay,
		SentEvents:  make(map[string]bool),
	}

	// Add subscription using copy-on-write
	subject.addSubscription(sub)

	// Set up unsubscribe function
	sub.Unsubscribe = func() {
		subject.removeSubscription(sub.ID)
	}

	// Handle replay if enabled
	if subject.config.replayEnabled && wantsReplay {
		subject.replayEvents(sub)
	}

	return sub
}

// Complete shuts down the event system, stopping all goroutines and cleaning up resources.
// This function is idempotent and safe to call multiple times.
func Complete(s *Subject) {
	if s == nil {
		return
	}

	// Try to close the shutdown channel only once using atomic operation
	if atomic.CompareAndSwapInt32(&s.closed, 0, 1) {
		close(s.shutdown)

		// Wait for goroutines to finish (with timeout to prevent hanging)
		done := make(chan struct{})
		go func() {
			s.wg.Wait()
			close(done)
		}()

		select {
		case <-done:
			// All goroutines finished
		case <-time.After(5 * time.Second):
			// Timeout waiting for goroutines
		}
	}
}

type event struct {
	topic   string
	message any
}

// Subscription represents a handler subscribed to a specific topic.
type Subscription struct {
	Topic       string
	CreatedAt   int64
	Handler     HandlerFunc
	ID          string
	WantsReplay bool
	SentEvents  map[string]bool // Replay tracking per subscription
	Unsubscribe func()
}

type subscriberMap map[string]map[string]Subscription

type Subject struct {
	// Lock-free state using atomics
	subscribers atomic.Pointer[subscriberMap]
	cache       atomic.Pointer[[]event]
	nextSubID   int64
	eventCount  int64

	// Single event channel
	events   chan event
	shutdown chan struct{}

	// Configuration (read-only after creation)
	config subjectConfig

	// Additional fields for Complete function
	closed int32
	wg     sync.WaitGroup
}

// NewSubject creates a new Subject with optional configuration.
func NewSubject(opts ...SubjectOption) *Subject {
	cfg := subjectConfig{
		bufferSize: 512, // default
	}

	// Apply options
	for _, opt := range opts {
		opt(&cfg)
	}

	s := &Subject{
		events:   make(chan event, cfg.bufferSize),
		shutdown: make(chan struct{}),
		config:   cfg,
	}

	// Initialize atomic pointers
	emptySubscribers := make(subscriberMap)
	s.subscribers.Store(&emptySubscribers)

	if cfg.replayEnabled {
		emptyCache := make([]event, 0, cfg.cacheSize)
		s.cache.Store(&emptyCache)
	}

	go s.eventLoop()
	return s
}

// eventLoop processes events and distributes them to subscribers
func (s *Subject) eventLoop() {
	s.wg.Add(1)
	defer s.wg.Done()

	for {
		select {
		case <-s.shutdown:
			return
		case evt := <-s.events:
			atomic.AddInt64(&s.eventCount, 1)

			// Add to cache if replay enabled (copy-on-write)
			if s.config.replayEnabled {
				s.addToCache(evt)
			}

			// Send to subscribers (lock-free read)
			subs := s.subscribers.Load()
			if topicSubs, ok := (*subs)[evt.topic]; ok {
				for _, sub := range topicSubs {
					s.sendToSubscriber(sub, evt, s.config.syncDelivery)
				}
			}
		}
	}
}

// addSubscription adds a subscription using copy-on-write
func (s *Subject) addSubscription(sub Subscription) {
	for {
		oldSubs := s.subscribers.Load()
		newSubs := s.copySubscribers(*oldSubs)

		if _, ok := newSubs[sub.Topic]; !ok {
			newSubs[sub.Topic] = make(map[string]Subscription)
		}
		newSubs[sub.Topic][sub.ID] = sub

		if s.subscribers.CompareAndSwap(oldSubs, &newSubs) {
			break
		}
		// Retry if CAS failed (another goroutine modified it)
	}
}

// removeSubscription removes a subscription using copy-on-write
func (s *Subject) removeSubscription(subID string) {
	for {
		oldSubs := s.subscribers.Load()
		newSubs := s.copySubscribers(*oldSubs)

		found := false
		for topic, topicSubs := range newSubs {
			if _, ok := topicSubs[subID]; ok {
				delete(topicSubs, subID)
				if len(topicSubs) == 0 {
					delete(newSubs, topic)
				}
				found = true
				break
			}
		}

		if !found {
			break // Subscription not found, nothing to do
		}

		if s.subscribers.CompareAndSwap(oldSubs, &newSubs) {
			break
		}
		// Retry if CAS failed
	}
}

// copySubscribers creates a deep copy of the subscribers map
func (s *Subject) copySubscribers(original subscriberMap) subscriberMap {
	cp := make(subscriberMap, len(original))
	for topic, topicSubs := range original {
		cp[topic] = make(map[string]Subscription, len(topicSubs))
		for id, sub := range topicSubs {
			cp[topic][id] = sub
		}
	}
	return cp
}

// addToCache adds an event to the cache using copy-on-write
func (s *Subject) addToCache(evt event) {
	for {
		oldCache := s.cache.Load()
		newCache := make([]event, len(*oldCache))
		copy(newCache, *oldCache)

		if len(newCache) == s.config.cacheSize {
			newCache = newCache[1:]
		}
		newCache = append(newCache, evt)

		if s.cache.CompareAndSwap(oldCache, &newCache) {
			break
		}
		// Retry if CAS failed
	}
}

// replayEvents sends cached events to a new subscriber
func (s *Subject) replayEvents(sub Subscription) {
	if !s.config.replayEnabled {
		return
	}

	cache := s.cache.Load()
	for _, evt := range *cache {
		if evt.topic == sub.Topic {
			eventID := fmt.Sprintf("%s-%v", evt.topic, evt.message)
			if !sub.SentEvents[eventID] {
				// Send replay events synchronously to preserve order
				s.sendToSubscriber(sub, evt, true)
				sub.SentEvents[eventID] = true
			}
		}
	}
}

// sendToSubscriber delivers an event to a subscriber.
// If sync is true, delivery is synchronous (blocking). If false, delivery is asynchronous.
func (s *Subject) sendToSubscriber(sub Subscription, evt event, sync bool) {
	deliverEvent := func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		if err := sub.Handler(ctx, evt.message); err != nil {
			if s.config.logger != nil {
				s.config.logger.Debug("event handler error",
					"topic", evt.topic,
					"error", err,
					"subscription_id", sub.ID,
					"delivery_mode", map[bool]string{true: "sync", false: "async"}[sync])
			}
		}
	}

	if sync {
		deliverEvent()
	} else {
		go deliverEvent()
	}
}
