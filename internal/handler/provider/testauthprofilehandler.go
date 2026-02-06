package provider

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	models "github.com/nebolabs/nebo/internal/provider"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// HTTP client with timeout for API testing
var testClient = &http.Client{
	Timeout: 30 * time.Second,
}

// Test auth profile (verify API key works)
func TestAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.TestAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		profile, err := svcCtx.DB.GetAuthProfile(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Test the API key based on provider
		var resp *types.TestAuthProfileResponse
		switch profile.Provider {
		case "anthropic":
			resp, err = testAnthropic(profile.ApiKey, profile.Model.String)
		case "openai":
			resp, err = testOpenAI(profile.ApiKey, profile.Model.String)
		case "google":
			resp, err = testGoogle(profile.ApiKey, profile.Model.String)
		case "ollama":
			resp, err = testOllama(profile.BaseUrl.String, profile.Model.String)
		default:
			resp = &types.TestAuthProfileResponse{
				Success: false,
				Message: fmt.Sprintf("Unknown provider: %s", profile.Provider),
			}
		}

		if err != nil {
			httputil.Error(w, err)
			return
		}
		httputil.OkJSON(w, resp)
	}
}

func testAnthropic(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("anthropic")
	}
	// Fallback to a known model if still empty (e.g., during initial setup)
	if model == "" {
		model = "claude-sonnet-4-20250514"
	}

	payload := map[string]interface{}{
		"model":      model,
		"max_tokens": 10,
		"messages":   []map[string]string{{"role": "user", "content": "Hi"}},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context with longer timeout for API calls
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

func testOpenAI(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("openai")
	}
	// Fallback to a known model if still empty (e.g., during initial setup)
	if model == "" {
		model = "gpt-4o"
	}

	payload := map[string]interface{}{
		"model":                 model,
		"max_completion_tokens": 10,
		"messages":              []map[string]string{{"role": "user", "content": "Hi"}},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context with longer timeout for API calls
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

func testGoogle(apiKey, model string) (*types.TestAuthProfileResponse, error) {
	if model == "" {
		model = models.GetDefaultModel("google")
	}
	// Fallback to a known model if still empty (e.g., during initial setup)
	if model == "" {
		model = "gemini-2.0-flash"
	}

	url := fmt.Sprintf("https://generativelanguage.googleapis.com/v1beta/models/%s:generateContent?key=%s", model, apiKey)
	payload := map[string]interface{}{
		"contents": []map[string]interface{}{
			{"parts": []map[string]string{{"text": "Hi"}}},
		},
	}
	body, _ := json.Marshal(payload)

	// Use fresh context with longer timeout for API calls
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

func testOllama(baseUrl, model string) (*types.TestAuthProfileResponse, error) {
	if baseUrl == "" {
		baseUrl = "http://localhost:11434"
	}
	if model == "" {
		model = models.GetDefaultModel("ollama")
	}
	// Fallback to a known model if still empty (e.g., during initial setup)
	if model == "" {
		model = "qwen3:4b"
	}

	// Use fresh context with longer timeout for API calls
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
