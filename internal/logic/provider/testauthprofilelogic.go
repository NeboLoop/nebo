package provider

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	models "gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

// HTTP client with timeout for API testing
var testClient = &http.Client{
	Timeout: 30 * time.Second,
}

type TestAuthProfileLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Test auth profile (verify API key works)
func NewTestAuthProfileLogic(ctx context.Context, svcCtx *svc.ServiceContext) *TestAuthProfileLogic {
	return &TestAuthProfileLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *TestAuthProfileLogic) TestAuthProfile(req *types.TestAuthProfileRequest) (resp *types.TestAuthProfileResponse, err error) {
	profile, err := l.svcCtx.DB.GetAuthProfile(l.ctx, req.Id)
	if err != nil {
		return nil, err
	}

	// Test the API key based on provider
	switch profile.Provider {
	case "anthropic":
		return l.testAnthropic(profile.ApiKey, profile.Model.String)
	case "openai":
		return l.testOpenAI(profile.ApiKey, profile.Model.String)
	case "google":
		return l.testGoogle(profile.ApiKey, profile.Model.String)
	case "ollama":
		return l.testOllama(profile.BaseUrl.String, profile.Model.String)
	default:
		return &types.TestAuthProfileResponse{
			Success: false,
			Message: fmt.Sprintf("Unknown provider: %s", profile.Provider),
		}, nil
	}
}

func (l *TestAuthProfileLogic) testAnthropic(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("anthropic")
	}

	payload := map[string]interface{}{
		"model":      model,
		"max_tokens": 10,
		"messages":   []map[string]string{{"role": "user", "content": "Hi"}},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context - go-zero's request context has a 3s timeout
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	req, _ := http.NewRequestWithContext(ctx, "POST", "https://api.anthropic.com/v1/messages", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("x-api-key", apiKey)
	req.Header.Set("anthropic-version", "2023-06-01")

	resp, err := testClient.Do(req)
	if err != nil {
		return &types.TestAuthProfileResponse{Success: false, Message: err.Error()}, nil
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		return &types.TestAuthProfileResponse{Success: true, Message: "API key is valid", Model: model}, nil
	}

	respBody, _ := io.ReadAll(resp.Body)
	return &types.TestAuthProfileResponse{Success: false, Message: fmt.Sprintf("API error: %s", string(respBody))}, nil
}

func (l *TestAuthProfileLogic) testOpenAI(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("openai")
	}

	payload := map[string]interface{}{
		"model":                 model,
		"max_completion_tokens": 10,
		"messages":              []map[string]string{{"role": "user", "content": "Hi"}},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context - go-zero's request context has a 3s timeout
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	req, _ := http.NewRequestWithContext(ctx, "POST", "https://api.openai.com/v1/chat/completions", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+apiKey)

	resp, err := testClient.Do(req)
	if err != nil {
		return &types.TestAuthProfileResponse{Success: false, Message: err.Error()}, nil
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		return &types.TestAuthProfileResponse{Success: true, Message: "API key is valid", Model: model}, nil
	}

	respBody, _ := io.ReadAll(resp.Body)
	return &types.TestAuthProfileResponse{Success: false, Message: fmt.Sprintf("API error: %s", string(respBody))}, nil
}

func (l *TestAuthProfileLogic) testGoogle(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("google")
	}

	url := fmt.Sprintf("https://generativelanguage.googleapis.com/v1beta/models/%s:generateContent?key=%s", model, apiKey)
	payload := map[string]interface{}{
		"contents": []map[string]interface{}{
			{"parts": []map[string]string{{"text": "Hi"}}},
		},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context - go-zero's request context has a 3s timeout
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	req, _ := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")

	resp, err := testClient.Do(req)
	if err != nil {
		return &types.TestAuthProfileResponse{Success: false, Message: err.Error()}, nil
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		return &types.TestAuthProfileResponse{Success: true, Message: "API key is valid", Model: model}, nil
	}

	respBody, _ := io.ReadAll(resp.Body)
	return &types.TestAuthProfileResponse{Success: false, Message: fmt.Sprintf("API error: %s", string(respBody))}, nil
}

func (l *TestAuthProfileLogic) testOllama(baseUrl, model string) (*types.TestAuthProfileResponse, error) {
	if baseUrl == "" {
		baseUrl = "http://localhost:11434"
	}
	if model == "" {
		model = models.GetDefaultModel("ollama")
	}

	// Use fresh context - go-zero's request context has a 3s timeout
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Check if Ollama is running by listing models
	req, _ := http.NewRequestWithContext(ctx, "GET", baseUrl+"/api/tags", nil)
	resp, err := testClient.Do(req)
	if err != nil {
		return &types.TestAuthProfileResponse{Success: false, Message: fmt.Sprintf("Cannot connect to Ollama at %s: %v", baseUrl, err)}, nil
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		return &types.TestAuthProfileResponse{Success: true, Message: "Ollama is running", Model: model}, nil
	}

	return &types.TestAuthProfileResponse{Success: false, Message: "Ollama not responding"}, nil
}
