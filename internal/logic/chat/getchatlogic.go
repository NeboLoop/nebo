package chat

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetChatLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get chat with messages
func NewGetChatLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetChatLogic {
	return &GetChatLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetChatLogic) GetChat(req *types.GetChatRequest) (resp *types.GetChatResponse, err error) {
	// Get chat
	chat, err := l.svcCtx.DB.GetChat(l.ctx, req.Id)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, errors.New("chat not found")
		}
		l.Errorf("Failed to get chat: %v", err)
		return nil, err
	}

	// Get messages
	messages, err := l.svcCtx.DB.GetChatMessages(l.ctx, req.Id)
	if err != nil {
		l.Errorf("Failed to get messages: %v", err)
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

	return &types.GetChatResponse{
		Chat: types.Chat{
			Id:        chat.ID,
			Title:     chat.Title,
			CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
			UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
		},
		Messages: msgList,
	}, nil
}
