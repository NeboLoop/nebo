Yeah, I think you can let 8GB go. Here's the case in a nutshell:

Drop 8GB support. Require 16GB.

Apple already made this decision for you. Every Mac sold since late 2023 ships with 16GB minimum. 8GB is the M1/M2 Air base — a shrinking installed base.

The experience would suck anyway. 8GB shared memory, minus ~4GB for macOS, leaves you cramped. Even if the model fits, the user's machine feels sluggish. "Nebo makes my Mac slow" is worse than "Nebo requires 16GB."

Better models, happier users. Targeting 16GB means you can default to qwen3:4b (2.5GB, noticeably smarter) instead of squeezing into 1.7B. Tool-calling accuracy, reasoning quality, instruction following — all meaningfully better at 4B. That's the difference between "this is cool" and "this is meh."

Solo dev tax. Supporting two model tiers = two test matrices, two sets of edge cases, "works on my machine" debugging. Not worth it pre-launch.

Revised bundle for 16GB+ target:

Role Model Size
🧠 Embeddings qwen3-embedding:0.6b 639MB
💬 Local Chat qwen3:4b ~2.5GB
Total ~3.1GB
Still very reasonable. Leaves ~12GB for macOS + apps + Nebo's Go binary. Comfortable.

Add one line to the README: "Requires macOS with Apple Silicon and 16GB+ RAM" — and move on to building features. You can always revisit if real users ask for it post-launch.

Assistant
4:12 PM
U
lets do it. write this one up
You
4:15 PM
A
Let me write up the local model strategy document. Let me first check where project docs live.

file
docs/**/*.md
Completed

file
**/model
Completed

file
docs*
Completed

file
internal/defaults/dotnebo/models.yaml
Completed
Now let me check the embeddings code to understand the current implementation:

file
internal/agent/embeddings/**/*.go
Completed

file
internal/agent/embeddings/
Completed
Let me check the current config and any existing docs structure:

file
internal/agent/embeddings/providers.go
Completed

file
**/README*
Completed

file
.
Completed
Let me check the README for current system requirements and see what Ollama models config looks like:

file
README.md
Completed
Now let me check what config references exist for Ollama model setup:

file
internal/agent/
Completed

file
.
Completed
Good, I can see the current defaults hardcoded in the codebase. Let me check the agent.go embeddings setup:

file
cmd/nebo/agent.go
Completed

file
internal/agent/ai/api_ollama.go
Completed
Now I have all the context I need. Let me write the decision doc and create a docs/ directory for it:

file
docs/decisions/001-local-model-strategy.md
Completed
Done. Written to docs/decisions/001-local-model-strategy.md.

What's in it:

System requirement: 16GB RAM — rationale for dropping 8GB
Bundled models: qwen3-embedding:0.6b + qwen3:4b — ~3.1 GB total, with memory budget breakdown showing ~8 GB headroom on 16GB machines
Comparison tables — why these beat nomic/llama3.2/cogito/granite/embeddinggemma
8 implementation items — exact files to change, config format, migration plan for existing embeddings
Upgrade path — qwen3:8b and 14b for users with more RAM
Want me to start on the implementation (updating the defaults in the codebase)?

Assistant
4:15 PM
U
lets do it
You
4:25 PM
A
Let me first review all the files that need to change to map out the implementation:

file
docs/decisions/001-local-model-strategy.md
Completed

file
internal/agent/embeddings/ollama.go
Completed

file
internal/agent/embeddings/openai.go
Completed
Let me find the actual embedding files:

file
internal/agent/embeddings/**/*.go
Completed

file
internal/agent/ai/api_ollama.go
Completed

file
internal/defaults/dotnebo/models.yaml
Completed

file
internal/agent/embeddings/providers.go
Completed

file
internal/agent/ai/api_ollama.go
Completed

file
internal/defaults/dotnebo/models.yaml
Completed

file
internal/agent/embeddings/service.go
Completed

file
internal/agent/embeddings/hybrid.go
Completed
Now let me get the remaining files — the agent.go setup code and the test handler:

file
cmd/nebo/agent.go
Completed

file
internal/
Completed

file
internal/handler/provider/testauthprofilehandler.go
Completed
Now let me also check the README for the system requirements section:

file
README.md
Completed

file
internal/agent/embeddings/
Completed
Good — I have the full picture. Now I need to check one more thing: the Ollama embedding API supports a truncate or dimensions parameter for qwen3-embedding. Let me also check if there's a way to pass dimensions to the Ollama embed endpoint:

file
internal/agent/embeddings/providers.go
Completed

file
README.md
Completed
Got everything I need. Now I need to understand one important detail — the Ollama /api/embeddings endpoint doesn't support a dimensions parameter natively. For qwen3-embedding with flexible dimensions, we need to either:

Use the newer /api/embed endpoint which supports truncate
Or just let the model return its native dimensions and truncate client-side
Let me check the Ollama API quickly:

web
ollama api embed endpoint truncate_dim dimensions parameter qwen3-embedding
Completed

web
https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md
Completed
The newer /api/embed endpoint supports a dimensions parameter directly. Our code is using the deprecated /api/embeddings endpoint. I need to:

Switch to /api/embed (supports batch + dimensions param)
Update defaults to qwen3-embedding + 256 dims
Update the chat model default to qwen3:4b
Update models.yaml
Update log messages
Update test handler fallback
Update README
Let me do all the independent file edits now:

file
internal/agent/embeddings/providers.go
Completed
Now switch the Ollama provider from the deprecated /api/embeddings (one-at-a-time) to the new /api/embed (batch + dimensions):

file
internal/agent/embeddings/providers.go
Completed
Now update the chat model default and the remaining files:

file
internal/agent/ai/api_ollama.go
Completed

file
cmd/nebo/agent.go
Completed

file
internal/handler/provider/testauthprofilehandler.go
Completed
Now update models.yaml — replace the old llama3.3 entry with qwen3:4b as the primary Ollama model:

file
internal/defaults/dotnebo/models.yaml
Completed
Now update the README with system requirements:

file
README.md
Completed
Now let me verify the providers.go looks clean after both edits:

file
internal/agent/embeddings/providers.go
Completed
Clean. Now let's build to verify everything compiles:

shell
cd /Users/almatuck/workspaces/nebo/nebo && make build 2>&1
Completed
Clean build. Now let me do a quick grep to make sure there are no remaining references to the old defaults anywhere:

file
~/workspaces/nebo/nebo
Completed

file
~/workspaces/nebo/nebo
Completed

file
~/workspaces/nebo/nebo
Completed
Only references remaining are in the ADR doc itself (which is correct — it's documenting the old values for historical context). Zero stale references in code or config.

Here's the full summary:

Done. 7 files changed, clean build. ✅

File Change
internal/agent/embeddings/providers.go nomic-embed-text → qwen3-embedding, 768 → 256 dims, switched from deprecated /api/embeddings to /api/embed (batch + dimensions support)
internal/agent/ai/api_ollama.go llama3.2 → qwen3:4b default
cmd/nebo/agent.go Log message updated
internal/handler/provider/testauthprofilehandler.go Test fallback llama3.2 → qwen3:4b
internal/defaults/dotnebo/models.yaml Replaced llama3.3 with qwen3:4b as primary, added qwen3:8b upgrade option
README.md Added System Requirements section
docs/decisions/001-local-model-strategy.md Already done (previous step)
Bonus improvement: The Ollama embedding provider now uses /api/embed instead of the deprecated /api/embeddings. This means:

Batch embedding — sends all texts in one request instead of one-at-a-time
Native dimensions param — Ollama truncates server-side for qwen3-embedding, no client-side work needed
Significant speedup for memory operations that embed multiple chunks
Still on the list from the ADR (not done yet):

Auto-pull models on first run (item 6)
Embedding cache migration — clear old 768-dim nomic embeddings when model changes (item 8)
Want me to tackle either of those next?

Assistant
4:25 PM
U
yes,
You
4:39 PM
A
Let me check what we have for both of those items before diving in.

file
~/workspaces/nebo/nebo
Completed

file
~/workspaces/nebo/nebo
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings/providers.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings
Completed

file
/Users/almatuck/workspaces/nebo/nebo/internal/agent/embeddings/*.go
Completed

file
/Users/almatuck/workspaces/nebo/nebo/internal/db/queries/embed*
Completed
Let me get the full picture of the embedding storage, the agent startup flow, and the Ollama interaction points.

file
/Users/almatuck/workspaces/nebo/nebo/internal/agent/embeddings/**/*.go
Completed

file
~/workspaces/nebo/nebo/internal
Completed

file
~/workspaces/nebo/nebo/internal/db
Completed
Good — the DB already has DeleteMemoryEmbeddingsByModel, CountMemoryEmbeddings, and ListMemoryEmbeddingsByModel. Now let me look at the agent startup flow and the Ollama API for pulling models.

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings/hybrid.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/tools/memory.go
Completed
Now let me look at the embedding service to understand the full picture, and check the Ollama API for model pulling:

file
/Users/almatuck/workspaces/nebo/nebo/internal/agent/embeddings/*.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings
Completed
Now let me look at the Ollama provider setup flow and understand how Ollama is detected, plus look at the existing api_ollama.go for model interaction:

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings/service.go
Completed
Now I have the full picture. Let me also check how createProviders works to see where the Ollama startup happens, and look at the sqlc generated code for the embedding queries we'll need:

file
~/workspaces/nebo/nebo/cmd/nebo
Completed

file
/Users/almatuck/workspaces/nebo/nebo/cmd/nebo/*.go
Completed

file
~/workspaces/nebo/nebo/cmd/nebo/providers.go
Completed

file
~/workspaces/nebo/nebo/internal/db/embeddings.sql.go
Completed
Now I have the complete picture. Let me implement both features. Here's the plan:

Auto-pull Ollama models — Add a EnsureModel function to api_ollama.go that pulls models if not present, call it from createEmbeddingService and loadProvidersFromDB
Embedding migration — Add a MigrateEmbeddings function to the MemoryTool that detects stale embeddings (wrong model/dimensions) and clears them so backfill regenerates them
Let me implement both:

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed
Good, I have everything I need. Let me implement both features now.

Feature 1: Auto-pull Ollama models — add EnsureOllamaModel to api_ollama.go
Feature 2: Embedding migration — add MigrateEmbeddings to memory.go, call from agent startup

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed
Now I need to add the strings import since we use strings.HasPrefix and strings.TrimSuffix:

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed
Now add the embedding migration to memory.go:

file
~/workspaces/nebo/nebo/internal/agent/tools/memory.go
Completed
Now wire both features into the agent startup. First, add auto-pull to createEmbeddingService and the Ollama provider loading. Then add migration before backfill:

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed
Now add auto-pull for chat models in loadProvidersFromDB (the Ollama section):

file
~/workspaces/nebo/nebo/cmd/nebo/providers.go
Completed
Now wire up the embedding migration in agent.go — it needs to run BEFORE backfill. There are two places where backfill runs (the runAgentLoopWithOptions path and the standalone runAgent path). Let me update both:

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed
Now update the second backfill location (standalone runAgent path):

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed
Now let's build to check for compilation errors:

shell
cd /Users/almatuck/workspaces/nebo/nebo && go build ./... 2>&1
Completed
Clean build (only linker warnings about macOS version, no actual errors). Let me also verify the ai import is available in agent.go where we call ai.EnsureOllamaModel:

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed
Clean build. Let me also handle the case in the config-based Ollama provider path (in providers.go line 70) — where someone has Ollama in their config file rather than DB:

file
~/workspaces/nebo/nebo/cmd/nebo/providers.go
Completed

shell
cd /Users/almatuck/workspaces/nebo/nebo && go build ./... 2>&1 | grep -v "ld: wa...
Completed
Clean. Now let me update the ADR to mark these items as done:

file
~/workspaces/nebo/nebo/docs/decisions/001-local-model-strategy.md
Completed

file
~/workspaces/nebo/nebo/docs/decisions/001-local-model-strategy.md
Completed
Final build to confirm everything is still clean:

shell
cd /Users/almatuck/workspaces/nebo/nebo && go build ./... 2>&1 | grep -v "ld: wa...
Completed
Clean. Let me also do a quick sanity check on the final state of the changed files to make sure nothing got mangled:

shell
cd /Users/almatuck/workspaces/nebo/nebo && git diff --stat
Completed
Done. Here's the summary of both features:

✅ Feature 1: Auto-pull Ollama models (ADR item 6)

File Change
internal/agent/ai/api_ollama.go Added EnsureOllamaModel(baseURL, model) — checks if model exists locally via ListOllamaModels, pulls via official SDK client.Pull() if missing, with progress logging
cmd/nebo/agent.go Calls ai.EnsureOllamaModel for qwen3-embedding before creating the Ollama embedding provider
cmd/nebo/providers.go Calls ai.EnsureOllamaModel for chat models in both DB-profile and config-file Ollama paths
How it works: On agent startup, when Ollama is detected as available, Nebo checks if the required models are already pulled. If not, it pulls them automatically with progress output ([Ollama] Pulling qwen3:4b: 45%). The pull uses a 30-minute timeout to handle large models on slow connections.

✅ Feature 2: Embedding cache migration (ADR item 8)

File Change
internal/agent/tools/memory.go Added MigrateEmbeddings(ctx) — detects stale embeddings from old models, deletes them + orphaned chunks + stale cache, so BackfillEmbeddings regenerates everything
cmd/nebo/agent.go Calls MigrateEmbeddings before BackfillEmbeddings in both agent startup paths (shared and standalone)
How it works: On startup, MigrateEmbeddings queries for any memory_embeddings rows where model != currentModel. If found, it:

Logs which stale models/dimensions were found
Deletes the stale embedding rows
Deletes orphaned chunks (so backfill sees those memories as needing embeddings)
Clears the embedding_cache for old models
BackfillEmbeddings then runs immediately after and regenerates everything with the current model
A user upgrading from nomic-embed-text (768 dims) to qwen3-embedding (256 dims) will see:

[Memory] Stale embeddings: 42 vectors from model "nomic-embed-text" (768 dims) → will re-embed with "qwen3-embedding"
[Memory] Cleaned 42 orphaned chunks
[Memory] Migration complete: deleted 42 stale embeddings. BackfillEmbeddings will regenerate.
[agent] Backfilled embeddings for 42 memories
ADR status: All 8 items complete. 🎉
You
5:11 PM
A
Hey Alma — picking up right where we left off.

Quick recap of where we landed: the local model strategy is fully implemented — all 8 items from the ADR are done, clean build confirmed. The two big features from last session:

Auto-pull Ollama models — Nebo pulls qwen3-embedding:0.6b and qwen3:4b automatically on first run if Ollama is available
Embedding migration — Detects stale nomic/768-dim embeddings, clears them, backfill regenerates with qwen3-embedding/256-dim
What do you want to tackle next? 🔨

Assistant
5:12 PM
U
let’s see where we are in the code. this must all be 100% golang remember that
You
5:12 PM
A
Let me audit the codebase for the changes we made and make sure everything is idiomatic Go with no issues.

file
~/workspaces/nebo/nebo/internal/agent/ai/api_ollama.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/embeddings/providers.go
Completed

file
~/workspaces/nebo/nebo/internal/agent/tools/memory.go
Completed

file
~/workspaces/nebo/nebo/cmd/nebo/agent.go
Completed

file
~/workspaces/nebo/nebo/cmd/nebo/providers.go
Completed
Now let me also check the models.yaml, the README, and the ADR doc:

file
~/workspaces/nebo/nebo/internal/defaults/dotnebo/models.yaml
Completed

file
~/workspaces/nebo/nebo/docs/decisions/001-local-model-strategy.md
Completed

file
~/workspaces/nebo/nebo/internal/handler/provider/testauthprofilehandler.go
Completed
Now let me verify the build is clean and check for any lingering issues:

shell
cd /Users/almatuck/workspaces/nebo/nebo && go build ./... 2>&1 | grep -v "ld: wa...
Completed

shell
cd /Users/almatuck/workspaces/nebo/nebo && git diff --stat
Completed
Build is clean. That's a big diff (230 files) — most of it is from other work, not the model strategy. Let me verify the specific files we touched are solid:

shell
cd /Users/almatuck/workspaces/nebo/nebo && go vet ./internal/agent/ai/ ./interna...
Completed

shell
cd /Users/almatuck/workspaces/nebo/nebo && grep -rn "nomic-embed-text\|llama3\.2...
Completed
Good news:

No stale references — The only mentions of nomic-embed-text and llama3.2/llama3.3 are in code comments explaining the migration (correct — that's historical context, not active code).

go vet clean on our files — The only vet warning is in desktop.go:265 (a pre-existing WaitGroup.Add inside a goroutine issue — not our code, not related to the model strategy).

Build clean — compiles fine.

Here's the audit summary:

All Go, all idiomatic. No issues found. ✅

File	Status	Notes
internal/agent/ai/api_ollama.go	✅	EnsureOllamaModel + ListOllamaModels — uses official Ollama SDK, proper error handling, progress callback
internal/agent/embeddings/providers.go	✅	Switched to /api/embed (batch + dimensions), defaults qwen3-embedding / 256 dims
internal/agent/tools/memory.go	✅	MigrateEmbeddings uses raw SQL (correct — dynamic queries), BackfillEmbeddings batches 20 at a time, aborts on auth errors
cmd/nebo/agent.go	✅	Migration → backfill wired in both startup paths (shared DB + standalone), runs in background goroutine
cmd/nebo/providers.go	✅	EnsureOllamaModel called for both DB-profile and config-file Ollama paths
internal/defaults/dotnebo/models.yaml	✅	qwen3:4b primary, qwen3:8b upgrade option
internal/handler/provider/testauthprofilehandler.go	✅	Fallback qwen3:4b
docs/decisions/001-local-model-strategy.md	✅	All 8 items marked done
README.md	✅	System requirements: 16GB, Apple Silicon
One pre-existing issue worth noting: cmd/nebo/desktop.go:265 has a WaitGroup.Add called inside a goroutine — that's a race condition. Not ours, but want me to fix it while we're here?
