package embeddings

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// OpenAIProvider provides embeddings via OpenAI API
type OpenAIProvider struct {
	apiKey     string
	model      string
	dimensions int
	baseURL    string
	client     *http.Client
}

// OpenAIConfig configures the OpenAI provider
type OpenAIConfig struct {
	APIKey     string
	Model      string // default: text-embedding-3-small
	Dimensions int    // default: 1536
	BaseURL    string // default: https://api.openai.com/v1
}

// NewOpenAIProvider creates a new OpenAI embedding provider
func NewOpenAIProvider(cfg OpenAIConfig) *OpenAIProvider {
	if cfg.Model == "" {
		cfg.Model = "text-embedding-3-small"
	}
	if cfg.Dimensions == 0 {
		cfg.Dimensions = 1536
	}
	if cfg.BaseURL == "" {
		cfg.BaseURL = "https://api.openai.com/v1"
	}

	return &OpenAIProvider{
		apiKey:     cfg.APIKey,
		model:      cfg.Model,
		dimensions: cfg.Dimensions,
		baseURL:    cfg.BaseURL,
		client: &http.Client{
			Timeout: 60 * time.Second,
		},
	}
}

func (p *OpenAIProvider) Embed(ctx context.Context, texts []string) ([][]float32, error) {
	if len(texts) == 0 {
		return nil, nil
	}

	reqBody := map[string]any{
		"input":      texts,
		"model":      p.model,
		"dimensions": p.dimensions,
	}

	body, err := json.Marshal(reqBody)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, "POST", p.baseURL+"/embeddings", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+p.apiKey)

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("OpenAI API error: %s - %s", resp.Status, string(respBody))
	}

	var result struct {
		Data []struct {
			Embedding []float32 `json:"embedding"`
			Index     int       `json:"index"`
		} `json:"data"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}

	// Sort by index and extract embeddings
	embeddings := make([][]float32, len(texts))
	for _, item := range result.Data {
		if item.Index < len(embeddings) {
			embeddings[item.Index] = item.Embedding
		}
	}

	return embeddings, nil
}

func (p *OpenAIProvider) Dimensions() int {
	return p.dimensions
}

func (p *OpenAIProvider) Model() string {
	return p.model
}

// OllamaProvider provides embeddings via Ollama API
type OllamaProvider struct {
	baseURL    string
	model      string
	dimensions int
	client     *http.Client
}

// OllamaConfig configures the Ollama provider
type OllamaConfig struct {
	BaseURL    string // default: http://localhost:11434
	Model      string // default: qwen3-embedding
	Dimensions int    // default: 256 (qwen3-embedding supports 32-1024)
}

// NewOllamaProvider creates a new Ollama embedding provider
func NewOllamaProvider(cfg OllamaConfig) *OllamaProvider {
	if cfg.BaseURL == "" {
		cfg.BaseURL = "http://localhost:11434"
	}
	if cfg.Model == "" {
		cfg.Model = "qwen3-embedding"
	}
	if cfg.Dimensions == 0 {
		cfg.Dimensions = 256
	}

	return &OllamaProvider{
		baseURL:    cfg.BaseURL,
		model:      cfg.Model,
		dimensions: cfg.Dimensions,
		client: &http.Client{
			Timeout: 120 * time.Second,
		},
	}
}

func (p *OllamaProvider) Embed(ctx context.Context, texts []string) ([][]float32, error) {
	if len(texts) == 0 {
		return nil, nil
	}

	// Use the newer /api/embed endpoint which supports batch input and dimensions
	reqBody := map[string]any{
		"model": p.model,
		"input": texts,
	}
	if p.dimensions > 0 {
		reqBody["dimensions"] = p.dimensions
	}

	body, err := json.Marshal(reqBody)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, "POST", p.baseURL+"/api/embed", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}

	req.Header.Set("Content-Type", "application/json")

	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("Ollama API error: %s - %s", resp.Status, string(respBody))
	}

	var result struct {
		Embeddings [][]float32 `json:"embeddings"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}

	if len(result.Embeddings) != len(texts) {
		return nil, fmt.Errorf("expected %d embeddings, got %d", len(texts), len(result.Embeddings))
	}

	return result.Embeddings, nil
}

func (p *OllamaProvider) Dimensions() int {
	return p.dimensions
}

func (p *OllamaProvider) Model() string {
	return p.model
}
