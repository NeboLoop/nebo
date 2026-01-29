package chat

import (
	"context"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListChatsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List chats
func NewListChatsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListChatsLogic {
	return &ListChatsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListChatsLogic) ListChats(req *types.ListChatsRequest) (resp *types.ListChatsResponse, err error) {
	page := req.Page
	if page < 1 {
		page = 1
	}
	pageSize := req.PageSize
	if pageSize < 1 || pageSize > 100 {
		pageSize = 20
	}
	offset := (page - 1) * pageSize

	chats, err := l.svcCtx.DB.ListChats(l.ctx, db.ListChatsParams{
		Limit:  int64(pageSize),
		Offset: int64(offset),
	})
	if err != nil {
		l.Errorf("Failed to list chats: %v", err)
		return nil, err
	}

	total, err := l.svcCtx.DB.CountChats(l.ctx)
	if err != nil {
		l.Errorf("Failed to count chats: %v", err)
		return nil, err
	}

	chatList := make([]types.Chat, len(chats))
	for i, c := range chats {
		chatList[i] = types.Chat{
			Id:        c.ID,
			Title:     c.Title,
			CreatedAt: time.Unix(c.CreatedAt, 0).Format(time.RFC3339),
			UpdatedAt: time.Unix(c.UpdatedAt, 0).Format(time.RFC3339),
		}
	}

	return &types.ListChatsResponse{
		Chats: chatList,
		Total: int(total),
	}, nil
}
