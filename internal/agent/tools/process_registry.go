package tools

import (
	"bytes"
	"context"
	"io"
	"os"
	"os/exec"
	"sync"
	"time"
)

// ProcessSession represents a running or completed bash process
type ProcessSession struct {
	ID            string
	Command       string
	Cwd           string
	PID           int
	StartedAt     time.Time
	ExitCode      *int
	ExitSignal    string
	Exited        bool
	Backgrounded  bool
	Truncated     bool

	// Output tracking
	MaxOutputChars        int
	PendingMaxOutputChars int
	TotalOutputChars      int
	PendingStdout         []byte
	PendingStderr         []byte
	Aggregated            string
	Tail                  string // Last 2000 chars

	// Process control
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	cancel context.CancelFunc

	mu sync.Mutex
}

// ProcessRegistry tracks running and finished bash processes
type ProcessRegistry struct {
	runningSessions  map[string]*ProcessSession
	finishedSessions map[string]*ProcessSession
	mu               sync.RWMutex

	// Cleanup settings
	finishedTTL time.Duration
	maxFinished int
	stopSweeper chan struct{}
}

const (
	DefaultMaxOutputChars        = 50000
	DefaultPendingMaxOutputChars = 10000
	DefaultTailChars             = 2000
	DefaultFinishedTTL           = 30 * time.Minute
	DefaultMaxFinished           = 100
	SweeperInterval              = 5 * time.Minute
)

// NewProcessRegistry creates a new process registry with auto-cleanup
func NewProcessRegistry() *ProcessRegistry {
	r := &ProcessRegistry{
		runningSessions:  make(map[string]*ProcessSession),
		finishedSessions: make(map[string]*ProcessSession),
		finishedTTL:      DefaultFinishedTTL,
		maxFinished:      DefaultMaxFinished,
		stopSweeper:      make(chan struct{}),
	}

	// Start background sweeper
	go r.sweeper()

	return r
}

// sweeper periodically cleans up expired finished sessions
func (r *ProcessRegistry) sweeper() {
	ticker := time.NewTicker(SweeperInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			r.cleanup()
		case <-r.stopSweeper:
			return
		}
	}
}

// cleanup removes expired finished sessions
func (r *ProcessRegistry) cleanup() {
	r.mu.Lock()
	defer r.mu.Unlock()

	now := time.Now()
	for id, sess := range r.finishedSessions {
		if now.Sub(sess.StartedAt) > r.finishedTTL {
			delete(r.finishedSessions, id)
		}
	}

	// Also enforce max finished limit (remove oldest)
	for len(r.finishedSessions) > r.maxFinished {
		var oldestID string
		var oldestTime time.Time
		for id, sess := range r.finishedSessions {
			if oldestID == "" || sess.StartedAt.Before(oldestTime) {
				oldestID = id
				oldestTime = sess.StartedAt
			}
		}
		if oldestID != "" {
			delete(r.finishedSessions, oldestID)
		}
	}
}

// Close stops the sweeper and cleans up
func (r *ProcessRegistry) Close() {
	close(r.stopSweeper)
}

// AddSession registers a new process session
func (r *ProcessRegistry) AddSession(sess *ProcessSession) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.runningSessions[sess.ID] = sess
}

// GetSession retrieves a running session by ID
func (r *ProcessRegistry) GetSession(id string) *ProcessSession {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.runningSessions[id]
}

// GetFinishedSession retrieves a finished session by ID
func (r *ProcessRegistry) GetFinishedSession(id string) *ProcessSession {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.finishedSessions[id]
}

// GetAnySession retrieves a session from either running or finished
func (r *ProcessRegistry) GetAnySession(id string) *ProcessSession {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if sess := r.runningSessions[id]; sess != nil {
		return sess
	}
	return r.finishedSessions[id]
}

// MarkBackgrounded marks a session as backgrounded
func (r *ProcessRegistry) MarkBackgrounded(id string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if sess := r.runningSessions[id]; sess != nil {
		sess.mu.Lock()
		sess.Backgrounded = true
		sess.mu.Unlock()
	}
}

// MarkExited moves a session from running to finished
func (r *ProcessRegistry) MarkExited(id string, exitCode int, exitSignal string) {
	r.mu.Lock()
	defer r.mu.Unlock()

	sess := r.runningSessions[id]
	if sess == nil {
		return
	}

	sess.mu.Lock()
	sess.Exited = true
	sess.ExitCode = &exitCode
	sess.ExitSignal = exitSignal
	sess.mu.Unlock()

	delete(r.runningSessions, id)
	r.finishedSessions[id] = sess
}

// DeleteSession removes a session from the registry
func (r *ProcessRegistry) DeleteSession(id string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.runningSessions, id)
	delete(r.finishedSessions, id)
}

// ListRunningSessions returns all running backgrounded sessions
func (r *ProcessRegistry) ListRunningSessions() []*ProcessSession {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var sessions []*ProcessSession
	for _, sess := range r.runningSessions {
		sess.mu.Lock()
		if sess.Backgrounded {
			sessions = append(sessions, sess)
		}
		sess.mu.Unlock()
	}
	return sessions
}

// ListFinishedSessions returns all finished sessions
func (r *ProcessRegistry) ListFinishedSessions() []*ProcessSession {
	r.mu.RLock()
	defer r.mu.RUnlock()

	sessions := make([]*ProcessSession, 0, len(r.finishedSessions))
	for _, sess := range r.finishedSessions {
		sessions = append(sessions, sess)
	}
	return sessions
}

// AppendOutput adds output to a session's buffers
func (r *ProcessRegistry) AppendOutput(id string, isStderr bool, data []byte) {
	r.mu.RLock()
	sess := r.runningSessions[id]
	r.mu.RUnlock()

	if sess == nil {
		return
	}

	sess.mu.Lock()
	defer sess.mu.Unlock()

	// Update aggregated output
	if len(sess.Aggregated)+len(data) > sess.MaxOutputChars {
		sess.Truncated = true
		remaining := sess.MaxOutputChars - len(sess.Aggregated)
		if remaining > 0 {
			sess.Aggregated += string(data[:remaining])
		}
	} else {
		sess.Aggregated += string(data)
	}

	// Update tail (last 2000 chars)
	sess.Tail += string(data)
	if len(sess.Tail) > DefaultTailChars {
		sess.Tail = sess.Tail[len(sess.Tail)-DefaultTailChars:]
	}

	// Update pending buffers
	if isStderr {
		sess.PendingStderr = append(sess.PendingStderr, data...)
		if len(sess.PendingStderr) > sess.PendingMaxOutputChars {
			sess.PendingStderr = sess.PendingStderr[len(sess.PendingStderr)-sess.PendingMaxOutputChars:]
		}
	} else {
		sess.PendingStdout = append(sess.PendingStdout, data...)
		if len(sess.PendingStdout) > sess.PendingMaxOutputChars {
			sess.PendingStdout = sess.PendingStdout[len(sess.PendingStdout)-sess.PendingMaxOutputChars:]
		}
	}

	sess.TotalOutputChars += len(data)
}

// DrainPending returns and clears pending output
func (r *ProcessRegistry) DrainPending(id string) (stdout, stderr []byte) {
	r.mu.RLock()
	sess := r.runningSessions[id]
	if sess == nil {
		sess = r.finishedSessions[id]
	}
	r.mu.RUnlock()

	if sess == nil {
		return nil, nil
	}

	sess.mu.Lock()
	defer sess.mu.Unlock()

	stdout = sess.PendingStdout
	stderr = sess.PendingStderr
	sess.PendingStdout = nil
	sess.PendingStderr = nil
	return stdout, stderr
}

// WriteStdin writes data to a session's stdin
func (r *ProcessRegistry) WriteStdin(id string, data []byte) error {
	r.mu.RLock()
	sess := r.runningSessions[id]
	r.mu.RUnlock()

	if sess == nil {
		return os.ErrNotExist
	}

	sess.mu.Lock()
	stdin := sess.stdin
	sess.mu.Unlock()

	if stdin == nil {
		return os.ErrClosed
	}

	_, err := stdin.Write(data)
	return err
}

// KillSession terminates a running session
func (r *ProcessRegistry) KillSession(id string) error {
	r.mu.RLock()
	sess := r.runningSessions[id]
	r.mu.RUnlock()

	if sess == nil {
		return os.ErrNotExist
	}

	sess.mu.Lock()
	cancel := sess.cancel
	cmd := sess.cmd
	sess.mu.Unlock()

	if cancel != nil {
		cancel()
	}
	if cmd != nil && cmd.Process != nil {
		return cmd.Process.Kill()
	}
	return nil
}

// GenerateSessionSlug creates a human-readable session ID
func GenerateSessionSlug(isTaken func(string) bool) string {
	adjectives := []string{
		"swift", "keen", "bold", "calm", "warm",
		"cool", "soft", "firm", "fair", "true",
		"safe", "wise", "kind", "neat", "pure",
	}
	nouns := []string{
		"cove", "dale", "glen", "vale", "reef",
		"cape", "bay", "peak", "ford", "moor",
		"oak", "elm", "ash", "pine", "fern",
	}

	for i := 0; i < 12; i++ {
		adj := adjectives[time.Now().UnixNano()%int64(len(adjectives))]
		noun := nouns[time.Now().UnixNano()%int64(len(nouns))]
		slug := adj + "-" + noun

		if i > 0 {
			slug = slug + "-" + string(rune('0'+i))
		}

		if !isTaken(slug) {
			return slug
		}
		time.Sleep(time.Millisecond) // Vary the nano timestamp
	}

	// Fallback to timestamp-based
	return "proc-" + time.Now().Format("150405")
}

// SpawnBackgroundProcess starts a process and registers it
func (r *ProcessRegistry) SpawnBackgroundProcess(ctx context.Context, command, cwd string, yieldMs int) (*ProcessSession, error) {
	// Generate unique session ID
	sessionID := GenerateSessionSlug(func(slug string) bool {
		return r.GetAnySession(slug) != nil
	})

	// Create cancellable context
	ctx, cancel := context.WithCancel(ctx)

	// Create command using platform-specific shell
	shell, shellArgs := ShellCommand()
	args := append(shellArgs, command)
	cmd := exec.CommandContext(ctx, shell, args...)
	if cwd != "" {
		cmd.Dir = cwd
	}

	// Set up pipes
	stdin, err := cmd.StdinPipe()
	if err != nil {
		cancel()
		return nil, err
	}

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return nil, err
	}

	stderrPipe, err := cmd.StderrPipe()
	if err != nil {
		cancel()
		return nil, err
	}

	// Create session
	sess := &ProcessSession{
		ID:                    sessionID,
		Command:               command,
		Cwd:                   cwd,
		StartedAt:             time.Now(),
		MaxOutputChars:        DefaultMaxOutputChars,
		PendingMaxOutputChars: DefaultPendingMaxOutputChars,
		cmd:                   cmd,
		stdin:                 stdin,
		cancel:                cancel,
	}

	// Start process
	if err := cmd.Start(); err != nil {
		cancel()
		return nil, err
	}
	sess.PID = cmd.Process.Pid

	// Register session
	r.AddSession(sess)

	// Start output readers
	go r.readOutput(sess.ID, stdoutPipe, false)
	go r.readOutput(sess.ID, stderrPipe, true)

	// Start exit watcher
	go func() {
		err := cmd.Wait()
		exitCode := 0
		exitSignal := ""
		if err != nil {
			if exitErr, ok := err.(*exec.ExitError); ok {
				exitCode = exitErr.ExitCode()
			}
		}
		r.MarkExited(sess.ID, exitCode, exitSignal)
	}()

	// Yield window - wait for initial output or timeout
	if yieldMs > 0 {
		yieldTimer := time.NewTimer(time.Duration(yieldMs) * time.Millisecond)
		defer yieldTimer.Stop()

		select {
		case <-yieldTimer.C:
			r.MarkBackgrounded(sess.ID)
		case <-ctx.Done():
			// Context cancelled
		}
	}

	return sess, nil
}

// readOutput reads from a pipe and appends to session buffers
func (r *ProcessRegistry) readOutput(sessionID string, pipe io.Reader, isStderr bool) {
	buf := make([]byte, 4096)
	for {
		n, err := pipe.Read(buf)
		if n > 0 {
			r.AppendOutput(sessionID, isStderr, buf[:n])
		}
		if err != nil {
			break
		}
	}
}

// GetSessionStatus returns a status summary for a session
func (sess *ProcessSession) GetStatus() map[string]any {
	sess.mu.Lock()
	defer sess.mu.Unlock()

	status := map[string]any{
		"id":           sess.ID,
		"command":      sess.Command,
		"cwd":          sess.Cwd,
		"pid":          sess.PID,
		"started_at":   sess.StartedAt.Format(time.RFC3339),
		"duration_ms":  time.Since(sess.StartedAt).Milliseconds(),
		"backgrounded": sess.Backgrounded,
		"exited":       sess.Exited,
		"truncated":    sess.Truncated,
	}

	if sess.ExitCode != nil {
		status["exit_code"] = *sess.ExitCode
	}
	if sess.ExitSignal != "" {
		status["exit_signal"] = sess.ExitSignal
	}

	return status
}

// GetOutput returns the current output buffer
func (sess *ProcessSession) GetOutput() string {
	sess.mu.Lock()
	defer sess.mu.Unlock()

	var result bytes.Buffer
	result.WriteString(sess.Aggregated)
	if len(sess.PendingStdout) > 0 {
		result.Write(sess.PendingStdout)
	}
	if len(sess.PendingStderr) > 0 {
		result.WriteString("\nSTDERR:\n")
		result.Write(sess.PendingStderr)
	}
	return result.String()
}
