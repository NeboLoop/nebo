// Package recovery handles task persistence and recovery after agent restart
package recovery

import (
	"context"
	"database/sql"
	"fmt"
	"time"

	"github.com/google/uuid"
)

// TaskStatus represents the status of a pending task
type TaskStatus string

const (
	StatusPending   TaskStatus = "pending"
	StatusRunning   TaskStatus = "running"
	StatusCompleted TaskStatus = "completed"
	StatusFailed    TaskStatus = "failed"
	StatusCancelled TaskStatus = "cancelled"
)

// TaskType represents the type of task
type TaskType string

const (
	TaskTypeSubagent    TaskType = "subagent"
	TaskTypeRun         TaskType = "run"
	TaskTypeEventAgent  TaskType = "event_agent"  // Scheduled/triggered tasks (renamed from cron_agent)
)

// PendingTask represents a task that can be recovered after restart
type PendingTask struct {
	ID           string
	TaskType     TaskType
	Status       TaskStatus
	SessionKey   string
	UserID       string
	Prompt       string
	SystemPrompt string
	Description  string
	Lane         string
	Priority     int
	Attempts     int
	MaxAttempts  int
	LastError    string
	CreatedAt    time.Time
	StartedAt    *time.Time
	CompletedAt  *time.Time
	ParentTaskID string
}

// Manager handles task persistence and recovery
type Manager struct {
	db *sql.DB
}

// NewManager creates a new recovery manager
func NewManager(db *sql.DB) *Manager {
	return &Manager{db: db}
}

// CreateTask persists a new task to the database
func (m *Manager) CreateTask(ctx context.Context, task *PendingTask) error {
	if task.ID == "" {
		task.ID = uuid.New().String()
	}
	if task.Status == "" {
		task.Status = StatusPending
	}
	if task.Lane == "" {
		task.Lane = "main"
	}
	if task.MaxAttempts == 0 {
		task.MaxAttempts = 3
	}

	_, err := m.db.ExecContext(ctx, `
		INSERT INTO pending_tasks (
			id, task_type, status, session_key, user_id, prompt, system_prompt,
			description, lane, priority, max_attempts, created_at, parent_task_id
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
	`,
		task.ID, string(task.TaskType), string(task.Status), task.SessionKey,
		nullString(task.UserID), task.Prompt, nullString(task.SystemPrompt),
		nullString(task.Description), task.Lane, task.Priority, task.MaxAttempts,
		time.Now().Unix(), nullString(task.ParentTaskID),
	)
	return err
}

// MarkRunning marks a task as running
func (m *Manager) MarkRunning(ctx context.Context, taskID string) error {
	_, err := m.db.ExecContext(ctx, `
		UPDATE pending_tasks
		SET status = 'running', started_at = ?, attempts = attempts + 1
		WHERE id = ?
	`, time.Now().Unix(), taskID)
	return err
}

// MarkCompleted marks a task as completed
func (m *Manager) MarkCompleted(ctx context.Context, taskID string) error {
	_, err := m.db.ExecContext(ctx, `
		UPDATE pending_tasks
		SET status = 'completed', completed_at = ?
		WHERE id = ?
	`, time.Now().Unix(), taskID)
	return err
}

// MarkFailed marks a task as failed (or requeues if retries remain)
func (m *Manager) MarkFailed(ctx context.Context, taskID, errorMsg string) error {
	_, err := m.db.ExecContext(ctx, `
		UPDATE pending_tasks
		SET status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
		    last_error = ?,
		    completed_at = CASE WHEN attempts >= max_attempts THEN ? ELSE NULL END
		WHERE id = ?
	`, errorMsg, time.Now().Unix(), taskID)
	return err
}

// GetRecoverableTasks returns tasks that need to be recovered after restart
func (m *Manager) GetRecoverableTasks(ctx context.Context) ([]*PendingTask, error) {
	rows, err := m.db.QueryContext(ctx, `
		SELECT id, task_type, status, session_key, user_id, prompt, system_prompt,
		       description, lane, priority, attempts, max_attempts, last_error,
		       created_at, started_at, completed_at, parent_task_id
		FROM pending_tasks
		WHERE status IN ('pending', 'running')
		ORDER BY priority DESC, created_at ASC
	`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var tasks []*PendingTask
	for rows.Next() {
		task := &PendingTask{}
		var taskType, status, userID, systemPrompt, description, lastError, parentTaskID sql.NullString
		var startedAt, completedAt sql.NullInt64
		var createdAt int64

		err := rows.Scan(
			&task.ID, &taskType, &status, &task.SessionKey, &userID, &task.Prompt, &systemPrompt,
			&description, &task.Lane, &task.Priority, &task.Attempts, &task.MaxAttempts, &lastError,
			&createdAt, &startedAt, &completedAt, &parentTaskID,
		)
		if err != nil {
			return nil, err
		}

		task.TaskType = TaskType(taskType.String)
		task.Status = TaskStatus(status.String)
		task.UserID = userID.String
		task.SystemPrompt = systemPrompt.String
		task.Description = description.String
		task.LastError = lastError.String
		task.ParentTaskID = parentTaskID.String
		task.CreatedAt = time.Unix(createdAt, 0)
		if startedAt.Valid {
			t := time.Unix(startedAt.Int64, 0)
			task.StartedAt = &t
		}
		if completedAt.Valid {
			t := time.Unix(completedAt.Int64, 0)
			task.CompletedAt = &t
		}

		tasks = append(tasks, task)
	}
	return tasks, rows.Err()
}

// CheckTaskCompletion checks if a task was actually completed by examining its session.
// A task is considered complete if:
// 1. The session has assistant messages with tool calls or substantial content, OR
// 2. The session has multiple exchange rounds (user → assistant → tool → assistant).
// This is deliberately generous — it's better to skip a completed task than re-run it.
func (m *Manager) CheckTaskCompletion(ctx context.Context, task *PendingTask) (bool, error) {
	// Count how many assistant messages exist in the session
	var assistantCount int
	var totalCount int
	var toolCallCount int

	err := m.db.QueryRowContext(ctx, `
		SELECT
			COUNT(*) as total,
			SUM(CASE WHEN role = 'assistant' THEN 1 ELSE 0 END) as assistant_count,
			SUM(CASE WHEN tool_calls IS NOT NULL AND tool_calls != '' AND tool_calls != 'null' THEN 1 ELSE 0 END) as tool_count
		FROM chat_messages
		WHERE chat_id = ?
	`, task.SessionKey).Scan(&totalCount, &assistantCount, &toolCallCount)

	if err == sql.ErrNoRows || totalCount == 0 {
		// No messages at all — task never started
		fmt.Printf("[Recovery] Task %s: no chat messages found\n", task.ID[:8])
		return false, nil
	}
	if err != nil {
		return false, err
	}

	fmt.Printf("[Recovery] Task %s: %d messages, %d assistant, %d with tool calls\n",
		task.ID[:8], totalCount, assistantCount, toolCallCount)

	// If the agent made tool calls, it was actively working — consider it complete
	// (better to lose partial work than re-run and duplicate side effects)
	if toolCallCount > 0 {
		return true, nil
	}

	// If the agent produced any assistant response, it at least started processing
	// Multiple messages means it went through the loop
	if assistantCount > 0 && totalCount > 2 {
		return true, nil
	}

	// Check the last message — if it's from assistant with real content, it's done
	var lastRole, lastContent sql.NullString
	err = m.db.QueryRowContext(ctx, `
		SELECT role, content
		FROM chat_messages
		WHERE chat_id = ?
		ORDER BY created_at DESC
		LIMIT 1
	`, task.SessionKey).Scan(&lastRole, &lastContent)

	if err != nil && err != sql.ErrNoRows {
		return false, err
	}

	if lastRole.String == "assistant" && lastContent.Valid && len(lastContent.String) > 50 {
		return true, nil
	}

	return false, nil
}

// RecoverTasks checks for incomplete tasks and returns them for re-execution
// This should be called on agent startup
func (m *Manager) RecoverTasks(ctx context.Context) ([]*PendingTask, error) {
	tasks, err := m.GetRecoverableTasks(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to get recoverable tasks: %w", err)
	}

	var toRecover []*PendingTask
	for _, task := range tasks {
		// Check if the task was actually completed (session has completion)
		completed, err := m.CheckTaskCompletion(ctx, task)
		if err != nil {
			fmt.Printf("[Recovery] Warning: failed to check completion for task %s: %v\n", task.ID, err)
			continue
		}

		if completed {
			// Task was actually completed, mark it as such
			fmt.Printf("[Recovery] Task %s was completed (found completion in session)\n", task.ID)
			if err := m.MarkCompleted(ctx, task.ID); err != nil {
				fmt.Printf("[Recovery] Warning: failed to mark task %s as completed: %v\n", task.ID, err)
			}
			continue
		}

		// Task needs to be recovered
		// Reset status to pending for re-execution
		if task.Status == StatusRunning {
			task.Status = StatusPending
			_, err := m.db.ExecContext(ctx, `
				UPDATE pending_tasks SET status = 'pending' WHERE id = ?
			`, task.ID)
			if err != nil {
				fmt.Printf("[Recovery] Warning: failed to reset task %s status: %v\n", task.ID, err)
			}
		}

		toRecover = append(toRecover, task)
	}

	return toRecover, nil
}

// MarkCancelled unconditionally marks a task as cancelled.
// Unlike MarkFailed, this never re-queues — use for shutdown.
func (m *Manager) MarkCancelled(ctx context.Context, taskID string) error {
	_, err := m.db.ExecContext(ctx, `
		UPDATE pending_tasks
		SET status = 'cancelled', last_error = 'shutdown', completed_at = ?
		WHERE id = ?
	`, time.Now().Unix(), taskID)
	return err
}

// CleanupOldTasks removes completed/failed tasks older than 7 days
func (m *Manager) CleanupOldTasks(ctx context.Context) (int64, error) {
	result, err := m.db.ExecContext(ctx, `
		DELETE FROM pending_tasks
		WHERE status IN ('completed', 'failed', 'cancelled')
		  AND completed_at < ?
	`, time.Now().Add(-7*24*time.Hour).Unix())
	if err != nil {
		return 0, err
	}
	return result.RowsAffected()
}

func nullString(s string) sql.NullString {
	if s == "" {
		return sql.NullString{}
	}
	return sql.NullString{String: s, Valid: true}
}
