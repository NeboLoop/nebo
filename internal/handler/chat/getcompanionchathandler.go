package chat

import (
	"database/sql"
	"encoding/json"
	"errors"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/markdown"
	"github.com/neboloop/nebo/internal/middleware"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"

	"github.com/google/uuid"
)

const companionUserIDFallback = "companion-default"
const defaultContextMessageLimit = 200 // Number of recent user+assistant messages to load for UI display

// Get companion chat (auto-creates if needed)
func GetCompanionChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Get user ID from JWT context, fall back to default for standalone mode
		userID := middleware.GetUserID(ctx)
		if userID == "" {
			userID = companionUserIDFallback
		}

		// Get or create the companion chat
		chat, err := svcCtx.DB.GetOrCreateCompanionChat(ctx, db.GetOrCreateCompanionChatParams{
			ID:     uuid.New().String(),
			UserID: sql.NullString{String: userID, Valid: true},
		})
		if err != nil {
			logging.Errorf("Failed to get/create companion chat: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get recent messages (limited for context window)
		messages, err := svcCtx.DB.GetRecentChatMessages(ctx, db.GetRecentChatMessagesParams{
			ChatID: chat.ID,
			Limit:  defaultContextMessageLimit,
		})
		if err != nil {
			if !errors.Is(err, sql.ErrNoRows) {
				logging.Errorf("Failed to get messages: %v", err)
				httputil.Error(w, err)
				return
			}
			messages = nil
		}

		// Get total message count for UI (to show "X more messages in history")
		totalCount, _ := svcCtx.DB.CountChatMessages(ctx, chat.ID)

		// Build a map of tool_call_id → result for pairing assistant tool calls with their results
		toolResultMap := buildToolResultMap(messages)

		// Filter out "tool" role messages — their data is already reconstructed
		// into assistant messages via buildMessageMetadata. Keeping them wastes
		// slots in the 50-message window and pushes user messages out of view.
		msgList := make([]types.ChatMessage, 0, len(messages))
		for _, m := range messages {
			if m.Role == "tool" {
				continue
			}
			metadata := buildMessageMetadata(m, toolResultMap)
			msgList = append(msgList, types.ChatMessage{
				Id:          m.ID,
				ChatId:      m.ChatID,
				Role:        m.Role,
				Content:     m.Content,
				ContentHtml: markdown.Render(m.Content),
				Metadata:    metadata,
				CreatedAt:   time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			})
		}

		httputil.OkJSON(w, &types.GetChatResponse{
			Chat: types.Chat{
				Id:        chat.ID,
				Title:     chat.Title,
				CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
			},
			Messages:      msgList,
			TotalMessages: int(totalCount),
		})
	}
}

// toolResultEntry stores a tool result paired with its call.
type toolResultEntry struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error,omitempty"`
}

// buildToolResultMap scans all messages for tool results and indexes them by tool_call_id.
func buildToolResultMap(messages []db.ChatMessage) map[string]toolResultEntry {
	m := make(map[string]toolResultEntry)
	for _, msg := range messages {
		if !msg.ToolResults.Valid || msg.ToolResults.String == "" {
			continue
		}
		var results []struct {
			ToolCallID string `json:"tool_call_id"`
			Content    string `json:"content"`
			IsError    bool   `json:"is_error,omitempty"`
		}
		if err := json.Unmarshal([]byte(msg.ToolResults.String), &results); err != nil {
			continue
		}
		for _, r := range results {
			m[r.ToolCallID] = toolResultEntry{Content: r.Content, IsError: r.IsError}
		}
	}
	return m
}

// buildMessageMetadata reconstructs the metadata JSON that the frontend expects from
// the tool_calls/tool_results DB columns. If the message already has metadata, returns it.
func buildMessageMetadata(m db.ChatMessage, toolResults map[string]toolResultEntry) string {
	// If metadata already exists (e.g., from a future save path), use it
	if m.Metadata.Valid && m.Metadata.String != "" {
		return m.Metadata.String
	}

	// Only assistant messages with tool_calls need reconstruction
	if m.Role != "assistant" || !m.ToolCalls.Valid || m.ToolCalls.String == "" {
		return ""
	}

	var calls []struct {
		ID    string          `json:"id"`
		Name  string          `json:"name"`
		Input json.RawMessage `json:"input"`
	}
	if err := json.Unmarshal([]byte(m.ToolCalls.String), &calls); err != nil || len(calls) == 0 {
		return ""
	}

	// Build toolCalls array matching the frontend format
	type uiToolCall struct {
		ID     string `json:"id,omitempty"`
		Name   string `json:"name"`
		Input  string `json:"input"`
		Output string `json:"output,omitempty"`
		Status string `json:"status"`
	}
	uiCalls := make([]uiToolCall, len(calls))
	for i, tc := range calls {
		uiCalls[i] = uiToolCall{
			ID:     tc.ID,
			Name:   tc.Name,
			Input:  string(tc.Input),
			Status: "complete",
		}
		if result, ok := toolResults[tc.ID]; ok {
			uiCalls[i].Output = result.Content
			if result.IsError {
				uiCalls[i].Status = "error"
			}
		}
	}

	// Build contentBlocks: text (if any content) then tool blocks
	type uiContentBlock struct {
		Type          string `json:"type"`
		Text          string `json:"text,omitempty"`
		ToolCallIndex *int   `json:"toolCallIndex,omitempty"`
	}
	var blocks []uiContentBlock
	if m.Content != "" {
		blocks = append(blocks, uiContentBlock{Type: "text", Text: m.Content})
	}
	for i := range calls {
		idx := i
		blocks = append(blocks, uiContentBlock{Type: "tool", ToolCallIndex: &idx})
	}

	meta := map[string]any{
		"toolCalls":     uiCalls,
		"contentBlocks": blocks,
	}
	data, err := json.Marshal(meta)
	if err != nil {
		return ""
	}
	return string(data)
}
