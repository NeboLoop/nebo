package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"sync"
	"time"

	cronlib "github.com/robfig/cron/v3"
)

// AgentTaskCallback is called when an agent task cron job fires
type AgentTaskCallback func(ctx context.Context, name, message string, deliver *DeliverConfig) error

// DeliverConfig specifies where to send task results
type DeliverConfig struct {
	Channel string `json:"channel"`
	To      string `json:"to"`
}

// CronTool manages scheduled recurring tasks
type CronTool struct {
	db            *sql.DB
	scheduler     *cronlib.Cron
	jobs          map[string]cronlib.EntryID
	mu            sync.RWMutex
	agentCallback AgentTaskCallback
}

type cronInput struct {
	Action   string `json:"action"`    // create, list, delete, pause, resume, run
	Name     string `json:"name"`      // Job name/identifier
	Schedule string `json:"schedule"`  // Cron expression (e.g., "*/5 * * * *")
	Command  string `json:"command"`   // Shell command to execute (for bash tasks)
	TaskType string `json:"task_type"` // "bash" (default) or "agent"
	Message  string `json:"message"`   // Agent prompt (for agent tasks)
	Deliver  *struct {
		Channel string `json:"channel"` // telegram, discord, slack
		To      string `json:"to"`      // chat/channel ID
	} `json:"deliver,omitempty"` // Optional: where to send result
	Enabled *bool `json:"enabled"` // Enable/disable job
}

type cronJob struct {
	ID        int64     `json:"id"`
	Name      string    `json:"name"`
	Schedule  string    `json:"schedule"`
	Command   string    `json:"command"`            // For bash tasks
	TaskType  string    `json:"task_type"`          // "bash" or "agent"
	Message   string    `json:"message,omitempty"`  // For agent tasks
	Deliver   string    `json:"deliver,omitempty"`  // JSON: {"channel":"telegram","to":"123"}
	Enabled   bool      `json:"enabled"`
	LastRun   time.Time `json:"last_run,omitempty"`
	NextRun   time.Time `json:"next_run,omitempty"`
	RunCount  int       `json:"run_count"`
	LastError string    `json:"last_error,omitempty"`
	CreatedAt time.Time `json:"created_at"`
}

// CronConfig configures the cron tool
type CronConfig struct {
	DB *sql.DB // Shared database connection (required)
}

// NewCronTool creates a new cron tool using the shared database connection.
// The database must already have the cron_jobs and cron_history tables (via migrations).
func NewCronTool(cfg CronConfig) (*CronTool, error) {
	if cfg.DB == nil {
		return nil, fmt.Errorf("database connection required")
	}

	tool := &CronTool{
		db:        cfg.DB,
		scheduler: cronlib.New(cronlib.WithSeconds()),
		jobs:      make(map[string]cronlib.EntryID),
	}

	// Load existing jobs from shared database
	if err := tool.loadJobs(); err != nil {
		return nil, err
	}

	// Start scheduler
	tool.scheduler.Start()

	return tool, nil
}

func (t *CronTool) loadJobs() error {
	rows, err := t.db.Query(`SELECT id, name, schedule, command, task_type, message, deliver, enabled FROM cron_jobs WHERE enabled = 1`)
	if err != nil {
		return err
	}
	defer rows.Close()

	for rows.Next() {
		var id int64
		var name, schedule, command, taskType, message, deliver string
		var enabled bool
		if err := rows.Scan(&id, &name, &schedule, &command, &taskType, &message, &deliver, &enabled); err != nil {
			continue
		}

		if enabled {
			t.scheduleJobFull(name, schedule, command, taskType, message, deliver)
		}
	}
	return nil
}

func (t *CronTool) scheduleJobFull(name, schedule, command, taskType, message, deliver string) error {
	t.mu.Lock()
	defer t.mu.Unlock()

	// Remove existing job if any
	if entryID, exists := t.jobs[name]; exists {
		t.scheduler.Remove(entryID)
		delete(t.jobs, name)
	}

	// Schedule new job
	entryID, err := t.scheduler.AddFunc(schedule, func() {
		t.executeJobFull(name, command, taskType, message, deliver)
	})
	if err != nil {
		return err
	}

	t.jobs[name] = entryID
	return nil
}

func (t *CronTool) executeJobFull(name, command, taskType, message, deliverJSON string) {
	started := time.Now()
	var output []byte
	var err error

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	if taskType == "agent" {
		// Execute agent task via callback
		t.mu.RLock()
		cb := t.agentCallback
		t.mu.RUnlock()

		if cb != nil {
			var deliver *DeliverConfig
			if deliverJSON != "" {
				deliver = &DeliverConfig{}
				json.Unmarshal([]byte(deliverJSON), deliver)
			}
			err = cb(ctx, name, message, deliver)
			if err == nil {
				output = []byte("Agent task completed successfully")
			}
		} else {
			err = fmt.Errorf("no agent callback configured")
		}
	} else {
		// Execute bash command
		cmd := exec.CommandContext(ctx, "bash", "-c", command)
		output, err = cmd.CombinedOutput()
	}

	finished := time.Now()
	success := err == nil
	var errStr string
	if err != nil {
		errStr = err.Error()
	}

	// Record execution
	t.db.Exec(`
		INSERT INTO cron_history (job_id, started_at, finished_at, success, output, error)
		SELECT id, ?, ?, ?, ?, ? FROM cron_jobs WHERE name = ?
	`, started, finished, success, string(output), errStr, name)

	// Update job stats
	if err != nil {
		t.db.Exec(`
			UPDATE cron_jobs SET last_run = ?, run_count = run_count + 1, last_error = ?
			WHERE name = ?
		`, started, errStr, name)
	} else {
		t.db.Exec(`
			UPDATE cron_jobs SET last_run = ?, run_count = run_count + 1, last_error = NULL
			WHERE name = ?
		`, started, name)
	}
}

// SetAgentCallback sets the callback for agent task execution
func (t *CronTool) SetAgentCallback(cb AgentTaskCallback) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.agentCallback = cb
}

func (t *CronTool) Close() error {
	if t.scheduler != nil {
		t.scheduler.Stop()
	}
	if t.db != nil {
		return t.db.Close()
	}
	return nil
}

func (t *CronTool) Name() string {
	return "cron"
}

func (t *CronTool) Description() string {
	return "Schedule recurring tasks using cron expressions. Create, list, pause, resume, or delete scheduled jobs."
}

func (t *CronTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["create", "list", "delete", "pause", "resume", "run", "history"],
				"description": "Cron action: create (new job), list (show all jobs), delete (remove job), pause (disable), resume (enable), run (execute now), history (show run history)"
			},
			"name": {
				"type": "string",
				"description": "Unique job name/identifier (required for create, delete, pause, resume, run, history)"
			},
			"schedule": {
				"type": "string",
				"description": "Cron expression with seconds: 'second minute hour day-of-month month day-of-week'. Examples: '0 */5 * * * *' (every 5 min), '0 0 9 * * 1-5' (9am weekdays)"
			},
			"task_type": {
				"type": "string",
				"enum": ["bash", "agent"],
				"description": "Task type: 'bash' runs shell command, 'agent' runs AI agent task. Default: bash"
			},
			"command": {
				"type": "string",
				"description": "Shell command to execute (required for bash tasks)"
			},
			"message": {
				"type": "string",
				"description": "Prompt for the agent to execute (required for agent tasks)"
			},
			"deliver": {
				"type": "object",
				"description": "Optional: where to send the result",
				"properties": {
					"channel": { "type": "string", "description": "Channel: telegram, discord, slack" },
					"to": { "type": "string", "description": "Destination chat/channel ID" }
				}
			}
		},
		"required": ["action"]
	}`)
}

func (t *CronTool) RequiresApproval() bool {
	return true // Scheduling tasks can be dangerous
}

func (t *CronTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params cronInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	var result string
	var err error

	switch params.Action {
	case "create":
		result, err = t.create(params)
	case "list":
		result, err = t.list()
	case "delete":
		result, err = t.delete(params.Name)
	case "pause":
		result, err = t.pause(params.Name)
	case "resume":
		result, err = t.resume(params.Name)
	case "run":
		result, err = t.runNow(params.Name)
	case "history":
		result, err = t.history(params.Name)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s", params.Action),
			IsError: true,
		}, nil
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Cron action failed: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: result,
		IsError: false,
	}, nil
}

func (t *CronTool) create(params cronInput) (string, error) {
	if params.Name == "" {
		return "", fmt.Errorf("name is required for create action")
	}
	if params.Schedule == "" {
		return "", fmt.Errorf("schedule is required for create action")
	}

	// Determine task type
	taskType := params.TaskType
	if taskType == "" {
		taskType = "bash"
	}

	// Validate required fields based on task type
	if taskType == "bash" && params.Command == "" {
		return "", fmt.Errorf("command is required for bash tasks")
	}
	if taskType == "agent" && params.Message == "" {
		return "", fmt.Errorf("message is required for agent tasks")
	}

	// Validate cron expression
	parser := cronlib.NewParser(cronlib.Second | cronlib.Minute | cronlib.Hour | cronlib.Dom | cronlib.Month | cronlib.Dow)
	schedule, err := parser.Parse(params.Schedule)
	if err != nil {
		return "", fmt.Errorf("invalid cron schedule: %w", err)
	}

	// Serialize deliver config
	var deliverJSON string
	if params.Deliver != nil {
		data, _ := json.Marshal(params.Deliver)
		deliverJSON = string(data)
	}

	// Insert or update
	_, err = t.db.Exec(`
		INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, enabled)
		VALUES (?, ?, ?, ?, ?, ?, 1)
		ON CONFLICT(name) DO UPDATE SET
			schedule = excluded.schedule,
			command = excluded.command,
			task_type = excluded.task_type,
			message = excluded.message,
			deliver = excluded.deliver,
			enabled = 1
	`, params.Name, params.Schedule, params.Command, taskType, params.Message, deliverJSON)
	if err != nil {
		return "", err
	}

	// Schedule the job
	if err := t.scheduleJobFull(params.Name, params.Schedule, params.Command, taskType, params.Message, deliverJSON); err != nil {
		return "", err
	}

	nextRun := schedule.Next(time.Now())
	if taskType == "agent" {
		return fmt.Sprintf("Created agent cron job '%s'\nSchedule: %s\nPrompt: %s\nNext run: %s",
			params.Name, params.Schedule, params.Message, nextRun.Format(time.RFC3339)), nil
	}
	return fmt.Sprintf("Created cron job '%s'\nSchedule: %s\nCommand: %s\nNext run: %s",
		params.Name, params.Schedule, params.Command, nextRun.Format(time.RFC3339)), nil
}

func (t *CronTool) list() (string, error) {
	rows, err := t.db.Query(`
		SELECT name, schedule, command, task_type, message, enabled, last_run, run_count, last_error, created_at
		FROM cron_jobs
		ORDER BY name
	`)
	if err != nil {
		return "", err
	}
	defer rows.Close()

	var jobs []string
	for rows.Next() {
		var name, schedule, command, taskType, message string
		var enabled bool
		var lastRun sql.NullTime
		var runCount int
		var lastError sql.NullString
		var createdAt time.Time

		if err := rows.Scan(&name, &schedule, &command, &taskType, &message, &enabled, &lastRun, &runCount, &lastError, &createdAt); err != nil {
			continue
		}

		status := "enabled"
		if !enabled {
			status = "paused"
		}

		// Calculate next run
		var nextRun string
		if enabled {
			t.mu.RLock()
			if entryID, exists := t.jobs[name]; exists {
				entry := t.scheduler.Entry(entryID)
				nextRun = entry.Next.Format("2006-01-02 15:04:05")
			}
			t.mu.RUnlock()
		}

		// Build job info based on task type
		var jobInfo string
		if taskType == "agent" {
			jobInfo = fmt.Sprintf("- %s [%s] (agent task)\n  Schedule: %s\n  Prompt: %s\n  Runs: %d",
				name, status, schedule, message, runCount)
		} else {
			jobInfo = fmt.Sprintf("- %s [%s] (bash)\n  Schedule: %s\n  Command: %s\n  Runs: %d",
				name, status, schedule, command, runCount)
		}
		if lastRun.Valid {
			jobInfo += fmt.Sprintf("\n  Last run: %s", lastRun.Time.Format("2006-01-02 15:04:05"))
		}
		if lastError.Valid && lastError.String != "" {
			jobInfo += fmt.Sprintf("\n  Last error: %s", lastError.String)
		}
		if nextRun != "" {
			jobInfo += fmt.Sprintf("\n  Next run: %s", nextRun)
		}

		jobs = append(jobs, jobInfo)
	}

	if len(jobs) == 0 {
		return "No cron jobs configured", nil
	}

	return fmt.Sprintf("Cron jobs (%d):\n\n%s", len(jobs), strings.Join(jobs, "\n\n")), nil
}

func (t *CronTool) delete(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for delete action")
	}

	// Remove from scheduler
	t.mu.Lock()
	if entryID, exists := t.jobs[name]; exists {
		t.scheduler.Remove(entryID)
		delete(t.jobs, name)
	}
	t.mu.Unlock()

	// Remove from database
	result, err := t.db.Exec(`DELETE FROM cron_jobs WHERE name = ?`, name)
	if err != nil {
		return "", err
	}

	rows, _ := result.RowsAffected()
	if rows == 0 {
		return fmt.Sprintf("No cron job found with name '%s'", name), nil
	}

	return fmt.Sprintf("Deleted cron job '%s'", name), nil
}

func (t *CronTool) pause(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for pause action")
	}

	// Remove from scheduler
	t.mu.Lock()
	if entryID, exists := t.jobs[name]; exists {
		t.scheduler.Remove(entryID)
		delete(t.jobs, name)
	}
	t.mu.Unlock()

	// Update database
	result, err := t.db.Exec(`UPDATE cron_jobs SET enabled = 0 WHERE name = ?`, name)
	if err != nil {
		return "", err
	}

	rows, _ := result.RowsAffected()
	if rows == 0 {
		return fmt.Sprintf("No cron job found with name '%s'", name), nil
	}

	return fmt.Sprintf("Paused cron job '%s'", name), nil
}

func (t *CronTool) resume(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for resume action")
	}

	// Get full job details
	var schedule, command, taskType, message, deliver string
	err := t.db.QueryRow(`SELECT schedule, command, task_type, message, deliver FROM cron_jobs WHERE name = ?`, name).
		Scan(&schedule, &command, &taskType, &message, &deliver)
	if err == sql.ErrNoRows {
		return fmt.Sprintf("No cron job found with name '%s'", name), nil
	}
	if err != nil {
		return "", err
	}

	// Update database
	_, err = t.db.Exec(`UPDATE cron_jobs SET enabled = 1 WHERE name = ?`, name)
	if err != nil {
		return "", err
	}

	// Schedule the job with full data
	if err := t.scheduleJobFull(name, schedule, command, taskType, message, deliver); err != nil {
		return "", err
	}

	return fmt.Sprintf("Resumed cron job '%s'", name), nil
}

func (t *CronTool) runNow(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for run action")
	}

	// Get full job details
	var command, taskType, message, deliver string
	err := t.db.QueryRow(`SELECT command, task_type, message, deliver FROM cron_jobs WHERE name = ?`, name).
		Scan(&command, &taskType, &message, &deliver)
	if err == sql.ErrNoRows {
		return fmt.Sprintf("No cron job found with name '%s'", name), nil
	}
	if err != nil {
		return "", err
	}

	// Execute synchronously
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	var output []byte

	if taskType == "agent" {
		// Execute agent task via callback
		t.mu.RLock()
		cb := t.agentCallback
		t.mu.RUnlock()

		if cb == nil {
			return "", fmt.Errorf("no agent callback configured - agent tasks require the agent to be running")
		}

		var deliverCfg *DeliverConfig
		if deliver != "" {
			deliverCfg = &DeliverConfig{}
			json.Unmarshal([]byte(deliver), deliverCfg)
		}

		err = cb(ctx, name, message, deliverCfg)
		if err == nil {
			output = []byte("Agent task completed successfully")
		}
	} else {
		// Execute bash command
		cmd := exec.CommandContext(ctx, "bash", "-c", command)
		output, err = cmd.CombinedOutput()
	}

	// Update stats
	if err != nil {
		t.db.Exec(`UPDATE cron_jobs SET last_run = CURRENT_TIMESTAMP, run_count = run_count + 1, last_error = ? WHERE name = ?`,
			err.Error(), name)
		return fmt.Sprintf("Job '%s' executed with error:\n%s\nOutput:\n%s", name, err.Error(), string(output)), nil
	}

	t.db.Exec(`UPDATE cron_jobs SET last_run = CURRENT_TIMESTAMP, run_count = run_count + 1, last_error = NULL WHERE name = ?`, name)

	outputStr := string(output)
	if len(outputStr) > 5000 {
		outputStr = outputStr[:5000] + "\n... (truncated)"
	}

	return fmt.Sprintf("Job '%s' executed successfully.\nOutput:\n%s", name, outputStr), nil
}

func (t *CronTool) history(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for history action")
	}

	rows, err := t.db.Query(`
		SELECT h.started_at, h.finished_at, h.success, h.output, h.error
		FROM cron_history h
		JOIN cron_jobs j ON j.id = h.job_id
		WHERE j.name = ?
		ORDER BY h.started_at DESC
		LIMIT 10
	`, name)
	if err != nil {
		return "", err
	}
	defer rows.Close()

	var entries []string
	for rows.Next() {
		var startedAt time.Time
		var finishedAt sql.NullTime
		var success bool
		var output, errStr sql.NullString

		if err := rows.Scan(&startedAt, &finishedAt, &success, &output, &errStr); err != nil {
			continue
		}

		status := "success"
		if !success {
			status = "failed"
		}

		duration := "running"
		if finishedAt.Valid {
			duration = finishedAt.Time.Sub(startedAt).String()
		}

		entry := fmt.Sprintf("- %s [%s] (duration: %s)",
			startedAt.Format("2006-01-02 15:04:05"), status, duration)
		if errStr.Valid && errStr.String != "" {
			entry += fmt.Sprintf("\n  Error: %s", errStr.String)
		}

		entries = append(entries, entry)
	}

	if len(entries) == 0 {
		return fmt.Sprintf("No history for job '%s'", name), nil
	}

	return fmt.Sprintf("History for '%s' (last 10 runs):\n\n%s", name, strings.Join(entries, "\n")), nil
}
