package tasks

import (
	"net/http"
	"strconv"

	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// getScheduler extracts the Scheduler from ServiceContext, returning nil if not wired.
func getScheduler(svcCtx *svc.ServiceContext) tools.Scheduler {
	s := svcCtx.Scheduler()
	if s == nil {
		return nil
	}
	sched, _ := s.(tools.Scheduler)
	return sched
}

// ListTasksHandler returns paginated list of tasks
func ListTasksHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 50)
		if pageSize > 100 {
			pageSize = 100
		}
		offset := (page - 1) * pageSize

		items, total, err := sched.List(r.Context(), pageSize, offset, false)
		if err != nil {
			logging.Errorf("Failed to list tasks: %v", err)
			httputil.InternalError(w, "failed to list tasks")
			return
		}

		resp := types.ListTasksResponse{
			Tasks: make([]types.TaskItem, len(items)),
			Total: total,
		}
		for i, item := range items {
			resp.Tasks[i] = scheduleItemToType(item)
		}
		httputil.OkJSON(w, resp)
	}
}

// GetTaskHandler returns a single task by name
func GetTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")
		item, err := sched.Get(r.Context(), name)
		if err != nil {
			httputil.NotFound(w, "task not found")
			return
		}

		httputil.OkJSON(w, types.GetTaskResponse{Task: scheduleItemToType(*item)})
	}
}

// CreateTaskHandler creates a new task
func CreateTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
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

		taskType := req.TaskType
		if taskType == "" {
			taskType = "message"
		}

		item, err := sched.Create(r.Context(), tools.ScheduleItem{
			Name:       req.Name,
			Expression: req.Schedule,
			TaskType:   taskType,
			Command:    req.Command,
			Message:    req.Message,
			Deliver:    req.Deliver,
			Enabled:    req.Enabled,
		})
		if err != nil {
			logging.Errorf("Failed to create task: %v", err)
			httputil.InternalError(w, "failed to create task")
			return
		}

		httputil.OkJSON(w, types.CreateTaskResponse{Task: scheduleItemToType(*item)})
	}
}

// UpdateTaskHandler updates a task by name
func UpdateTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")

		var req types.UpdateTaskRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get existing to merge partial updates
		existing, err := sched.Get(r.Context(), name)
		if err != nil {
			httputil.NotFound(w, "task not found")
			return
		}

		if req.Name != "" {
			existing.Name = req.Name
		}
		if req.Schedule != "" {
			existing.Expression = req.Schedule
		}
		if req.Command != "" {
			existing.Command = req.Command
		}
		if req.TaskType != "" {
			existing.TaskType = req.TaskType
		}
		if req.Message != "" {
			existing.Message = req.Message
		}
		if req.Deliver != "" {
			existing.Deliver = req.Deliver
		}

		updated, err := sched.Update(r.Context(), *existing)
		if err != nil {
			logging.Errorf("Failed to update task: %v", err)
			httputil.InternalError(w, "failed to update task")
			return
		}

		httputil.OkJSON(w, types.GetTaskResponse{Task: scheduleItemToType(*updated)})
	}
}

// DeleteTaskHandler deletes a task by name
func DeleteTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")
		if err := sched.Delete(r.Context(), name); err != nil {
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
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")

		// Get current state to toggle
		existing, err := sched.Get(r.Context(), name)
		if err != nil {
			httputil.NotFound(w, "task not found")
			return
		}

		if existing.Enabled {
			_, err = sched.Disable(r.Context(), name)
		} else {
			_, err = sched.Enable(r.Context(), name)
		}
		if err != nil {
			logging.Errorf("Failed to toggle task: %v", err)
			httputil.InternalError(w, "failed to toggle task")
			return
		}

		httputil.OkJSON(w, types.ToggleTaskResponse{Enabled: !existing.Enabled})
	}
}

// RunTaskHandler triggers a task immediately
func RunTaskHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")
		output, err := sched.Trigger(r.Context(), name)

		resp := types.RunTaskResponse{
			Success: err == nil,
			Output:  output,
		}
		if err != nil {
			resp.Error = err.Error()
		}

		httputil.OkJSON(w, resp)
	}
}

// ListTaskHistoryHandler returns execution history for a task
func ListTaskHistoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		sched := getScheduler(svcCtx)
		if sched == nil {
			httputil.InternalError(w, "scheduler not configured")
			return
		}

		name := httputil.PathVar(r, "name")
		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 20)
		offset := (page - 1) * pageSize

		entries, total, err := sched.History(r.Context(), name, pageSize, offset)
		if err != nil {
			logging.Errorf("Failed to list history: %v", err)
			httputil.InternalError(w, "failed to list history")
			return
		}

		resp := types.ListTaskHistoryResponse{
			History: make([]types.TaskHistoryItem, len(entries)),
			Total:   total,
		}
		for i, e := range entries {
			resp.History[i] = historyEntryToType(e)
		}

		httputil.OkJSON(w, resp)
	}
}

// scheduleItemToType maps a tools.ScheduleItem to the API response type.
func scheduleItemToType(item tools.ScheduleItem) types.TaskItem {
	t := types.TaskItem{
		Name:     item.Name,
		Schedule: item.Expression,
		Command:  item.Command,
		TaskType: item.TaskType,
		Message:  item.Message,
		Deliver:  item.Deliver,
		Enabled:  item.Enabled,
		RunCount: item.RunCount,
	}

	if n, err := strconv.ParseInt(item.ID, 10, 64); err == nil {
		t.Id = n
	}
	if !item.LastRun.IsZero() {
		t.LastRun = item.LastRun.Format("2006-01-02T15:04:05Z")
	}
	t.LastError = item.LastError
	if !item.CreatedAt.IsZero() {
		t.CreatedAt = item.CreatedAt.Format("2006-01-02T15:04:05Z")
	}
	return t
}

// historyEntryToType maps a tools.ScheduleHistoryEntry to the API response type.
func historyEntryToType(e tools.ScheduleHistoryEntry) types.TaskHistoryItem {
	t := types.TaskHistoryItem{
		Success: e.Success,
		Output:  e.Output,
		Error:   e.Error,
	}
	if n, err := strconv.ParseInt(e.ID, 10, 64); err == nil {
		t.Id = n
	}
	if !e.StartedAt.IsZero() {
		t.StartedAt = e.StartedAt.Format("2006-01-02T15:04:05Z")
	}
	if !e.FinishedAt.IsZero() {
		t.FinishedAt = e.FinishedAt.Format("2006-01-02T15:04:05Z")
	}
	return t
}
