package tasks

import (
	"database/sql"
	"net/http"
	"strconv"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// ListTasksHandler returns paginated list of tasks
func ListTasksHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 50)
		if pageSize > 100 {
			pageSize = 100
		}
		offset := (page - 1) * pageSize

		tasks, err := svcCtx.DB.ListCronJobs(ctx, db.ListCronJobsParams{
			Limit:  int64(pageSize),
			Offset: int64(offset),
		})
		if err != nil {
			logging.Errorf("Failed to list tasks: %v", err)
			httputil.InternalError(w, "failed to list tasks")
			return
		}

		total, _ := svcCtx.DB.CountCronJobs(ctx)

		response := types.ListTasksResponse{
			Tasks: make([]types.TaskItem, len(tasks)),
			Total: total,
		}

		for i, t := range tasks {
			response.Tasks[i] = dbCronJobToType(t)
		}

		httputil.OkJSON(w, response)
	}
}

// GetTaskHandler returns a single task by ID
func GetTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		task, err := svcCtx.DB.GetCronJob(ctx, id)
		if err != nil {
			if err == sql.ErrNoRows {
				httputil.NotFound(w, "task not found")
				return
			}
			logging.Errorf("Failed to get task: %v", err)
			httputil.InternalError(w, "failed to get task")
			return
		}

		httputil.OkJSON(w, types.GetTaskResponse{
			Task: dbCronJobToType(task),
		})
	}
}

// CreateTaskHandler creates a new task
func CreateTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		var req types.CreateTaskRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if req.Name == "" {
			httputil.BadRequest(w, "name is required")
			return
		}
		if req.Schedule == "" {
			httputil.BadRequest(w, "schedule is required")
			return
		}

		enabled := int64(0)
		if req.Enabled {
			enabled = 1
		}

		taskType := req.TaskType
		if taskType == "" {
			taskType = "message"
		}

		task, err := svcCtx.DB.CreateCronJob(ctx, db.CreateCronJobParams{
			Name:     req.Name,
			Schedule: req.Schedule,
			Command:  req.Command,
			TaskType: taskType,
			Message:  toNullString(req.Message),
			Deliver:  toNullString(req.Deliver),
			Enabled:  sql.NullInt64{Int64: enabled, Valid: true},
		})
		if err != nil {
			logging.Errorf("Failed to create task: %v", err)
			httputil.InternalError(w, "failed to create task")
			return
		}

		httputil.OkJSON(w, types.CreateTaskResponse{
			Task: dbCronJobToType(task),
		})
	}
}

// UpdateTaskHandler updates a task
func UpdateTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		var req types.UpdateTaskRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		err = svcCtx.DB.UpdateCronJob(ctx, db.UpdateCronJobParams{
			ID:       id,
			Name:     toNullString(req.Name),
			Schedule: toNullString(req.Schedule),
			Command:  toNullString(req.Command),
			TaskType: toNullString(req.TaskType),
			Message:  toNullString(req.Message),
			Deliver:  toNullString(req.Deliver),
		})
		if err != nil {
			logging.Errorf("Failed to update task: %v", err)
			httputil.InternalError(w, "failed to update task")
			return
		}

		// Return updated task
		task, _ := svcCtx.DB.GetCronJob(ctx, id)
		httputil.OkJSON(w, types.GetTaskResponse{
			Task: dbCronJobToType(task),
		})
	}
}

// DeleteTaskHandler deletes a task
func DeleteTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		err = svcCtx.DB.DeleteCronJob(ctx, id)
		if err != nil {
			logging.Errorf("Failed to delete task: %v", err)
			httputil.InternalError(w, "failed to delete task")
			return
		}

		httputil.OkJSON(w, map[string]bool{"success": true})
	}
}

// ToggleTaskHandler toggles a task's enabled status
func ToggleTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		err = svcCtx.DB.ToggleCronJob(ctx, id)
		if err != nil {
			logging.Errorf("Failed to toggle task: %v", err)
			httputil.InternalError(w, "failed to toggle task")
			return
		}

		// Return updated status
		task, _ := svcCtx.DB.GetCronJob(ctx, id)
		httputil.OkJSON(w, types.ToggleTaskResponse{
			Enabled: task.Enabled.Int64 == 1,
		})
	}
}

// RunTaskHandler runs a task immediately
func RunTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		task, err := svcCtx.DB.GetCronJob(ctx, id)
		if err != nil {
			if err == sql.ErrNoRows {
				httputil.NotFound(w, "task not found")
				return
			}
			logging.Errorf("Failed to get task: %v", err)
			httputil.InternalError(w, "failed to get task")
			return
		}

		// Create history entry
		history, err := svcCtx.DB.CreateCronHistory(ctx, id)
		if err != nil {
			logging.Errorf("Failed to create history: %v", err)
		}

		// Execute the task based on type
		var output string
		var execErr error

		switch task.TaskType {
		case "message":
			// For message type, we just record success
			// The actual message would be sent via channels
			output = "Message task triggered"
		case "bash":
			// For bash type, we would execute the command
			// For now, just log it
			output = "Bash task triggered: " + task.Command
		default:
			output = "Task triggered"
		}

		// Update history
		if history.ID != 0 {
			success := sql.NullInt64{Int64: 1, Valid: true}
			var errStr sql.NullString
			if execErr != nil {
				success = sql.NullInt64{Int64: 0, Valid: true}
				errStr = sql.NullString{String: execErr.Error(), Valid: true}
			}
			_ = svcCtx.DB.UpdateCronHistory(ctx, db.UpdateCronHistoryParams{
				Success: success,
				Output:  sql.NullString{String: output, Valid: true},
				Error:   errStr,
				ID:      history.ID,
			})
		}

		// Update last run
		var lastErr sql.NullString
		if execErr != nil {
			lastErr = sql.NullString{String: execErr.Error(), Valid: true}
		}
		_ = svcCtx.DB.UpdateCronJobLastRun(ctx, db.UpdateCronJobLastRunParams{
			ID:        id,
			LastError: lastErr,
		})

		resp := types.RunTaskResponse{
			Success: execErr == nil,
			Output:  output,
		}
		if execErr != nil {
			resp.Error = execErr.Error()
		}

		httputil.OkJSON(w, resp)
	}
}

// ListTaskHistoryHandler returns execution history for a task
func ListTaskHistoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		idStr := httputil.PathVar(r, "id")
		id, err := strconv.ParseInt(idStr, 10, 64)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 20)
		offset := (page - 1) * pageSize

		history, err := svcCtx.DB.ListCronHistory(ctx, db.ListCronHistoryParams{
			JobID:  id,
			Limit:  int64(pageSize),
			Offset: int64(offset),
		})
		if err != nil {
			logging.Errorf("Failed to list history: %v", err)
			httputil.InternalError(w, "failed to list history")
			return
		}

		total, _ := svcCtx.DB.CountCronHistory(ctx, id)

		response := types.ListTaskHistoryResponse{
			History: make([]types.TaskHistoryItem, len(history)),
			Total:   total,
		}

		for i, h := range history {
			response.History[i] = dbCronHistoryToType(h)
		}

		httputil.OkJSON(w, response)
	}
}

// Helper functions

func toNullString(s string) sql.NullString {
	if s == "" {
		return sql.NullString{}
	}
	return sql.NullString{String: s, Valid: true}
}

func dbCronJobToType(job db.CronJob) types.TaskItem {
	item := types.TaskItem{
		Id:       job.ID,
		Name:     job.Name,
		Schedule: job.Schedule,
		Command:  job.Command,
		TaskType: job.TaskType,
		Message:  job.Message.String,
		Deliver:  job.Deliver.String,
		Enabled:  job.Enabled.Int64 == 1,
		RunCount: job.RunCount.Int64,
	}

	if job.LastRun.Valid {
		item.LastRun = job.LastRun.Time.Format("2006-01-02T15:04:05Z")
	}
	if job.LastError.Valid {
		item.LastError = job.LastError.String
	}
	if job.CreatedAt.Valid {
		item.CreatedAt = job.CreatedAt.Time.Format("2006-01-02T15:04:05Z")
	}

	return item
}

func dbCronHistoryToType(h db.CronHistory) types.TaskHistoryItem {
	item := types.TaskHistoryItem{
		Id:      h.ID,
		JobId:   h.JobID,
		Success: h.Success.Int64 == 1,
	}

	if h.StartedAt.Valid {
		item.StartedAt = h.StartedAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if h.FinishedAt.Valid {
		item.FinishedAt = h.FinishedAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if h.Output.Valid {
		item.Output = h.Output.String
	}
	if h.Error.Valid {
		item.Error = h.Error.String
	}

	return item
}
