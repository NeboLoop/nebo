package local

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/hybridgroup/yzma/pkg/llama"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
)

// ChatProvider implements ai.Provider using yzma (llama.cpp via purego).
// It loads a GGUF chat model and generates responses locally with no external
// dependencies. Streaming is supported; tool calling is not (4B models are
// unreliable for structured tool output).
type ChatProvider struct {
	manager *Manager
	spec    ModelSpec

	model llama.Model
	vocab llama.Vocab

	mu     sync.Mutex
	loaded bool
}

// NewChatProvider creates a local chat provider.
func NewChatProvider(manager *Manager, spec ModelSpec) *ChatProvider {
	return &ChatProvider{
		manager: manager,
		spec:    spec,
	}
}

// Init loads the chat model. Safe to call multiple times.
func (p *ChatProvider) Init() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.loaded {
		return nil
	}

	// Ensure yzma runtime is initialized
	if err := p.manager.Init(); err != nil {
		return fmt.Errorf("init yzma: %w", err)
	}

	// Download model if not present
	modelPath, err := p.manager.EnsureModel(p.spec)
	if err != nil {
		return fmt.Errorf("ensure model: %w", err)
	}

	// Load model with GPU offloading
	params := llama.ModelDefaultParams()
	params.NGpuLayers = 99 // Offload everything (Metal on macOS)

	model, err := llama.ModelLoadFromFile(modelPath, params)
	if err != nil {
		return fmt.Errorf("load model %s: %w", p.spec.Name, err)
	}

	p.model = model
	p.vocab = llama.ModelGetVocab(model)
	p.loaded = true

	fmt.Printf("[Local] Chat model loaded: %s (ctx_train=%d)\n",
		p.spec.Name, llama.ModelNCtxTrain(model))
	return nil
}

// ID returns the provider identifier.
func (p *ChatProvider) ID() string {
	return "local"
}

// ProfileID returns empty — local providers don't need auth profile tracking.
func (p *ChatProvider) ProfileID() string {
	return ""
}

// Stream sends a request and returns a channel of streaming events.
// Implements ai.Provider interface.
func (p *ChatProvider) Stream(ctx context.Context, req *ai.ChatRequest) (<-chan ai.StreamEvent, error) {
	if err := p.Init(); err != nil {
		return nil, err
	}

	resultCh := make(chan ai.StreamEvent, 100)

	go func() {
		defer close(resultCh)

		p.mu.Lock()
		model := p.model
		vocab := p.vocab
		p.mu.Unlock()

		// Build prompt from chat template
		prompt, err := p.buildPrompt(model, vocab, req)
		if err != nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		// Tokenize the prompt
		tokens := llama.Tokenize(vocab, prompt, false, true)
		if len(tokens) == 0 {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
			return
		}

		// Create inference context
		ctxParams := llama.ContextDefaultParams()
		ctxSize := uint32(llama.ModelNCtxTrain(model))
		if ctxSize > 8192 {
			ctxSize = 8192 // Cap context for memory efficiency
		}
		ctxParams.NCtx = ctxSize
		ctxParams.NBatch = 512
		ctxParams.NUbatch = 512
		ctxParams.NThreads = 4

		lctx, err := llama.InitFromModel(model, ctxParams)
		if err != nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("create context: %w", err)}
			return
		}
		defer llama.Free(lctx)

		// Truncate prompt tokens if needed
		maxPrompt := int(ctxSize) - 256 // Reserve space for generation
		if len(tokens) > maxPrompt {
			tokens = tokens[len(tokens)-maxPrompt:]
		}

		// Build sampler chain
		sampler := p.buildSampler(vocab, req)
		defer llama.SamplerFree(sampler)

		// Determine max tokens
		maxTokens := 4096
		if req.MaxTokens > 0 {
			maxTokens = req.MaxTokens
		}

		// Process prompt in a single batch
		batch := llama.BatchGetOne(tokens)
		if _, err := llama.Decode(lctx, batch); err != nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("prompt decode: %w", err)}
			return
		}

		// Autoregressive generation loop
		for i := 0; i < maxTokens; i++ {
			// Check cancellation
			select {
			case <-ctx.Done():
				resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
				return
			default:
			}

			// Sample next token
			token := llama.SamplerSample(sampler, lctx, -1)
			llama.SamplerAccept(sampler, token)

			// Check for end-of-generation
			if llama.VocabIsEOG(vocab, token) {
				break
			}

			// Convert token to text
			piece := llama.Detokenize(vocab, []llama.Token{token}, false, true)
			if piece != "" {
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeText,
					Text: piece,
				}
			}

			// Decode the new token for next iteration
			nextBatch := llama.BatchGetOne([]llama.Token{token})
			if _, err := llama.Decode(lctx, nextBatch); err != nil {
				resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("decode step %d: %w", i, err)}
				return
			}
		}

		resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
	}()

	return resultCh, nil
}

// Close releases the chat model resources.
func (p *ChatProvider) Close() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.loaded && p.model != 0 {
		llama.ModelFree(p.model)
		p.model = 0
		p.loaded = false
	}
}

// buildPrompt applies the model's chat template to the request messages.
func (p *ChatProvider) buildPrompt(model llama.Model, vocab llama.Vocab, req *ai.ChatRequest) (string, error) {
	// Get the model's built-in chat template
	tmpl := llama.ModelChatTemplate(model, "")
	if tmpl == "" {
		// Fallback: build a simple prompt
		return p.buildSimplePrompt(req), nil
	}

	// Build chat message list
	var msgs []llama.ChatMessage

	// System message
	if req.System != "" {
		msgs = append(msgs, llama.NewChatMessage("system", req.System))
	}

	// Conversation messages
	for _, msg := range req.Messages {
		switch msg.Role {
		case "user":
			msgs = append(msgs, llama.NewChatMessage("user", msg.Content))
		case "assistant":
			content := msg.Content
			// Include tool call info as text since we don't have structured tool support
			if len(msg.ToolCalls) > 0 {
				var calls []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &calls); err == nil {
					for _, tc := range calls {
						content += fmt.Sprintf("\n[Called tool: %s]", tc.Name)
					}
				}
			}
			if content != "" {
				msgs = append(msgs, llama.NewChatMessage("assistant", content))
			}
		case "system":
			msgs = append(msgs, llama.NewChatMessage("system", msg.Content))
		case "tool":
			// Represent tool results as user messages for simple models
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					for _, r := range results {
						msgs = append(msgs, llama.NewChatMessage("user",
							fmt.Sprintf("[Tool result: %s]", r.Content)))
					}
				}
			}
		}
	}

	// Apply template with assistant prompt generation
	buf := make([]byte, 0, 32*1024) // 32KB initial buffer
	n := llama.ChatApplyTemplate(tmpl, msgs, true, buf)
	if n <= 0 {
		return p.buildSimplePrompt(req), nil
	}

	// Resize buffer if needed and retry
	if int(n) > cap(buf) {
		buf = make([]byte, n)
		n = llama.ChatApplyTemplate(tmpl, msgs, true, buf)
	}

	if n > 0 && int(n) <= len(buf) {
		return string(buf[:n]), nil
	}

	// Fallback
	return p.buildSimplePrompt(req), nil
}

// buildSimplePrompt creates a basic prompt without templates.
func (p *ChatProvider) buildSimplePrompt(req *ai.ChatRequest) string {
	var prompt string

	if req.System != "" {
		prompt += "<|system|>\n" + req.System + "\n"
	}

	for _, msg := range req.Messages {
		switch msg.Role {
		case "user":
			prompt += "<|user|>\n" + msg.Content + "\n"
		case "assistant":
			prompt += "<|assistant|>\n" + msg.Content + "\n"
		case "system":
			prompt += "<|system|>\n" + msg.Content + "\n"
		}
	}

	prompt += "<|assistant|>\n"
	return prompt
}

// buildSampler creates a sampling chain appropriate for the request.
func (p *ChatProvider) buildSampler(vocab llama.Vocab, req *ai.ChatRequest) llama.Sampler {
	params := llama.SamplerChainDefaultParams()
	chain := llama.SamplerChainInit(params)

	// Temperature (default 0.7 for local models)
	temp := float32(0.7)
	if req.Temperature > 0 {
		temp = float32(req.Temperature)
	}

	// Repetition penalty to reduce loops
	llama.SamplerChainAdd(chain, llama.SamplerInitPenalties(
		64,   // lastN: consider last 64 tokens
		1.1,  // repeat penalty
		0.0,  // frequency penalty
		0.0,  // presence penalty
	))

	// Top-K → Top-P → Min-P → Temperature → Distribution
	llama.SamplerChainAdd(chain, llama.SamplerInitTopK(40))
	llama.SamplerChainAdd(chain, llama.SamplerInitTopP(0.95, 1))
	llama.SamplerChainAdd(chain, llama.SamplerInitMinP(0.05, 1))
	llama.SamplerChainAdd(chain, llama.SamplerInitTempExt(temp, 0, 1))
	llama.SamplerChainAdd(chain, llama.SamplerInitDist(0))

	return chain
}
