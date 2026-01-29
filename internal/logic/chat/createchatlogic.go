package chat

import (
	"context"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/google/uuid"
	"github.com/zeromicro/go-zero/core/logx"
)

type CreateChatLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Create new chat
func NewCreateChatLogic(ctx context.Context, svcCtx *svc.ServiceContext) *CreateChatLogic {
	return &CreateChatLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *CreateChatLogic) CreateChat(req *types.CreateChatRequest) (resp *types.CreateChatResponse, err error) {
	chatID := uuid.New().String()
	title := req.Title
	if title == "" {
		title = "New Chat"
	}

	chat, err := l.svcCtx.DB.CreateChat(l.ctx, db.CreateChatParams{
		ID:    chatID,
		Title: title,
	})
	if err != nil {
		l.Errorf("Failed to create chat: %v", err)
		return nil, err
	}

	return &types.CreateChatResponse{
		Chat: types.Chat{
			Id:        chat.ID,
			Title:     chat.Title,
			CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
			UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
		},
	}, nil
}
