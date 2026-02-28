package agent

import (
	"context"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
)

// GetLoopsHandler returns loop/channel hierarchy from the agent's NeboLoop connection.
func GetLoopsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		hub := svcCtx.AgentHub
		if hub == nil || hub.GetAnyAgent() == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "Agent not connected")
			return
		}

		ctx, cancel := context.WithTimeout(r.Context(), 5*time.Second)
		defer cancel()

		frame, err := hub.SendRequestSync(ctx, "get_loops", nil)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusGatewayTimeout, "Agent did not respond: "+err.Error())
			return
		}

		httputil.OkJSON(w, frame.Payload)
	}
}

// GetChannelMessagesHandler returns recent messages from a loop channel.
func GetChannelMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		hub := svcCtx.AgentHub
		if hub == nil || hub.GetAnyAgent() == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "Agent not connected")
			return
		}

		channelID := httputil.PathVar(r, "channelId")
		if channelID == "" {
			httputil.BadRequest(w, "channelId is required")
			return
		}
		limit := httputil.QueryInt(r, "limit", 50)

		ctx, cancel := context.WithTimeout(r.Context(), 10*time.Second)
		defer cancel()

		frame, err := hub.SendRequestSync(ctx, "get_channel_messages", map[string]any{
			"channel_id": channelID,
			"limit":      limit,
		})
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusGatewayTimeout, "Agent did not respond: "+err.Error())
			return
		}

		httputil.OkJSON(w, frame.Payload)
	}
}

// SendChannelMessageHandler sends a message to a loop channel.
func SendChannelMessageHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		hub := svcCtx.AgentHub
		if hub == nil || hub.GetAnyAgent() == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "Agent not connected")
			return
		}

		channelID := httputil.PathVar(r, "channelId")
		if channelID == "" {
			httputil.BadRequest(w, "channelId is required")
			return
		}

		var req struct {
			Text string `json:"text"`
		}
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if req.Text == "" {
			httputil.BadRequest(w, "text is required")
			return
		}

		ctx, cancel := context.WithTimeout(r.Context(), 10*time.Second)
		defer cancel()

		frame, err := hub.SendRequestSync(ctx, "send_channel_message", map[string]any{
			"channel_id": channelID,
			"text":       req.Text,
		})
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusGatewayTimeout, "Agent did not respond: "+err.Error())
			return
		}

		httputil.OkJSON(w, frame.Payload)
	}
}
