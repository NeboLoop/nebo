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

	"github.com/nebolabs/nebo/internal/db"

	cronlib "github.com/robfig/cron/v3"
)

// AgentTaskCallback is called when an agent task cron job fires
type AgentTaskCallback func(ctx context.Context, name, message string, deliver *DeliverConfig) error

// DeliverConfig specifies where to send task results
type DeliverConfig struct {
	Channel string `json:"channel"`
	To      string `json:"to"`
}

// CronTool manages scheduled recurring tasks.
// Implements both the Tool interface (for agent use) and the Scheduler interface
// (for pluggable scheduling via the app platform).
type CronTool struct {
	queries        *db.Queries        // sqlc queries for database operations
	scheduler      *cronlib.Cron
	jobs           map[string]cronlib.EntryID
	mu             sync.RWMutex
	agentCallback  AgentTaskCallback
	triggerHandler func(ScheduleTriggerEvent)
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
// Uses sqlc queries for all database operations per architectural requirements.
func NewCronTool(cfg CronConfig) (*CronTool, error) {
	if cfg.DB == nil {
		return nil, fmt.Errorf("database connection required")
	}

	tool := &CronTool{
		queries:   db.New(cfg.DB), // Create sqlc queries from DB connection
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
	// Use sqlc to list enabled cron jobs
	jobs, err := t.queries.ListEnabledCronJobs(context.Background())
	if err != nil {
		return err
	}

	for _, job := range jobs {
		// Enabled is NullInt64, check if valid and non-zero
		if job.Enabled.Valid && job.Enabled.Int64 != 0 {
			t.scheduleJobFull(job.Name, job.Schedule, job.Command, job.TaskType, job.Message.String, job.Deliver.String)
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
	t.mu.RLock()
	th := t.triggerHandler
	cb := t.agentCallback
	t.mu.RUnlock()

	// If a trigger handler is set (Scheduler interface), use it.
	// This routes through SchedulerManager → LaneEvents, which is the new unified path.
	if th != nil {
		th(ScheduleTriggerEvent{
			Name:     name,
			TaskType: taskType,
			Command:  command,
			Message:  message,
			Deliver:  deliverJSON,
			FiredAt:  time.Now(),
		})

		// Update job stats
		t.queries.UpdateCronJobLastRunByName(context.Background(), db.UpdateCronJobLastRunByNameParams{
			Name: name,
		})
		return
	}

	// Legacy path: direct execution via agentCallback
	var err error
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()

	if taskType == "agent" {
		if cb != nil {
			var deliver *DeliverConfig
			if deliverJSON != "" {
				deliver = &DeliverConfig{}
				json.Unmarshal([]byte(deliverJSON), deliver)
			}
			err = cb(ctx, name, message, deliver)
		} else {
			err = fmt.Errorf("no agent callback configured")
		}
	} else {
		shell, shellArgs := ShellCommand()
		args := append(shellArgs, command)
		cmd := exec.CommandContext(ctx, shell, args...)
		_, err = cmd.CombinedOutput()
	}

	var lastError sql.NullString
	if err != nil {
		lastError = sql.NullString{String: err.Error(), Valid: true}
	}
	t.queries.UpdateCronJobLastRunByName(ctx, db.UpdateCronJobLastRunByNameParams{
		Name:      name,
		LastError: lastError,
	})
}

// SetAgentCallback sets the callback for agent task execution
func (t *CronTool) SetAgentCallback(cb AgentTaskCallback) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.agentCallback = cb
}

// Close stops the scheduler. Database is shared and managed elsewhere.
func (t *CronTool) Close() error {
	if t.scheduler != nil {
		t.scheduler.Stop()
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
	var deliverJSON sql.NullString
	if params.Deliver != nil {
		data, _ := json.Marshal(params.Deliver)
		deliverJSON = sql.NullString{String: string(data), Valid: true}
	}

	// Insert or update using sqlc
	err = t.queries.UpsertCronJob(context.Background(), db.UpsertCronJobParams{
		Name:     params.Name,
		Schedule: params.Schedule,
		Command:  params.Command,
		TaskType: taskType,
		Message:  sql.NullString{String: params.Message, Valid: params.Message != ""},
		Deliver:  deliverJSON,
		Enabled:  sql.NullInt64{Int64: 1, Valid: true},
	})
	if err != nil {
		return "", err
	}

	// Schedule the job
	if err := t.scheduleJobFull(params.Name, params.Schedule, params.Command, taskType, params.Message, deliverJSON.String); err != nil {
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
	// Use sqlc to list all cron jobs
	cronJobs, err := t.queries.ListCronJobs(context.Background(), db.ListCronJobsParams{
		Limit:  1000, // Reasonable limit for CLI display
		Offset: 0,
	})
	if err != nil {
		return "", err
	}

	var jobs []string
	for _, job := range cronJobs {
		enabled := job.Enabled.Valid && job.Enabled.Int64 != 0
		status := "enabled"
		if !enabled {
			status = "paused"
		}

		// Calculate next run
		var nextRun string
		if enabled {
			t.mu.RLock()
			if entryID, exists := t.jobs[job.Name]; exists {
				entry := t.scheduler.Entry(entryID)
				nextRun = entry.Next.Format("2006-01-02 15:04:05")
			}
			t.mu.RUnlock()
		}

		runCount := int64(0)
		if job.RunCount.Valid {
			runCount = job.RunCount.Int64
		}

		// Build job info based on task type
		var jobInfo string
		if job.TaskType == "agent" {
			jobInfo = fmt.Sprintf("- %s [%s] (agent task)\n  Schedule: %s\n  Prompt: %s\n  Runs: %d",
				job.Name, status, job.Schedule, job.Message.String, runCount)
		} else {
			jobInfo = fmt.Sprintf("- %s [%s] (bash)\n  Schedule: %s\n  Command: %s\n  Runs: %d",
				job.Name, status, job.Schedule, job.Command, runCount)
		}
		if job.LastRun.Valid {
			jobInfo += fmt.Sprintf("\n  Last run: %s", job.LastRun.Time.Format("2006-01-02 15:04:05"))
		}
		if job.LastError.Valid && job.LastError.String != "" {
			jobInfo += fmt.Sprintf("\n  Last error: %s", job.LastError.String)
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

	// Remove from database using sqlc
	result, err := t.queries.DeleteCronJobByName(context.Background(), name)
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

	// Update database using sqlc
	result, err := t.queries.DisableCronJobByName(context.Background(), name)
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

	// Get full job details using sqlc
	job, err := t.queries.GetCronJobByName(context.Background(), name)
	if err == sql.ErrNoRows {
		return fmt.Sprintf("No cron job found with name '%s'", name), nil
	}
	if err != nil {
		return "", err
	}

	// Update database using sqlc
	err = t.queries.EnableCronJobByName(context.Background(), name)
	if err != nil {
		return "", err
	}

	// Schedule the job with full data
	if err := t.scheduleJobFull(name, job.Schedule, job.Command, job.TaskType, job.Message.String, job.Deliver.String); err != nil {
		return "", err
	}

	return fmt.Sprintf("Resumed cron job '%s'", name), nil
}

func (t *CronTool) runNow(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for run action")
	}

	// Get full job details using sqlc
	job, err := t.queries.GetCronJobByName(context.Background(), name)
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

	if job.TaskType == "agent" {
		// Execute agent task via callback
		t.mu.RLock()
		cb := t.agentCallback
		t.mu.RUnlock()

		if cb == nil {
			return "", fmt.Errorf("no agent callback configured - agent tasks require the agent to be running")
		}

		var deliverCfg *DeliverConfig
		if job.Deliver.Valid && job.Deliver.String != "" {
			deliverCfg = &DeliverConfig{}
			json.Unmarshal([]byte(job.Deliver.String), deliverCfg)
		}

		err = cb(ctx, name, job.Message.String, deliverCfg)
		if err == nil {
			output = []byte("Agent task completed successfully")
		}
	} else {
		// Execute shell command using platform-specific shell
		shell, shellArgs := ShellCommand()
		args := append(shellArgs, job.Command)
		cmd := exec.CommandContext(ctx, shell, args...)
		output, err = cmd.CombinedOutput()
	}

	// Update stats using sqlc
	var lastError sql.NullString
	if err != nil {
		lastError = sql.NullString{String: err.Error(), Valid: true}
	}
	t.queries.UpdateCronJobLastRunByName(ctx, db.UpdateCronJobLastRunByNameParams{
		Name:      name,
		LastError: lastError,
	})

	if err != nil {
		return fmt.Sprintf("Job '%s' executed with error:\n%s\nOutput:\n%s", name, err.Error(), string(output)), nil
	}

	outputStr := string(output)
	if len(outputStr) > 5000 {
		outputStr = outputStr[:5000] + "\n... (truncated)"
	}

	return fmt.Sprintf("Job '%s' executed successfully.\nOutput:\n%s", name, outputStr), nil
}

// --- Scheduler interface implementation ---

// SetTriggerHandler sets the callback invoked when a schedule fires.
func (t *CronTool) SetTriggerHandler(fn func(ScheduleTriggerEvent)) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.triggerHandler = fn
}

// SchedulerCreate implements Scheduler.Create.
func (t *CronTool) SchedulerCreate(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	if item.Name == "" {
		return nil, fmt.Errorf("name is required")
	}
	if item.Expression == "" {
		return nil, fmt.Errorf("expression is required")
	}

	taskType := item.TaskType
	if taskType == "" {
		taskType = "bash"
	}
	if taskType == "bash" && item.Command == "" {
		return nil, fmt.Errorf("command is required for bash tasks")
	}
	if taskType == "agent" && item.Message == "" {
		return nil, fmt.Errorf("message is required for agent tasks")
	}

	// Validate cron expression
	parser := cronlib.NewParser(cronlib.Second | cronlib.Minute | cronlib.Hour | cronlib.Dom | cronlib.Month | cronlib.Dow)
	sched, err := parser.Parse(item.Expression)
	if err != nil {
		return nil, fmt.Errorf("invalid cron expression: %w", err)
	}

	deliverNull := sql.NullString{}
	if item.Deliver != "" {
		deliverNull = sql.NullString{String: item.Deliver, Valid: true}
	}

	err = t.queries.UpsertCronJob(ctx, db.UpsertCronJobParams{
		Name:     item.Name,
		Schedule: item.Expression,
		Command:  item.Command,
		TaskType: taskType,
		Message:  sql.NullString{String: item.Message, Valid: item.Message != ""},
		Deliver:  deliverNull,
		Enabled:  sql.NullInt64{Int64: 1, Valid: true},
	})
	if err != nil {
		return nil, err
	}

	if err := t.scheduleJobFull(item.Name, item.Expression, item.Command, taskType, item.Message, item.Deliver); err != nil {
		return nil, err
	}

	nextRun := sched.Next(time.Now())
	return &ScheduleItem{
		Name:       item.Name,
		Expression: item.Expression,
		TaskType:   taskType,
		Command:    item.Command,
		Message:    item.Message,
		Deliver:    item.Deliver,
		Enabled:    true,
		NextRun:    nextRun,
		CreatedAt:  time.Now(),
	}, nil
}

// SchedulerGet implements Scheduler.Get.
func (t *CronTool) SchedulerGet(ctx context.Context, name string) (*ScheduleItem, error) {
	job, err := t.queries.GetCronJobByName(ctx, name)
	if err == sql.ErrNoRows {
		return nil, fmt.Errorf("schedule not found: %s", name)
	}
	if err != nil {
		return nil, err
	}
	return t.dbJobToScheduleItem(job), nil
}

// SchedulerList implements Scheduler.List.
func (t *CronTool) SchedulerList(ctx context.Context, limit, offset int, enabledOnly bool) ([]ScheduleItem, int64, error) {
	if enabledOnly {
		jobs, err := t.queries.ListEnabledCronJobs(ctx)
		if err != nil {
			return nil, 0, err
		}
		items := make([]ScheduleItem, len(jobs))
		for i, job := range jobs {
			items[i] = *t.dbJobToScheduleItem(job)
		}
		return items, int64(len(items)), nil
	}

	jobs, err := t.queries.ListCronJobs(ctx, db.ListCronJobsParams{
		Limit:  int64(limit),
		Offset: int64(offset),
	})
	if err != nil {
		return nil, 0, err
	}
	items := make([]ScheduleItem, len(jobs))
	for i, job := range jobs {
		items[i] = *t.dbJobToScheduleItem(job)
	}
	return items, int64(len(items)), nil
}

// SchedulerUpdate implements Scheduler.Update.
func (t *CronTool) SchedulerUpdate(ctx context.Context, item ScheduleItem) (*ScheduleItem, error) {
	// Upsert — same as Create for CronTool since DB uses upsert
	return t.SchedulerCreate(ctx, item)
}

// SchedulerDelete implements Scheduler.Delete.
func (t *CronTool) SchedulerDelete(ctx context.Context, name string) error {
	t.mu.Lock()
	if entryID, exists := t.jobs[name]; exists {
		t.scheduler.Remove(entryID)
		delete(t.jobs, name)
	}
	t.mu.Unlock()

	_, err := t.queries.DeleteCronJobByName(ctx, name)
	return err
}

// SchedulerEnable implements Scheduler.Enable.
func (t *CronTool) SchedulerEnable(ctx context.Context, name string) (*ScheduleItem, error) {
	job, err := t.queries.GetCronJobByName(ctx, name)
	if err == sql.ErrNoRows {
		return nil, fmt.Errorf("schedule not found: %s", name)
	}
	if err != nil {
		return nil, err
	}

	if err := t.queries.EnableCronJobByName(ctx, name); err != nil {
		return nil, err
	}

	if err := t.scheduleJobFull(name, job.Schedule, job.Command, job.TaskType, job.Message.String, job.Deliver.String); err != nil {
		return nil, err
	}

	item := t.dbJobToScheduleItem(job)
	item.Enabled = true
	return item, nil
}

// SchedulerDisable implements Scheduler.Disable.
func (t *CronTool) SchedulerDisable(ctx context.Context, name string) (*ScheduleItem, error) {
	job, err := t.queries.GetCronJobByName(ctx, name)
	if err == sql.ErrNoRows {
		return nil, fmt.Errorf("schedule not found: %s", name)
	}
	if err != nil {
		return nil, err
	}

	t.mu.Lock()
	if entryID, exists := t.jobs[name]; exists {
		t.scheduler.Remove(entryID)
		delete(t.jobs, name)
	}
	t.mu.Unlock()

	if _, err := t.queries.DisableCronJobByName(ctx, name); err != nil {
		return nil, err
	}

	item := t.dbJobToScheduleItem(job)
	item.Enabled = false
	return item, nil
}

// SchedulerTrigger implements Scheduler.Trigger.
func (t *CronTool) SchedulerTrigger(ctx context.Context, name string) (string, error) {
	return t.runNow(name)
}

// SchedulerHistory implements Scheduler.History.
func (t *CronTool) SchedulerHistory(ctx context.Context, name string, limit, offset int) ([]ScheduleHistoryEntry, int64, error) {
	historyItems, err := t.queries.GetRecentCronHistoryByJobName(ctx, name)
	if err != nil {
		return nil, 0, err
	}

	entries := make([]ScheduleHistoryEntry, len(historyItems))
	for i, h := range historyItems {
		entry := ScheduleHistoryEntry{
			ID:           fmt.Sprintf("%d", h.ID),
			ScheduleName: name,
			Success:      h.Success.Valid && h.Success.Int64 != 0,
		}
		if h.StartedAt.Valid {
			entry.StartedAt = h.StartedAt.Time
		}
		if h.FinishedAt.Valid {
			entry.FinishedAt = h.FinishedAt.Time
		}
		if h.Output.Valid {
			entry.Output = h.Output.String
		}
		if h.Error.Valid {
			entry.Error = h.Error.String
		}
		entries[i] = entry
	}
	return entries, int64(len(entries)), nil
}

// --- DB row converters ---

func (t *CronTool) dbJobToScheduleItem(job db.CronJob) *ScheduleItem {
	enabled := job.Enabled.Valid && job.Enabled.Int64 != 0
	item := &ScheduleItem{
		ID:         fmt.Sprintf("%d", job.ID),
		Name:       job.Name,
		Expression: job.Schedule,
		TaskType:   job.TaskType,
		Command:    job.Command,
		Enabled:    enabled,
	}
	if job.Message.Valid {
		item.Message = job.Message.String
	}
	if job.Deliver.Valid {
		item.Deliver = job.Deliver.String
	}
	if job.LastRun.Valid {
		item.LastRun = job.LastRun.Time
	}
	if job.RunCount.Valid {
		item.RunCount = job.RunCount.Int64
	}
	if job.LastError.Valid {
		item.LastError = job.LastError.String
	}
	if job.CreatedAt.Valid {
		item.CreatedAt = job.CreatedAt.Time
	}

	// Get next run from the scheduler if job is enabled
	if enabled {
		t.mu.RLock()
		if entryID, exists := t.jobs[job.Name]; exists {
			entry := t.scheduler.Entry(entryID)
			item.NextRun = entry.Next
		}
		t.mu.RUnlock()
	}
	return item
}


// --- Tool interface private methods (kept for backwards compat) ---

func (t *CronTool) history(name string) (string, error) {
	if name == "" {
		return "", fmt.Errorf("name is required for history action")
	}

	// Use sqlc to get recent history by job name
	historyItems, err := t.queries.GetRecentCronHistoryByJobName(context.Background(), name)
	if err != nil {
		return "", err
	}

	var entries []string
	for _, h := range historyItems {
		status := "success"
		if !h.Success.Valid || h.Success.Int64 == 0 {
			status = "failed"
		}

		duration := "running"
		if h.FinishedAt.Valid && h.StartedAt.Valid {
			duration = h.FinishedAt.Time.Sub(h.StartedAt.Time).String()
		}

		startedAtStr := "unknown"
		if h.StartedAt.Valid {
			startedAtStr = h.StartedAt.Time.Format("2006-01-02 15:04:05")
		}

		entry := fmt.Sprintf("- %s [%s] (duration: %s)", startedAtStr, status, duration)
		if h.Error.Valid && h.Error.String != "" {
			entry += fmt.Sprintf("\n  Error: %s", h.Error.String)
		}

		entries = append(entries, entry)
	}

	if len(entries) == 0 {
		return fmt.Sprintf("No history for job '%s'", name), nil
	}

	return fmt.Sprintf("History for '%s' (last 10 runs):\n\n%s", name, strings.Join(entries, "\n")), nil
}
