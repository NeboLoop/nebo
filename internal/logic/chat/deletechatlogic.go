package chat

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type DeleteChatLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Delete chat
func NewDeleteChatLogic(ctx context.Context, svcCtx *svc.ServiceContext) *DeleteChatLogic {
	return &DeleteChatLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *DeleteChatLogic) DeleteChat(req *types.DeleteChatRequest) (resp *types.MessageResponse, err error) {
	// Delete (cascade will remove messages)
	err = l.svcCtx.DB.DeleteChat(l.ctx, req.Id)
	if err != nil {
		l.Errorf("Failed to delete chat: %v", err)
		return nil, err
	}

	return &types.MessageResponse{
		Message: "Chat deleted",
	}, nil
}
