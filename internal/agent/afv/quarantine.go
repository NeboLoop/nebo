package afv

import (
	"sync"
	"time"
)

const maxQuarantineEntries = 50

// QuarantinedResponse holds a response that failed fence verification.
type QuarantinedResponse struct {
	SessionID    string
	Content      string
	Timestamp    time.Time
	VerifyResult *VerifyResult
}

// QuarantineStore is an in-memory ring buffer for quarantined responses.
// Never persisted to disk or database.
type QuarantineStore struct {
	mu      sync.Mutex
	entries []QuarantinedResponse
	head    int
	count   int
}

// NewQuarantineStore creates a new in-memory quarantine store.
func NewQuarantineStore() *QuarantineStore {
	return &QuarantineStore{
		entries: make([]QuarantinedResponse, maxQuarantineEntries),
	}
}

// Add stores a quarantined response, evicting the oldest if at capacity.
func (q *QuarantineStore) Add(entry QuarantinedResponse) {
	q.mu.Lock()
	defer q.mu.Unlock()

	q.entries[q.head] = entry
	q.head = (q.head + 1) % maxQuarantineEntries
	if q.count < maxQuarantineEntries {
		q.count++
	}
}

// Recent returns the n most recent quarantined responses, newest first.
func (q *QuarantineStore) Recent(n int) []QuarantinedResponse {
	q.mu.Lock()
	defer q.mu.Unlock()

	if n > q.count {
		n = q.count
	}
	result := make([]QuarantinedResponse, n)
	for i := 0; i < n; i++ {
		idx := (q.head - 1 - i + maxQuarantineEntries) % maxQuarantineEntries
		result[i] = q.entries[idx]
	}
	return result
}

// Count returns the number of quarantined responses.
func (q *QuarantineStore) Count() int {
	q.mu.Lock()
	defer q.mu.Unlock()
	return q.count
}
