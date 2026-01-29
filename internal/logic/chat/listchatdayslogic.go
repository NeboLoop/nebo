package chat

import (
	"context"
	"database/sql"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListChatDaysLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List days with messages for history browsing
func NewListChatDaysLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListChatDaysLogic {
	return &ListChatDaysLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListChatDaysLogic) ListChatDays(req *types.ListChatDaysRequest) (resp *types.ListChatDaysResponse, err error) {
	// Get companion chat first
	chat, err := l.svcCtx.DB.GetCompanionChatByUser(l.ctx, sql.NullString{String: companionUserID, Valid: true})
	if err != nil {
		if err == sql.ErrNoRows {
			// No companion chat yet, return empty
			return &types.ListChatDaysResponse{Days: []types.DayInfo{}}, nil
		}
		l.Errorf("Failed to get companion chat: %v", err)
		return nil, err
	}

	// Set defaults
	pageSize := req.PageSize
	if pageSize <= 0 {
		pageSize = 30
	}
	page := req.Page
	if page <= 0 {
		page = 1
	}
	offset := (page - 1) * pageSize

	// Get days with message counts
	days, err := l.svcCtx.DB.GetDaysWithMessages(l.ctx, db.GetDaysWithMessagesParams{
		ChatID: chat.ID,
		Limit:  int64(pageSize),
		Offset: int64(offset),
	})
	if err != nil {
		l.Errorf("Failed to get days with messages: %v", err)
		return nil, err
	}

	dayList := make([]types.DayInfo, 0, len(days))
	for _, d := range days {
		if d.DayMarker.Valid {
			dayList = append(dayList, types.DayInfo{
				Day:          d.DayMarker.String,
				MessageCount: int(d.MessageCount),
			})
		}
	}

	return &types.ListChatDaysResponse{
		Days: dayList,
	}, nil
}
