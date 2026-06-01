# PRD: Prompt Optimization Engine

## The Problem

NeboAI's prompt test harness measures First-Call Success Rate (FCSR) and context pollution across tool calls. It works. But the optimization loop is human-in-the-loop: a person reads traces, diagnoses failures, writes a one-sentence fix, runs the suite, reads the results. The harness automates measurement. The human does all the thinking.

This doesn't scale. There are 26 STRAP docs, 100+ resource/action pairs, 17+ fixtures (growing), and the interactions between prompt components mean a change to one doc can regress another. A human can't hold all of that in their head. And the search space of possible prompt phrasings is effectively infinite — a human will try 5 variations and pick the best one. A machine can try 500.

## The Thesis

Prompt engineering is an optimization problem with a measurable loss function. We have the loss function (FCSR + context pollution + token cost). We have the measurement infrastructure (the harness). What's missing is the optimization loop: automated mutation, automated evaluation, automated selection.

This is not fine-tuning the model. The model is fixed. We're optimizing the text the model reads — the STRAP docs, behavioral rules, tool descriptions, and system prompt components. The model is the function. The prompt is the input. FCSR is the output. We're doing gradient descent on the input.

## Loss Function

```
L = (1 - FCSR) + α(context_pollution) + β(token_cost_normalized)

Where:
  FCSR                    = successful first calls / total tool calls (0-1)
  context_pollution       = retry_tokens / total_tokens (0-1)
  token_cost_normalized   = total_tokens / budget_ceiling (0-1)
  α                       = 0.1 (pollution weight)
  β                       = 0.001 (cost weight — tiebreaker, not driver)
```

Lower is better. A perfect run: `L = 0 + 0 + ~0 = 0`. The degenerate loop screenshot: `L = 0.9 + 0.6 + 0.5 = ~1.1`.

α and β are tunable. FCSR dominates by design — a prompt that reduces token cost but drops FCSR is rejected. Context pollution is the diagnostic signal that explains *why* FCSR dropped.

### Per-Fixture Loss vs Suite Loss

Every fixture produces its own loss. The suite loss is the mean across all fixtures, but with a critical constraint: no individual fixture can regress beyond the noise threshold. This prevents the optimizer from sacrificing one capability to improve another.

```
Suite Loss = mean(L_i for all fixtures i)
Constraint: for all i, L_i(candidate) ≤ L_i(baseline) + noise_threshold
```

If the constraint is violated for any fixture, the candidate is rejected regardless of suite loss improvement.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                  Optimization Engine                 │
│                                                     │
│  ┌───────────┐  ┌───────────┐  ┌────────────────┐  │
│  │  Mutator  │→ │  Runner   │→ │   Evaluator    │  │
│  │           │  │           │  │                │  │
│  │ Generates │  │ Runs N=10 │  │ Computes loss  │  │
│  │ prompt    │  │ per fix-  │  │ per fixture,   │  │
│  │ variants  │  │ ture via  │  │ runs t-test,   │  │
│  │           │  │ harness   │  │ checks regress │  │
│  └───────────┘  └───────────┘  └───────┬────────┘  │
│       ↑                                │            │
│       │         ┌───────────┐          │            │
│       └─────────│  Selector │←─────────┘            │
│                 │           │                       │
│                 │ SHIP /    │                       │
│                 │ REVERT /  │                       │
│                 │ INCONCLUS │                       │
│                 └─────┬─────┘                       │
│                       │                             │
│                 ┌─────▼─────┐                       │
│                 │  History  │                       │
│                 │  Store    │                       │
│                 └───────────┘                       │
└─────────────────────────────────────────────────────┘
```

### Component 1: Mutator

The mutator takes a STRAP doc section and generates N variations. It uses a smart model (Claude Sonnet or Opus) with a structured prompt:

```
You are optimizing a prompt component for an AI agent platform.

CURRENT PROMPT SECTION:
{section_text}

FIXTURE FAILURES THIS SECTION IS INVOLVED IN:
{failed_fixture_ids, failure descriptions, trace excerpts}

LOSS HISTORY (last 10 experiments on this section):
{experiment_log}

Generate {N} variations of this section. Each variation should:
1. Address the specific failure mode described above
2. Change as little as possible (minimize blast radius)
3. Be testable — the change should produce a measurable FCSR delta

Return JSON:
[
  {
    "id": "v1",
    "hypothesis": "Adding explicit 'do not read before editing' should prevent the read-first pattern",
    "diff": "unified diff of the change",
    "full_text": "the complete replacement section text",
    "target_fixture": "os-file-edit",
    "expected_impact": "FCSR +50% on os-file-edit, no regression on os-file-read"
  },
  ...
]
```

**Mutation strategies** (the mutator cycles through these):

1. **Directive insertion** — add one imperative sentence addressing the failure
2. **Reordering** — move a behavioral rule higher/lower in the doc
3. **Simplification** — remove words/sentences to reduce cognitive load
4. **Example addition** — add a concrete example of correct behavior
5. **Negative example** — add "do NOT do X" for the observed failure pattern
6. **Consolidation** — merge two related rules into one clearer rule
7. **Decomposition** — split a complex rule into two simpler ones
8. **Vocabulary alignment** — change terms to match the model's training distribution (informed by Claude Code naming conventions)

The mutator doesn't generate random text. It generates *hypotheses* — each variation has a stated reason for why it should improve FCSR. The hypothesis is stored with the experiment for later analysis.

### Component 2: Runner

The runner is the existing harness (`nebo test run`), called programmatically:

```
For each mutation:
  1. Inject the mutated section via --override
  2. Run the full fixture suite with --runs 10
  3. Capture all traces to the experiment directory
  4. Pass traces to the evaluator
```

**Parallelism:** Mutations are independent. Run M mutations × N runs concurrently, bounded by Janus rate limits. At $0.029/M tokens and ~25K tokens per request, 10 mutations × 10 runs × 17 fixtures = 1,700 requests = ~$1.23 per optimization round. Cheap enough to run frequently.

**Budget ceiling:** Each optimization round has a hard token budget. If a mutation produces runs that exceed the budget, those runs are terminated early and scored as failures. This prevents runaway costs from degenerate mutations.

### Component 3: Evaluator

The evaluator computes the loss function and statistical significance for each mutation:

```rust
struct EvalResult {
    mutation_id: String,
    suite_loss: f64,
    per_fixture_loss: HashMap<String, f64>,
    fcsr_mean: f64,
    fcsr_std: f64,
    pollution_mean: f64,
    token_mean: f64,
    
    // Statistical comparison against baseline
    fcsr_delta: f64,          // positive = improvement
    p_value: f64,             // from Welch's t-test
    significant: bool,        // p < 0.05
    
    // Regression check
    regressions: Vec<String>, // fixture IDs that degraded beyond noise
    verdict: Verdict,         // SHIP / REVERT / INCONCLUSIVE
}

enum Verdict {
    Ship,         // significant improvement AND no regressions
    Revert,       // any regression beyond threshold
    Inconclusive, // improvement exists but p > 0.05 — need more runs
}
```

**Welch's t-test** (not Student's) because we can't assume equal variance between baseline and candidate runs. The null hypothesis is "the candidate has the same FCSR as baseline." We reject at p < 0.05.

**Noise threshold for regression:** A fixture is considered regressed if `L_fixture(candidate) > L_fixture(baseline) + 2σ_baseline`. This allows for normal LLM variance while catching real degradations.

**Inconclusive handling:** If the improvement looks real but p > 0.05, the system can automatically request more runs (up to N=20) before making a final verdict. If it's still inconclusive at N=20, it reports the data and lets a human decide.

### Component 4: Selector

The selector is the decision gate:

```
IF verdict == SHIP:
    - Update the blessed baseline with the candidate's traces
    - Log the experiment to history
    - Apply the mutation to the STRAP doc source file
    - Rebuild the server (cargo build)
    - Run a confirmation suite to verify the compiled change matches the override results
    
IF verdict == REVERT:
    - Log the experiment with failure reason
    - Feed the failure back to the mutator as a negative example
    
IF verdict == INCONCLUSIVE:
    - Request more runs if under N=20
    - Otherwise log and move to next mutation
```

**Auto-apply vs human approval:** Configurable. In auto mode, SHIP verdicts are applied immediately and the optimizer moves to the next target. In approval mode, SHIP verdicts are queued for human review. Start with approval mode until trust is established.

### Component 5: History Store

Every experiment is an append-only record:

```json
{
  "experiment_id": "exp-2026-05-31-001",
  "timestamp": "2026-05-31T21:00:00Z",
  "target_component": "os_macos.txt",
  "target_section": "behavioral_rules",
  "mutation_strategy": "directive_insertion",
  "hypothesis": "Adding 'do not read before editing' prevents read-first pattern",
  "diff": "...",
  "baseline_fcsr": 0.82,
  "candidate_fcsr": 0.94,
  "delta": 0.12,
  "p_value": 0.003,
  "verdict": "SHIP",
  "regressions": [],
  "per_fixture_results": { ... },
  "traces_path": ".nebo/test-results/experiments/exp-2026-05-31-001/"
}
```

**History as training data:** After 100+ experiments, the history contains signal about which mutation strategies work for which types of failures. A meta-learner can analyze this:

- "Directive insertion works 80% of the time for tool-routing failures"
- "Reordering works 60% of the time for retry-spiral failures"
- "Simplification rarely works for hallucination failures"
- "Vocabulary alignment has the highest effect size across all failure types"

This feedback loop means the mutator gets smarter over time. Early experiments are random search. Later experiments are guided by what's worked before.

## CLI Interface

```bash
# Run one optimization round on a specific component
nebo optimize \
  --target tool.os \
  --suite suites/smoke.yaml \
  --mutations 10 \
  --runs-per-mutation 10 \
  --grader claude-sonnet-4-6 \
  --auto-apply false

# Run continuous optimization until convergence
nebo optimize \
  --target all \
  --suite suites/smoke.yaml \
  --max-rounds 50 \
  --convergence-threshold 0.01 \
  --auto-apply true

# Review pending approvals
nebo optimize --review

# Show optimization history
nebo optimize --history
nebo optimize --history --component os_macos.txt

# Analyze which mutation strategies work
nebo optimize --meta-analysis
```

### Target Selection

`--target` specifies what to optimize:

- `tool.os` — the OS STRAP doc (all sections)
- `tool.os.behavioral_rules` — one section within a STRAP doc
- `tool.web` — the web STRAP doc
- `all` — cycle through all components with RED or YELLOW fixtures
- `auto` — the system picks the component with the worst FCSR first

When `--target auto`, the system:
1. Runs the baseline suite
2. Sorts fixtures by loss (worst first)
3. Maps each failing fixture to its target STRAP doc
4. Optimizes the worst STRAP doc first
5. Moves to the next worst
6. Stops when all fixtures are GREEN or max rounds reached

## Convergence

The optimizer stops when:

1. **All fixtures are GREEN** (FCSR ≥ 0.9) — the prompt is good enough
2. **Max rounds reached** — safety limit
3. **No improvement for N rounds** — the search has exhausted easy wins. Remaining failures may need code changes (like merging agent/agents), not prompt changes
4. **Budget exhausted** — hard cost ceiling per session

Convergence threshold: if suite loss delta < 0.01 for 5 consecutive rounds, stop. The remaining improvement is in the noise.

## Safety

**Blast radius control.** Each mutation changes ONE section of ONE STRAP doc. Never multiple sections. Never multiple docs. The single-variable principle from the harness design carries forward.

**Rollback.** Every SHIP stores the previous version. If a post-apply confirmation suite shows regression, auto-rollback and mark the experiment as a false positive.

**Human circuit breaker.** Even in auto-apply mode, the system stops and alerts if:
- Suite loss increases by >20% in a single round
- More than 3 consecutive REVERT verdicts (the mutator is stuck)
- Token cost per round exceeds 2x the budget ceiling
- Any fixture drops from GREEN to RED

**Prompt injection resistance.** The mutator generates prompt text that will be injected into the system prompt. The evaluator checks that generated text doesn't contain injection patterns (role-play instructions, "ignore previous instructions", encoded payloads). Any mutation containing suspicious patterns is rejected before evaluation.

## Data Model

```
.nebo/optimization/
├── config.yaml                 # α, β, N, thresholds, budget
├── baseline/                   # Current blessed baseline traces
│   ├── metadata.json           # Git commit, STRAP doc hashes, suite FCSR
│   └── traces/                 # Per-fixture trace files
├── experiments/
│   ├── exp-2026-05-31-001/
│   │   ├── hypothesis.json     # Mutation details
│   │   ├── diff.patch          # Exact STRAP doc change
│   │   ├── traces/             # All run traces
│   │   ├── eval.json           # Loss, p-value, verdict
│   │   └── meta.json           # Timing, cost, runs
│   └── ...
├── history.jsonl               # Append-only experiment log
├── pending_approvals.json      # SHIP verdicts awaiting human review
└── meta_analysis/
    ├── strategy_effectiveness.json  # Which strategies work for what
    └── component_difficulty.json    # Which STRAP docs resist optimization
```

## Implementation Phases

### Phase 1: Automated Evaluation Pipeline
- Evaluator component: loss function, t-test, regression detection, verdict
- History store: append-only JSONL, baseline management
- CLI: `nebo optimize --target --suite` runs one round manually
- Human reviews every result
- **Requires:** existing harness, scipy/statrs for t-test

### Phase 2: Mutator
- LLM-based mutation generation via Janus
- 8 mutation strategies
- Hypothesis-driven generation (not random)
- Negative example feedback from REVERT history
- **Requires:** Phase 1, grader model access

### Phase 3: Selector + Auto-Apply
- SHIP/REVERT/INCONCLUSIVE decision gate
- Auto-apply mode with rollback
- Confirmation suite after apply
- Human circuit breaker alerts
- **Requires:** Phase 2, cargo build integration

### Phase 4: Continuous Optimization
- `--target auto` picks worst component
- Convergence detection
- Budget management
- `nebo optimize --review` for pending approvals
- **Requires:** Phase 3

### Phase 5: Meta-Learning
- Strategy effectiveness analysis across history
- Component difficulty ranking
- Guided mutation (use strategies that historically work)
- Predictive: "this type of change is likely to improve this fixture by X%"
- **Requires:** 100+ experiments in history

## Cost Estimate

Per optimization round:
- 10 mutations × 10 runs × 17 fixtures × 25K tokens = 42.5M tokens
- At $0.029/M (Qwen 3.5 Flash): **$1.23 per round**
- Grader calls (Claude Sonnet): 170 calls × ~5K tokens = 850K tokens × $18/M = **$15.30 per round**
- Mutator call (Claude Sonnet): 1 call × ~10K tokens = **$0.18**
- **Total: ~$16.71 per round**

At 10 rounds per session: **~$167 per optimization session**

At convergence (50 rounds max): **~$835 worst case**

This is a one-time cost to optimize the prompt stack. After convergence, ongoing cost is regression suite runs at ~$1.23 per deployment.

## Success Criteria

1. Suite FCSR reaches 95%+ across all fixtures (from current ~75%)
2. No fixture below 80% FCSR
3. Context pollution below 0.1 average across suite
4. The system finds prompt improvements a human didn't think of
5. Optimization converges within 30 rounds
6. Zero regressions reach production (rollback catches them)
7. History produces actionable meta-analysis after 100 experiments
