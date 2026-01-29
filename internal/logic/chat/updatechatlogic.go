package chat

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdateChatLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update chat title
func NewUpdateChatLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdateChatLogic {
	return &UpdateChatLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdateChatLogic) UpdateChat(req *types.UpdateChatRequest) (resp *types.Chat, err error) {
	// Get chat
	chat, err := l.svcCtx.DB.GetChat(l.ctx, req.Id)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, errors.New("chat not found")
		}
		return nil, err
	}

	// Update title
	err = l.svcCtx.DB.UpdateChatTitle(l.ctx, db.UpdateChatTitleParams{
		Title: req.Title,
		ID:    req.Id,
	})
	if err != nil {
		l.Errorf("Failed to update chat: %v", err)
		return nil, err
	}

	return &types.Chat{
		Id:        chat.ID,
		Title:     req.Title,
		CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt: time.Now().Format(time.RFC3339),
	}, nil
}
