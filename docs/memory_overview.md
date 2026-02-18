Memory System Snapshot

Architecture overview
- WRITE PATHS (non-blocking): Extract memories from chat, pre-compaction flush, session indexing, direct user requests, and style reinforcement.
- READ PATHS: System prompt injection, explicit memory search (HybridSearch: 70% vector cosine, 30% FTS5 BM25, threshold 0.3), exact recall.
- PERSONALITY LAYER: Style observations accumulate and decay over 14 days; synthesize directive for prompts.
- STORAGE LAYERS: tacit (permanent, decays), daily (session-scoped), entity (people/projects).

Data model
- memories (facts), memory_chunks (1600-char segments), memory_embeddings (vectors), memory_fts (FTS5)

What’s Working Well
- Non-blocking extraction paths
- Hybrid search recall
- Emergent personality
- 50-memory cap with dedup

Observations and risks
- Extraction may run twice for some conversations
- Context loading repeats DB queries per iteration
- Embeddings migration missing on model changes
- 50-memory cap may be suboptimal

Recommendations
- Add per-session dedup guards, result caching for context loading, embedding versioning/migration hooks, adaptive memory cap, improved observability, and privacy controls.
