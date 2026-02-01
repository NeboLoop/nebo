package memory

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"strconv"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// ListMemoriesHandler returns paginated list of memories
func ListMemoriesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		namespace := r.URL.Query().Get("namespace")
		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 50)
		if pageSize > 100 {
			pageSize = 100
		}

		offset := (page - 1) * pageSize

		var memories []db.Memory
		var total int64
		var err error

		if namespace != "" {
			memories, err = svcCtx.DB.ListMemoriesByNamespace(ctx, db.ListMemoriesByNamespaceParams{
				NamespacePrefix: sql.NullString{String: namespace, Valid: true},
				Limit:           int64(pageSize),
				Offset:          int64(offset),
			})
			if err == nil {
				total, _ = svcCtx.DB.CountMemoriesByNamespace(ctx, sql.NullString{String: namespace, Valid: true})
			}
		} else {
			memories, err = svcCtx.DB.ListMemories(ctx, db.ListMemoriesParams{
				Limit:  int64(pageSize),
				Offset: int64(offset),
			})
			if err == nil {
				total, _ = svcCtx.DB.CountMemories(ctx)
			}
		}

		if err != nil {
			logging.Errorf("Failed to list memories: %v", err)
			httputil.InternalError(w, "failed to list memories")
			return
		}

		response := types.ListMemoriesResponse{
			Memories: make([]types.MemoryItem, len(memories)),
			Total:    total,
		}

		for i, m := range memories {
			response.Memories[i] = dbMemoryToType(m)
		}

		httputil.OkJSON(w, response)
	}
}

// GetMemoryHandler returns a single memory by ID
func GetMemoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
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

		memory, err := svcCtx.DB.GetMemory(ctx, id)
		if err != nil {
			if err == sql.ErrNoRows {
				httputil.NotFound(w, "memory not found")
				return
			}
			logging.Errorf("Failed to get memory: %v", err)
			httputil.InternalError(w, "failed to get memory")
			return
		}

		// Increment access count
		_ = svcCtx.DB.IncrementMemoryAccess(ctx, id)

		httputil.OkJSON(w, types.GetMemoryResponse{
			Memory: dbMemoryToType(memory),
		})
	}
}

// UpdateMemoryHandler updates a memory's value and tags
func UpdateMemoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
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

		var req types.UpdateMemoryRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		var tagsJSON sql.NullString
		if len(req.Tags) > 0 {
			data, _ := json.Marshal(req.Tags)
			tagsJSON = sql.NullString{String: string(data), Valid: true}
		}

		err = svcCtx.DB.UpdateMemory(ctx, db.UpdateMemoryParams{
			ID:    id,
			Value: toNullString(req.Value),
			Tags:  tagsJSON,
		})
		if err != nil {
			logging.Errorf("Failed to update memory: %v", err)
			httputil.InternalError(w, "failed to update memory")
			return
		}

		// Return updated memory
		memory, _ := svcCtx.DB.GetMemory(ctx, id)
		httputil.OkJSON(w, types.GetMemoryResponse{
			Memory: dbMemoryToType(memory),
		})
	}
}

// DeleteMemoryHandler deletes a memory by ID
func DeleteMemoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
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

		err = svcCtx.DB.DeleteMemory(ctx, id)
		if err != nil {
			logging.Errorf("Failed to delete memory: %v", err)
			httputil.InternalError(w, "failed to delete memory")
			return
		}

		httputil.OkJSON(w, map[string]bool{"success": true})
	}
}

// SearchMemoriesHandler searches memories by query
func SearchMemoriesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		query := r.URL.Query().Get("query")
		if query == "" {
			httputil.Error(w, nil)
			return
		}

		page := httputil.QueryInt(r, "page", 1)
		pageSize := httputil.QueryInt(r, "pageSize", 50)
		offset := (page - 1) * pageSize

		memories, err := svcCtx.DB.SearchMemories(ctx, db.SearchMemoriesParams{
			Query:  sql.NullString{String: query, Valid: true},
			Limit:  int64(pageSize),
			Offset: int64(offset),
		})
		if err != nil {
			logging.Errorf("Failed to search memories: %v", err)
			httputil.InternalError(w, "failed to search memories")
			return
		}

		response := types.SearchMemoriesResponse{
			Memories: make([]types.MemoryItem, len(memories)),
			Total:    int64(len(memories)), // Approximate - would need separate count query
		}

		for i, m := range memories {
			response.Memories[i] = dbMemoryToType(m)
		}

		httputil.OkJSON(w, response)
	}
}

// GetMemoryStatsHandler returns memory statistics
func GetMemoryStatsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		total, _ := svcCtx.DB.CountMemories(ctx)

		stats, err := svcCtx.DB.GetMemoryStats(ctx)
		if err != nil {
			logging.Errorf("Failed to get memory stats: %v", err)
			httputil.InternalError(w, "failed to get stats")
			return
		}

		namespaces, _ := svcCtx.DB.GetDistinctNamespaces(ctx)

		layerCounts := make(map[string]int64)
		for _, s := range stats {
			layerCounts[s.Layer] = s.Count
		}

		httputil.OkJSON(w, types.MemoryStatsResponse{
			TotalCount:  total,
			LayerCounts: layerCounts,
			Namespaces:  namespaces,
		})
	}
}

// Helper functions

func toNullString(s string) sql.NullString {
	if s == "" {
		return sql.NullString{}
	}
	return sql.NullString{String: s, Valid: true}
}

func dbMemoryToType(m db.Memory) types.MemoryItem {
	var tags []string
	if m.Tags.Valid && m.Tags.String != "" {
		json.Unmarshal([]byte(m.Tags.String), &tags)
	}

	item := types.MemoryItem{
		Id:          m.ID,
		Namespace:   m.Namespace,
		Key:         m.Key,
		Value:       m.Value,
		Tags:        tags,
		AccessCount: m.AccessCount.Int64,
	}

	if m.CreatedAt.Valid {
		item.CreatedAt = m.CreatedAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if m.UpdatedAt.Valid {
		item.UpdatedAt = m.UpdatedAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if m.AccessedAt.Valid {
		item.AccessedAt = m.AccessedAt.Time.Format("2006-01-02T15:04:05Z")
	}

	return item
}
