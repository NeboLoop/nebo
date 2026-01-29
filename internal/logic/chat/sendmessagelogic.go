package chat

import (
	"context"
	"database/sql"
	"errors"
	"strings"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/google/uuid"
	"github.com/zeromicro/go-zero/core/logx"
)

type SendMessageLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Send message (creates chat if needed)
func NewSendMessageLogic(ctx context.Context, svcCtx *svc.ServiceContext) *SendMessageLogic {
	return &SendMessageLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *SendMessageLogic) SendMessage(req *types.SendMessageRequest) (resp *types.SendMessageResponse, err error) {
	chatID := req.ChatId

	// If no chat ID, create a new chat
	if chatID == "" {
		chatID = uuid.New().String()
		// Generate title from first message (truncate to 50 chars)
		title := req.Content
		if len(title) > 50 {
			title = title[:47] + "..."
		}
		title = strings.TrimSpace(title)
		if title == "" {
			title = "New Chat"
		}

		_, err = l.svcCtx.DB.CreateChat(l.ctx, db.CreateChatParams{
			ID:    chatID,
			Title: title,
		})
		if err != nil {
			l.Errorf("Failed to create chat: %v", err)
			return nil, err
		}
	} else {
		// Verify chat exists
		_, err = l.svcCtx.DB.GetChat(l.ctx, chatID)
		if err != nil {
			if errors.Is(err, sql.ErrNoRows) {
				return nil, errors.New("chat not found")
			}
			return nil, err
		}
	}

	// Create message
	role := req.Role
	if role == "" {
		role = "user"
	}
	messageID := uuid.New().String()
	msg, err := l.svcCtx.DB.CreateChatMessage(l.ctx, db.CreateChatMessageParams{
		ID:      messageID,
		ChatID:  chatID,
		Role:    role,
		Content: req.Content,
	})
	if err != nil {
		l.Errorf("Failed to create message: %v", err)
		return nil, err
	}

	// Update chat timestamp
	_ = l.svcCtx.DB.UpdateChatTimestamp(l.ctx, chatID)

	return &types.SendMessageResponse{
		ChatId: chatID,
		Message: types.ChatMessage{
			Id:        msg.ID,
			ChatId:    msg.ChatID,
			Role:      msg.Role,
			Content:   msg.Content,
			CreatedAt: time.Unix(msg.CreatedAt, 0).Format(time.RFC3339),
		},
	}, nil
}
