# ADR-001: Local Model Strategy

**Status:** Accepted  
**Date:** 2026-02-05  
**Author:** Alma Tuck

## Context

Nebo ships with local AI capabilities via Ollama for two purposes:

1. **Embeddings** — Powering the 3-tier memory system (tacit/daily/entity) with vector similarity search
2. **Local chat** — Running background lanes (sub-agents, events, heartbeat) without burning cloud API credits

The previous defaults were `nomic-embed-text` (embeddings) and `llama3.2:3b` (chat). Both are now outdated — the Qwen3 family (released mid-2025) significantly outperforms them at equivalent or smaller sizes.

This ADR also establishes a **minimum system requirement of 16GB RAM**, dropping 8GB Mac support.

## Decision

### System Requirements

**Minimum:** macOS with Apple Silicon (M1+) and **16GB RAM**

We are not targeting 8GB machines. Rationale:

- Every Mac sold since late 2023 ships with 16GB minimum (M3 Air onward)
- 8GB shared memory minus ~4GB for macOS leaves insufficient headroom for local models + Nebo + user apps
- The user experience on 8GB would be mediocre — laggy inference, memory pressure, swap thrashing
- Supporting two model tiers (1.7B for 8GB, 4B for 16GB) doubles the testing matrix for a solo dev pre-launch
- Early adopters skew toward recent hardware and power-user setups
- "Nebo makes my Mac slow" is worse press than "Nebo requires 16GB"

We'll revisit if real user demand materializes post-launch.

### Bundled Models

Nebo auto-pulls these Ollama models on first run:

| Role | Model | Size | Parameters | Context | Why |
|------|-------|------|------------|---------|-----|
| 🧠 Embeddings | `qwen3-embedding:0.6b` | 639 MB | 600M | 32K | #1 on MTEB multilingual, flexible output dimensions (32–1024), 100+ languages, Apache 2.0 |
| 💬 Local Chat | `qwen3:4b` | ~2.5 GB | 4B | 128K | Tool calling + hybrid thinking mode, "rivals Qwen2.5-72B" quality, 18.6M pulls, Apache 2.0 |
| | **Total** | **~3.1 GB** | | | |

**Memory budget on 16GB Mac:**

```
macOS + system services     ~4 GB
Nebo Go binary              ~100 MB
Ollama runtime              ~200 MB
qwen3-embedding:0.6b        ~800 MB (inference)
qwen3:4b                    ~3 GB (inference)
─────────────────────────────────
Total                       ~8.1 GB
Remaining for user apps     ~7.9 GB  ✅ Comfortable
```

### Why Qwen3 over alternatives

**Embeddings — `qwen3-embedding:0.6b` vs `nomic-embed-text`:**

| | qwen3-embedding:0.6b | nomic-embed-text |
|---|---|---|
| Quality | State-of-the-art (MTEB #1 family) | Good, but a generation behind |
| Dimensions | Flexible: 32–1024 (we use 256) | Fixed: 768 |
| Context window | 32K tokens | 8K tokens |
| Languages | 100+ | English-focused |
| Size on disk | 639 MB | 274 MB |
| Last updated | 4 months ago | 1+ year ago |
| License | Apache 2.0 | Apache 2.0 |

The 365 MB size increase is worth the quality and flexibility gains. Flexible dimensions let us use 256-dim vectors for short key-value memories, saving storage and compute vs nomic's fixed 768.

**Local chat — `qwen3:4b` vs `llama3.2:3b`:**

| | qwen3:4b | llama3.2:3b |
|---|---|---|
| Tool calling | ✅ Built-in, reliable | ✅ Supported |
| Thinking mode | ✅ Hybrid (on/off per request) | ❌ None |
| Quality | Rivals Qwen2.5-72B (per Alibaba) | Good for 3B, but ceiling is lower |
| Context | 128K | 128K |
| Size | ~2.5 GB | 2.0 GB |
| Ecosystem | 18.6M pulls, very active | 93.6M pulls, mature but stagnant |
| License | Apache 2.0 | Llama Community |

The 500 MB size increase buys noticeably better reasoning, tool-calling accuracy, and the ability to "think" when the task demands it. On a 16GB machine, this fits comfortably.

### What we considered but rejected

| Model | Size | Why not |
|-------|------|---------|
| `qwen3:1.7b` | 1.4 GB | Would be the pick for 8GB. Since we're targeting 16GB, the 4B is strictly better. |
| `cogito:3b` | 2.2 GB | Good hybrid reasoning, but smaller community and less battle-tested tool calling. |
| `granite4:3b` | 2.1 GB | Solid tool calling, but no thinking mode and IBM's ecosystem is less active. |
| `llama3.2:3b` | 2.0 GB | The previous default. Surpassed by qwen3:4b on every metric except raw download count. |
| `embeddinggemma:300m` | 622 MB | Only 2K context window — too limiting for memory chunks. |
| `nomic-embed-text` | 274 MB | Stale, English-focused, fixed 768 dims. The old default. |

## Implementation

### Code changes required

1. **`internal/agent/embeddings/providers.go`** — Update `OllamaConfig` defaults:
   - Model: `nomic-embed-text` → `qwen3-embedding`
   - Dimensions: `768` → `256` (leverage flexible dims for efficiency)

2. **`internal/agent/ai/api_ollama.go`** — Update default model:
   - `llama3.2` → `qwen3:4b`

3. **`internal/defaults/dotnebo/models.yaml`** — Update Ollama provider entries:
   - Replace `llama3.3` with `qwen3:4b` as primary
   - Add `qwen3-embedding` as embedding model reference

4. **`cmd/nebo/agent.go`** — Update log messages:
   - `"using Ollama nomic-embed-text"` → `"using Ollama qwen3-embedding"`

5. **`internal/handler/provider/testauthprofilehandler.go`** — Update test model:
   - `llama3.2` → `qwen3:4b`

6. ✅ **Auto-pull on first run** — `ai.EnsureOllamaModel()` checks if a model exists locally and pulls it if not. Called from `createEmbeddingService` (for `qwen3-embedding`) and `loadProvidersFromDB` / config-based Ollama paths (for chat models like `qwen3:4b`). Uses the official Ollama SDK `Pull` API with progress logging.

7. ✅ **`README.md`** — Add system requirements section:
   ```markdown
   ## System Requirements
   
   - macOS with Apple Silicon (M1 or later)
   - 16 GB RAM minimum
   - ~4 GB disk space (for Nebo + local models)
   ```

8. ✅ **Embedding dimension migration** — `MemoryTool.MigrateEmbeddings()` detects stale embeddings from old models (e.g., nomic-embed-text at 768 dims), deletes them along with orphaned chunks and stale cache entries, so `BackfillEmbeddings` regenerates everything with the current model. Runs automatically at agent startup before backfill.

### Configuration

Users can override bundled models in `config.yaml`:

```yaml
ollama:
  embedding_model: qwen3-embedding     # default
  embedding_dimensions: 256             # default, range: 32-1024
  chat_model: qwen3:4b                 # default
```

### Upgrade path

For users on 32GB+ machines who want better local quality:

```yaml
ollama:
  chat_model: qwen3:8b      # 4.9 GB, significantly smarter
  # or
  chat_model: qwen3:14b     # 9 GB, approaching cloud quality
```

## Consequences

### Positive

- Better memory search quality (embeddings) and tool-calling accuracy (chat)
- Smaller embedding vectors (256 vs 768) = less storage, faster similarity search
- Single ecosystem (Qwen3) for both models = simpler mental model
- Apache 2.0 across the board = clean licensing
- 16GB requirement simplifies development and testing
- Comfortable memory headroom (~8 GB free for user apps)

### Negative

- Users with 8GB Macs cannot run Nebo with local models (they can still use cloud-only mode if we add that path later)
- ~3.1 GB first-run download (vs ~2.3 GB with old models)
- Existing nomic-embed-text embeddings need re-computation on upgrade
- Qwen3 is from Alibaba — some users may have vendor preference concerns (mitigated by Apache 2.0 license and the model weights being fully local)

### Neutral

- Ollama remains the local model runtime — no change to the integration architecture
- Cloud providers (Anthropic, OpenAI, Google, DeepSeek) are unaffected
- The STRAP tool pattern is unaffected
