package auth

import (
	"context"
	"errors"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type DevLoginLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Dev auto-login (local development only)
func NewDevLoginLogic(ctx context.Context, svcCtx *svc.ServiceContext) *DevLoginLogic {
	return &DevLoginLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *DevLoginLogic) DevLogin() (resp *types.LoginResponse, err error) {
	// Try to get a user - first try test@example.com, then try admin email pattern
	emails := []string{"test@example.com", "admin@localhost", "alma.tuck@gmail.com"}

	var userID, userEmail string
	for _, email := range emails {
		user, err := l.svcCtx.DB.GetUserByEmail(l.ctx, email)
		if err == nil {
			userID = user.ID
			userEmail = user.Email
			break
		}
	}

	if userID == "" {
		return nil, errors.New("no users found - run setup first")
	}

	// Generate tokens for the user
	authResp, err := l.svcCtx.Auth.GenerateTokensForUser(l.ctx, userID, userEmail)
	if err != nil {
		return nil, err
	}

	l.Logger.Infof("Dev login: auto-logged in as %s", userEmail)

	return &types.LoginResponse{
		Token:        authResp.Token,
		RefreshToken: authResp.RefreshToken,
		ExpiresAt:    authResp.ExpiresAt.Unix() * 1000, // Convert to milliseconds for JS
	}, nil
}
