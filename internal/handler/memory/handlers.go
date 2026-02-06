package memory

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"strconv"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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

		var total int64
		var response types.ListMemoriesResponse

		if namespace != "" {
			memories, err := svcCtx.DB.ListMemoriesByNamespace(ctx, db.ListMemoriesByNamespaceParams{
				NamespacePrefix: sql.NullString{String: namespace, Valid: true},
				Limit:           int64(pageSize),
				Offset:          int64(offset),
			})
			if err == nil {
				total, _ = svcCtx.DB.CountMemoriesByNamespace(ctx, sql.NullString{String: namespace, Valid: true})
			}
			if err != nil {
				logging.Errorf("Failed to list memories: %v", err)
				httputil.InternalError(w, "failed to list memories")
				return
			}
			response = types.ListMemoriesResponse{
				Memories: make([]types.MemoryItem, len(memories)),
				Total:    total,
			}
			for i, m := range memories {
				response.Memories[i] = listMemoryByNamespaceRowToType(m)
			}
		} else {
			memories, err := svcCtx.DB.ListMemories(ctx, db.ListMemoriesParams{
				Limit:  int64(pageSize),
				Offset: int64(offset),
			})
			if err == nil {
				total, _ = svcCtx.DB.CountMemories(ctx)
			}
			if err != nil {
				logging.Errorf("Failed to list memories: %v", err)
				httputil.InternalError(w, "failed to list memories")
				return
			}
			response = types.ListMemoriesResponse{
				Memories: make([]types.MemoryItem, len(memories)),
				Total:    total,
			}
			for i, m := range memories {
				response.Memories[i] = listMemoryRowToType(m)
			}
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
			Memory: getMemoryRowToType(memory),
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
			Memory: getMemoryRowToType(memory),
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
			response.Memories[i] = searchMemoryRowToType(m)
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

// Converter functions for sqlc row types

func listMemoryRowToType(m db.ListMemoriesRow) types.MemoryItem {
	return convertMemoryRow(m.ID, m.Namespace, m.Key, m.Value, m.Tags, m.CreatedAt, m.UpdatedAt, m.AccessedAt, m.AccessCount)
}

func listMemoryByNamespaceRowToType(m db.ListMemoriesByNamespaceRow) types.MemoryItem {
	return convertMemoryRow(m.ID, m.Namespace, m.Key, m.Value, m.Tags, m.CreatedAt, m.UpdatedAt, m.AccessedAt, m.AccessCount)
}

func getMemoryRowToType(m db.GetMemoryRow) types.MemoryItem {
	return convertMemoryRow(m.ID, m.Namespace, m.Key, m.Value, m.Tags, m.CreatedAt, m.UpdatedAt, m.AccessedAt, m.AccessCount)
}

func searchMemoryRowToType(m db.SearchMemoriesRow) types.MemoryItem {
	return convertMemoryRow(m.ID, m.Namespace, m.Key, m.Value, m.Tags, m.CreatedAt, m.UpdatedAt, m.AccessedAt, m.AccessCount)
}

func convertMemoryRow(id int64, namespace, key, value string, tags sql.NullString, createdAt, updatedAt, accessedAt sql.NullTime, accessCount sql.NullInt64) types.MemoryItem {
	var tagsList []string
	if tags.Valid && tags.String != "" {
		json.Unmarshal([]byte(tags.String), &tagsList)
	}

	item := types.MemoryItem{
		Id:          id,
		Namespace:   namespace,
		Key:         key,
		Value:       value,
		Tags:        tagsList,
		AccessCount: accessCount.Int64,
	}

	if createdAt.Valid {
		item.CreatedAt = createdAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if updatedAt.Valid {
		item.UpdatedAt = updatedAt.Time.Format("2006-01-02T15:04:05Z")
	}
	if accessedAt.Valid {
		item.AccessedAt = accessedAt.Time.Format("2006-01-02T15:04:05Z")
	}

	return item
}
