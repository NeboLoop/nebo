package chat

import (
	"context"
	"database/sql"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type SearchChatMessagesLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Search chat messages
func NewSearchChatMessagesLogic(ctx context.Context, svcCtx *svc.ServiceContext) *SearchChatMessagesLogic {
	return &SearchChatMessagesLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *SearchChatMessagesLogic) SearchChatMessages(req *types.SearchChatMessagesRequest) (resp *types.SearchChatMessagesResponse, err error) {
	// Get companion chat first
	chat, err := l.svcCtx.DB.GetCompanionChatByUser(l.ctx, sql.NullString{String: companionUserID, Valid: true})
	if err != nil {
		if err == sql.ErrNoRows {
			// No companion chat yet, return empty
			return &types.SearchChatMessagesResponse{
				Messages: []types.ChatMessage{},
				Total:    0,
			}, nil
		}
		l.Errorf("Failed to get companion chat: %v", err)
		return nil, err
	}

	// Set defaults
	pageSize := req.PageSize
	if pageSize <= 0 {
		pageSize = 20
	}
	page := req.Page
	if page <= 0 {
		page = 1
	}
	offset := (page - 1) * pageSize

	// Search messages
	messages, err := l.svcCtx.DB.SearchChatMessages(l.ctx, db.SearchChatMessagesParams{
		ChatID:  chat.ID,
		Column2: sql.NullString{String: req.Query, Valid: true},
		Limit:   int64(pageSize),
		Offset:  int64(offset),
	})
	if err != nil {
		l.Errorf("Failed to search messages: %v", err)
		return nil, err
	}

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

	return &types.SearchChatMessagesResponse{
		Messages: msgList,
		Total:    len(msgList),
	}, nil
}
