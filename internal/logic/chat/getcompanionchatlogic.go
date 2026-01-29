package chat

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/google/uuid"
	"github.com/zeromicro/go-zero/core/logx"
)

const companionUserID = "companion-default"
const defaultContextMessageLimit = 50 // Number of recent messages to load for context

type GetCompanionChatLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get companion chat (auto-creates if needed)
func NewGetCompanionChatLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetCompanionChatLogic {
	return &GetCompanionChatLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetCompanionChatLogic) GetCompanionChat() (resp *types.GetChatResponse, err error) {
	// For now, use a fixed user ID for standalone mode
	// In the future, this can be extracted from JWT context
	userID := companionUserID

	// Get or create the companion chat
	chat, err := l.svcCtx.DB.GetOrCreateCompanionChat(l.ctx, db.GetOrCreateCompanionChatParams{
		ID:     uuid.New().String(),
		UserID: sql.NullString{String: userID, Valid: true},
	})
	if err != nil {
		l.Errorf("Failed to get/create companion chat: %v", err)
		return nil, err
	}

	// Get recent messages (limited for context window)
	messages, err := l.svcCtx.DB.GetRecentChatMessages(l.ctx, db.GetRecentChatMessagesParams{
		ChatID: chat.ID,
		Limit:  defaultContextMessageLimit,
	})
	if err != nil {
		if !errors.Is(err, sql.ErrNoRows) {
			l.Errorf("Failed to get messages: %v", err)
			return nil, err
		}
		messages = nil
	}

	// Get total message count for UI (to show "X more messages in history")
	totalCount, _ := l.svcCtx.DB.CountChatMessages(l.ctx, chat.ID)

	msgList := make([]types.ChatMessage, len(messages))
	for i, m := range messages {
		metadata := ""
		if m.Metadata.Valid {
			metadata = m.Metadata.String
		}
		msgList[i] = types.ChatMessage{
			Id:        m.ID,
			ChatId:    m.ChatID,
			Role:      m.Role,
			Content:   m.Content,
			Metadata:  metadata,
			CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
		}
	}

	return &types.GetChatResponse{
		Chat: types.Chat{
			Id:        chat.ID,
			Title:     chat.Title,
			CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
			UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
		},
		Messages:      msgList,
		TotalMessages: int(totalCount),
	}, nil
}
