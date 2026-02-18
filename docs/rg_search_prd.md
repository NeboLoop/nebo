# Product Requirements Document (PRD): Built-in ripgrep-backed search (rg_search)

Executive summary
- Nebo will embed a lightweight ripgrep-backed search capability (rg_search) as a built-in core feature. It provides fast, structured search results (JSON) across local workspaces, with optional streaming and memory integration. It gracefully degrades to grep if rg is unavailable, preserving Nebo’s core philosophy of self-contained, reliable tooling.
- The feature enables Nebo to index and surface search results across code, docs, and data, improving discovery, triage, and automation workflows without requiring an external app.

Strategic fit
- Improves developer productivity by delivering fast, reproducible search results inside Nebo.
- Reduces external tool dependencies for common search tasks when rg is not installed.
- Provides a consistent data format (JSON) suitable for storage, recall, and analytics within Nebo.

Scope
- In-scope: core rg_search implementation, CLI surface, JSON result format, streaming/paging, memory integration, basic configuration hooks, tests, and documentation.
- Out-of-scope: a separate GUI app, full-text indexing, external search providers, or advanced natural-language search; integration with non-local data stores is deferred.

Definitions and abbreviations
- rg: ripgrep, a fast command-line search tool.
- grep: standard Unix search tool used as a fallback.
- Nebo: the local AI agent that runs with access to the user’s machine.
- JSON: JavaScript Object Notation, a lightweight, machine-parseable format.

Goals and success criteria
- FR1: Core functionality to search a directory with a query, supporting optional file patterns and max results.
- FR2: Use rg when available, fallback to grep if not installed.
- FR3: Output a machine-parseable result stream in JSON; include fields such as file, line, snippet, before, after, score, path, timestamp.
- FR4: Provide a CLI interface (nebosearch rg_search) with parameters: query, dir, max, pattern, case-sensitive, and output mode (streaming vs. batch).
- FR5: Surface top results in Nebo’s memory for quick recall and re-use; allow refresh/eviction.
- FR6: Respect privacy and ignore patterns; avoid leaking sensitive data; honor user configuration.
- NFR1: Performance targets: sub-second latency for typical monorepo searches; scalable to large repos; memory usage bounded.
- NFR2: Reliability: graceful fallback and helpful error messages if rg is missing or errors occur.
- NFR3: Security: sanitize inputs to avoid shell-injection or command injection; proper permission handling.
- NFR4: Usability: simple, consistent CLI; clear docs; test coverage.

User personas and stakeholders
- Primary user: Alma (the owner/developer who provided Nebo; uses Nebo to search code and docs locally).
- Secondary users: developers using Nebo-driven automation, data scientists exploring local corpora, and users relying on Nebo for quick discovery.
- Stakeholders: Nebo core team, security/compliance (privacy), product owner for Nebo.

Assumptions
- rg is installed on target systems or can be installed by the user; fallback path remains functional.
- Nebo has access to the local filesystem where searches occur.
- The output will be captured by Nebo memory and surfaced to the user in subsequent interactions.

Constraints
- Cross-platform (macOS, Linux). Windows support may require additional polyfills; current scope focuses on macOS/Linux environments.
- CLI footprint should be lightweight; avoid large binary dependencies.
- Licensing: respect open-source licenses of rg and any dependencies; ensure Nebo does not repackage or override licenses without permission.

Functional requirements (detailed)
- FR-1: Search parameters
  - query: string to search for (required)
  - dir: directory to search (required)
  - max_results: integer cap for results (optional, default 100 or 200)
  - pattern: glob pattern to constrain search (optional)
  - case_sensitive: boolean flag (optional)
- FR-2: Engine selection
  - If rg is available on PATH, use it: rg --json --line-number --no-heading --glob <pattern> <query> <dir>
  - If rg is not available, fall back to grep-like semantics: grep -R --with-filename -n <query> <dir> and optionally support a pattern.
- FR-3: Output format
  - Each match emitted as a JSON object with fields:
    - file: absolute path to the file
    - line: line number where match occurs
    - snippet: the matched line or a contextual snippet
    - before: preceding context (optional)
    - after: trailing context (optional)
    - score: equivalence score (rg provides a score; grep path can include a heuristic score)
    - path: normalized path (may be same as file)
    - timestamp: ISO8601 timestamp of match
- FR-4: Streaming and paging
  - Support streaming mode (one JSON object per line) for long searches.
  - Provide paging support via a remaining/offset parameter or a batch count.
- FR-5: CLI surface
  - nebosearch rg_search --query "..." --dir "/path" [--max 200] [--pattern "*.md"] [--case-sensitive] [--stream] [--format json]
- FR-6: Memory integration
  - Store top-N results for quick recall (tacit layer); include metadata: source, timestamp, license hints.
  - Provide commands to clear or refresh stored results.
- FR-7: Privacy and security
  - Respect existing ignore patterns (.gitignore, etc.) if possible; do not index sensitive data unless user opts in.
  - Sanitize inputs to avoid shell injection in wrapper implementation.
- FR-8: Observability
  - Emit logs and metrics for search invocations, latency, and error rates; surfaced to Nebo's monitoring surface.

Non-functional requirements (quality attributes)
- NF-1: Performance: target sub-second latency for typical queries in medium-sized repos; linear scaling with repo size; parallel search should be throttled to avoid resource exhaustion.
- NF-2: Reliability: robust fallback path; clear error messages and graceful degradation; backward-compatible invocation when not installed.
- NF-3: Usability: consistent CLI UX, clear docs, minimal configuration; test coverage; predictable JSON output.
- NF-4: Security: input sanitization; least privilege operations; avoid leaking PII; encryption at rest for any stored results if stored in memory.
- NF-5: Portability: works on macOS and Linux; consistent behavior across shells.

Data management
- Data produced by searches is stored in Nebo’s memory layers (tacit/daily/entity) for recall.
- Provide TTL or explicit eviction controls for memory-stored results.
- Do not persist search results to disk unless explicitly requested by a feature flag or user action.

Risks and mitigations
- Risk: rg not installed; mitigation: robust fallback to grep with clear messaging.
- Risk: Large search results overwhelm memory; mitigation: cap max_results and streaming/batching; implement paging.
- Risk: Privacy concerns with indexing; mitigation: opt-in for storing results; honor ignore patterns; provide explicit user controls.
- Risk: Compatibility gaps across OS versions; mitigation: maintain minimal dependency surface; test on supported platforms.

Dependencies
- ripgrep (rg) on PATH or accessible by Nebo; fallback to grep.
- Optional: Python/Node wrapper for JSON formatting; not required for core but helpful if Nebo uses internal scripting.

Milestones and timelines (high-level)
- M0: PRD approval and design freeze.
- M1: Implement core rg_search wrapper (rg path detection, command construction) – 2–3 weeks.
- M2: JSON result formatter, streaming mode, and CLI surface – 2 weeks.
- M3: Memory integration and recall APIs – 1–2 weeks.
- M4: Tests, security review, usability testing – 2 weeks.
- M5: Documentation, beta release, feedback collection – 2 weeks.
- M6: General availability – after beta successful review.

Acceptance criteria
- AC1: nebosearch rg_search executes with a query and dir and returns streaming JSON results via stdout or a pipe.
- AC2: Fallback path works with grep and returns consistent fields in JSON.
- AC3: Top-N results can be stored in memory for recall.
- AC4: Error cases are handled gracefully with actionable messages.
- AC5: Documentation is updated with usage examples and troubleshooting.

Appendix: Data formats and interfaces
- Result object schema (JSON):
  {
    "file": "/abs/path/to/file.ext",
    "line": 123,
    "snippet": "matched line or snippet",
    "before": "context before match",
    "after": "context after match",
    "score": 42,
    "path": "/abs/path/to/file.ext",
    "timestamp": "2026-02-17T16:00:00Z"
  }
- Memory surface model: top-N results stored with fields: source (path), timestamp, license_hint (if detected), etc.

Notes and caveats
- This PRD emphasizes a non-breaking, opt-in approach for additional search capability, but the plan is to integrate tightly as a built-in feature in Nebo’s core. If the team later decides to extract rg_search into a separate app, API compatibility can be preserved by maintaining the same CLI surface and JSON formats.
- Keep the user’s privacy and data sensitivity front and center; no new persistent storage beyond what Nebo already uses unless explicitly configured.
