package auth

import (
	"fmt"
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/local"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

func ForgotPasswordHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ForgotPasswordRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		token, err := svcCtx.Auth.CreatePasswordResetToken(r.Context(), req.Email)
		if err != nil {
			logging.Errorf("Failed to create password reset token: %v", err)
		}

		if token != "" && svcCtx.Email != nil {
			baseURL := svcCtx.Config.App.BaseURL
			resetURL := fmt.Sprintf("%s/auth/reset-password?token=%s", baseURL, token)

			_, emailErr := svcCtx.Email.SendEmail(r.Context(), local.SendEmailRequest{
				To:      req.Email,
				Subject: "Reset your password",
				Body: fmt.Sprintf(`
<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body style="font-family: sans-serif; max-width: 600px; margin: 0 auto; padding: 20px;">
	<h1 style="color: #333;">Reset Your Password</h1>
	<p>You requested to reset your password. Click the button below to set a new password:</p>
	<p style="margin: 30px 0;">
		<a href="%s" style="background-color: #4F46E5; color: white; padding: 12px 24px; text-decoration: none; border-radius: 6px;">
			Reset Password
		</a>
	</p>
	<p style="color: #666; font-size: 14px;">This link will expire in 1 hour.</p>
	<p style="color: #666; font-size: 14px;">If you didn't request this, you can safely ignore this email.</p>
</body>
</html>`, resetURL),
				TextBody: fmt.Sprintf("Reset your password by visiting: %s\n\nThis link will expire in 1 hour.\n\nIf you didn't request this, you can safely ignore this email.", resetURL),
			})

			if emailErr != nil {
				logging.Errorf("Failed to send password reset email: %v", emailErr)
			} else {
				logging.Infof("Password reset email sent to %s", req.Email)
			}
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "If an account with that email exists, a password reset link has been sent.",
		})
	}
}
