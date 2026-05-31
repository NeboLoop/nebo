# Nebo Prompt Test Harness v2

## North Star Metric: First-Call Success Rate

The single number that predicts agent task completion:

```
First-Call Success Rate = tool calls that achieved their intent on the first attempt
                          ─────────────────────────────────────────────────────────
                          total tool calls
```

This metric is a function of two things that must be tested together:

1. **Tool reliability** — did the tool execute correctly and return a useful response?
2. **Tool prompt clarity** — did the model call the tool correctly with the right arguments?

When both are working, context stays clean and even a cheap model completes complex tasks. When either fails, context pollution begins and the task spirals regardless of model intelligence.

## The Context Pollution Model

A complex agent task is a chain of tool calls. Each call either succeeds (clean context) or fails (polluted context). The probability of task completion degrades exponentially with each failure:

```
Clean:    User → Tool₁ ✓ → Tool₂ ✓ → Tool₃ ✓ → Done
          Context: [task] [result₁] [result₂] [result₃] [answer]
          Tokens: ~4K

Polluted: User → Tool₁ ✗ → Retry₁ ✗ → Retry₂ ✗ → Tool₁ ✓ → Tool₂ ✓ → Done
          Context: [task] [fail₁] [error₁] [fail₂] [error₂] [fail₃] [error₃]
                   [result₁] [result₂] [answer]
          Tokens: ~25K
          Model is now reasoning through 3 failures before doing actual work
```

Every failed tool call injects noise into the context that the model must carry forward. The model doesn't forget the failures — they sit in the context window, competing for attention with the actual task. This is why a 7-step task with 100% first-call success completes cleanly at 5K tokens, but the same task with 50% first-call success bloats to 40K tokens and often fails entirely.

## Two Testing Surfaces

### Surface 1: Tool Reliability

Does the tool itself work correctly?

| What's tested | Example failure |
|---|---|
| Tool executes without error | Shell command hangs, API times out |
| Tool returns a useful response | API returns raw JSON blob instead of structured data |
| Tool response is parseable by the model | Error message is a stack trace instead of a sentence |
| Tool response is appropriately sized | `ls` returns 10,000 lines, flooding the context |
| Tool fails gracefully with clear errors | Tool returns exit code 1 with no stderr |
| Tool is idempotent when it should be | File write creates duplicates on retry |
| Tool latency is acceptable | 30-second API call causes timeout |

These are tested with **live tool calls** in a controlled environment — real tool, real execution, measured response quality.

### Surface 2: Tool Prompt Clarity

Given a working tool, does the model use it correctly?

| What's tested | Example failure |
|---|---|
| Model calls the right tool for the task | Uses shell when file-read tool exists |
| Model passes correct arguments | Wrong path format, missing required args |
| Model interprets the response correctly | Treats an error as success, or success as error |
| Model knows the tool's constraints | Tries to write to a read-only path |
| Model knows when NOT to use the tool | Uses shell for a task the model can do directly |
| Model handles tool errors without looping | Retries the same failing command 10 times |
| Model combines tools in the right order | Tries to read a file before creating it |

These are tested with **mock tool responses** — the tool is simulated so the prompt is the only variable.

### Why They Must Be Tested Together

A tool that returns a confusing error is a tool reliability problem. But the symptom is a prompt problem — the model can't interpret the error and starts looping. The fix might be:

- **Tool side**: Change the error message from `ENOENT` to `File not found: /path/to/file`
- **Prompt side**: Add "If a tool returns an error code, report the error to the user instead of retrying"
- **Both**: Fix the error message AND teach the model to handle it

You don't know which fix is right until you test both surfaces. The harness lets you hold one constant while changing the other.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    nebo test prompt                       │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────┐ │
│  │   Fixture    │  │   Prompt     │  │   Override      │ │
│  │   (YAML)     │──│   Assembler  │──│   Injection     │ │
│  │             │  │   (real)     │  │                 │ │
│  └──────┬──────┘  └──────┬───────┘  └─────────────────┘ │
│         │                │                               │
│         ▼                ▼                               │
│  ┌─────────────────────────────────────┐                 │
│  │         Execution Engine            │                 │
│  │                                     │                 │
│  │  ┌───────────┐   ┌───────────────┐  │                 │
│  │  │ Mock Mode │   │  Live Mode    │  │                 │
│  │  │ (prompt   │   │  (tool        │  │                 │
│  │  │  testing) │   │   testing)    │  │                 │
│  │  └───────────┘   └───────────────┘  │                 │
│  │                                     │                 │
│  │         ┌─────────────┐             │                 │
│  │         │ Trace       │             │                 │
│  │         │ Capture     │             │                 │
│  │         └──────┬──────┘             │                 │
│  └────────────────┼────────────────────┘                 │
│                   │                                      │
│                   ▼                                      │
│  ┌─────────────────────────────────────┐                 │
│  │         Grader (smart model)        │                 │
│  │                                     │                 │
│  │  Evaluates:                         │                 │
│  │  - Tool call correctness            │                 │
│  │  - Tool response quality            │                 │
│  │  - Model interpretation accuracy    │                 │
│  │  - First-call success rate          │                 │
│  │  - Context pollution score          │                 │
│  └──────────────┬──────────────────────┘                 │
│                 │                                        │
│                 ▼                                        │
│  ┌─────────────────────────────────────┐                 │
│  │         Report                      │                 │
│  └─────────────────────────────────────┘                 │
└──────────────────────────────────────────────────────────┘
```

## CLI Interface

```bash
# Test tool prompt clarity (mock mode — default)
nebo test prompt \
  --fixture fixtures/shell-path-not-found.yaml \
  --override tool.shell:./overrides/shell-v2.md \
  --model qwen3.5-flash \
  --grader claude-sonnet-4-6

# Test tool reliability (live mode)
nebo test tool \
  --fixture fixtures/shell-ls-desktop.yaml \
  --live \
  --grader claude-sonnet-4-6

# Test both together (integrated mode)
nebo test integrated \
  --fixture fixtures/shell-path-not-found.yaml \
  --override tool.shell:./overrides/shell-v2.md \
  --model qwen3.5-flash \
  --grader claude-sonnet-4-6 \
  --live

# Measure first-call success rate across a suite
nebo test fcsr \
  --suite suites/all-tools.yaml \
  --model qwen3.5-flash \
  --grader claude-sonnet-4-6

# Inspect assembled prompt
nebo test prompt --dry-run \
  --fixture fixtures/shell-path-not-found.yaml \
  --override tool.shell:./overrides/shell-v2.md
```

## Fixture Format v2

The fixture now captures both surfaces — tool behavior and model behavior — in a single file.

```yaml
id: shell-path-not-found
name: "Shell tool: path does not exist"
description: >
  Agent is asked to find files in a directory that doesn't exist.
  Tests both tool error quality and model response to failure.

target_component: tool.shell

conversation:
  - role: user
    content: "Show me the screenshots on my Desktop"

# ─── TOOL RELIABILITY SURFACE ───

tool_config:
  shell:
    enabled: true

    # What the tool SHOULD return (tool quality spec)
    response_quality:
      - scenario: "path does not exist"
        requirements:
          - "Error message names the specific path that failed"
          - "Error message is a single human-readable sentence, not a stack trace"
          - "Exit code is non-zero"
          - "Response is under 200 characters"

    # Mock responses for prompt testing mode
    mock_responses:
      - pattern: "ls */Desktop/Screenshots*"
        response:
          exit_code: 1
          stdout: ""
          stderr: "ls: cannot access '/Users/almatuck/Desktop/Screenshots': No such file or directory"
      - pattern: "ls */Desktop*"
        response:
          exit_code: 1
          stdout: ""
          stderr: "ls: cannot access '/Users/almatuck/Desktop': No such file or directory"

    # What a BAD tool response looks like (for comparison)
    bad_response_examples:
      - description: "Cryptic error — model can't interpret this"
        response:
          exit_code: 2
          stdout: ""
          stderr: "ENOENT"
      - description: "Stack trace — floods context"
        response:
          exit_code: 1
          stdout: ""
          stderr: |
            Traceback (most recent call last):
              File "/usr/lib/python3/dist/shutil.py", line 789, in move
                os.rename(src, real_dst)
            FileNotFoundError: [Errno 2] No such file or directory...
            [50 more lines]

    # Response size limits
    response_budget:
      max_chars: 500
      max_lines: 10
      rationale: >
        A tool response over 500 chars starts to dominate the context.
        If a tool needs to return more, it should summarize or paginate.

# ─── TOOL PROMPT CLARITY SURFACE ───

prompt_assertions:

  # First-call correctness
  first_call:
    - id: correct-tool
      text: "Model selects the shell tool (not file-read, not browser)"
      severity: critical

    - id: correct-args
      text: "First shell command is a reasonable path to check (e.g., ~/Desktop/Screenshots or /Users/*/Desktop/Screenshots)"
      severity: critical

    - id: correct-interpretation
      text: "Model correctly interprets the 'No such file or directory' error as 'path does not exist'"
      severity: critical

  # Recovery behavior
  recovery:
    - id: max-retries
      text: "Agent attempts no more than 2 shell commands before stopping"
      severity: critical

    - id: graceful-stop
      text: "Agent communicates the limitation clearly to the user"
      severity: critical

    - id: helpful-alternative
      text: "Agent suggests a concrete next step (upload the file, provide the path, check if the directory exists)"
      severity: important

    - id: no-path-invention
      text: "Agent does NOT fabricate paths it hasn't been told about (e.g., /home/user/Pictures, /tmp/screenshots)"
      severity: important

  # Cost discipline
  cost:
    - id: token-budget
      metric: total_tokens
      threshold: 4000
      text: "Total tokens stays under 4,000"
      severity: important

    - id: tool-call-budget
      metric: tool_call_count
      threshold: 3
      text: "Total tool calls stays under 3"
      severity: important

    - id: no-context-pollution
      metric: retry_tokens
      threshold: 1000
      text: "Tokens spent on retry-related content stays under 1,000"
      severity: important

# ─── INTEGRATED ASSERTIONS ───
# These test the interaction between tool quality and model behavior

integrated_assertions:
  - id: error-comprehension
    text: "Model's response demonstrates it understood the specific error (names the path, explains it doesn't exist) rather than giving a generic failure message"
    severity: critical
    tests: "tool response quality → model interpretation pipeline"

  - id: no-error-amplification
    text: "Model does not restate the full error message back to the user — it summarizes"
    severity: important
    tests: "tool response size → model output size pipeline"

  - id: first-call-success
    text: "The model achieved its intent (inform user the path doesn't exist and offer help) with the minimum number of tool calls"
    severity: critical
    tests: "overall first-call success rate"

# ─── IDEAL BEHAVIOR ───

ideal_behavior:
  tool_calls: 1
  total_tokens: 2500
  narrative: >
    Model calls `ls ~/Desktop/Screenshots`. Gets "No such file or directory".
    Model immediately tells the user: "That directory doesn't exist.
    Could you check the path or upload the files directly?"
    One tool call. One clear response. Done.
```

## Trace Format v2

The trace captures both tool-side and model-side data for each tool call:

```json
{
  "fixture_id": "shell-path-not-found",
  "run_id": "run-2026-05-31-001",
  "model": "qwen3.5-flash",
  "mode": "mock",

  "first_call_success": true,
  "context_pollution_score": 0.05,

  "tool_calls": [
    {
      "sequence": 1,
      "intent": "List files in Desktop/Screenshots",

      "call": {
        "tool": "shell",
        "arguments": {"command": "ls ~/Desktop/Screenshots"},
        "argument_quality": {
          "correct_tool": true,
          "correct_args": true,
          "notes": "Reasonable first path to try"
        }
      },

      "response": {
        "exit_code": 1,
        "stdout": "",
        "stderr": "ls: cannot access '/Users/almatuck/Desktop/Screenshots': No such file or directory",
        "response_quality": {
          "parseable": true,
          "human_readable": true,
          "actionable": true,
          "char_count": 78,
          "within_budget": true,
          "notes": "Clear error message naming the specific path"
        }
      },

      "model_interpretation": {
        "understood_error": true,
        "action_taken": "Informed user and suggested alternative",
        "retried": false,
        "notes": "Model correctly interpreted ENOENT and stopped"
      },

      "first_call_success": true,
      "tokens_consumed": 487
    }
  ],

  "final_response": {
    "content": "I can't find a Screenshots folder on your Desktop. Could you check the exact path or upload the screenshots directly?",
    "tokens": 312,
    "appropriate_length": true,
    "contains_error_dump": false
  },

  "metrics": {
    "total_tool_calls": 1,
    "successful_first_calls": 1,
    "failed_first_calls": 0,
    "retries": 0,
    "total_tokens": 3487,
    "input_tokens": 3175,
    "output_tokens": 312,
    "retry_tokens": 0,
    "latency_ms": 1140,
    "context_pollution_ratio": 0.0
  }
}
```

### Context Pollution Score

A computed metric that quantifies how much noise the tool interaction injected:

```
context_pollution_score = retry_tokens + error_dump_tokens + redundant_explanation_tokens
                          ─────────────────────────────────────────────────────────────
                          total_tokens

0.0  = perfectly clean (no retries, no noise)
0.05 = minor noise (one small retry)
0.3  = significant pollution (multiple retries, error dumps)
0.6+ = degenerative (the screenshot scenario — most of the context is failure noise)
```

The grader calculates this by classifying each token span in the trace as either "productive" (advancing the task) or "pollution" (retries, error interpretation, redundant attempts).

## Grader Prompt v2

The grader evaluates both surfaces in a single pass:

```
You are evaluating an AI agent's tool usage in a controlled test.

Your job is to assess TWO things:
1. TOOL QUALITY — Did the tool return a response the model could work with?
2. MODEL BEHAVIOR — Did the model use the tool correctly and handle the response well?

## Scenario
{fixture.description}

## Transcript
{trace — formatted as readable conversation}

## Tool Quality Checklist
For each tool call, evaluate the TOOL'S response:
- Was the response human-readable? (not a stack trace, not a raw error code)
- Was the response appropriately sized? (under {response_budget.max_chars} chars)
- Did the response name the specific problem? (not just "error")
- Could a human reading the response understand what went wrong?

## Model Behavior Checklist
For each tool call, evaluate the MODEL's behavior:
- Did the model call the correct tool?
- Did the model pass the correct arguments?
- Did the model interpret the response correctly?
- Did the model retry unnecessarily?
- Did the model recover gracefully from errors?

## First-Call Success
For each tool call, determine:
- Did the model achieve its INTENT on this call?
- Intent is not "command succeeds" — intent is "model gets the information it needs
  to proceed with the task." A tool call that correctly returns "file not found" is
  a successful first call if the model's intent was to check whether the file exists.

## Assertions
{fixture.prompt_assertions + fixture.integrated_assertions}

## Context Pollution
Classify each section of the transcript as:
- PRODUCTIVE: Advances the task (correct tool call, useful response, task-relevant output)
- POLLUTION: Noise from failures (retries, error dumps, redundant attempts, confusion)

Calculate: pollution_tokens / total_tokens

Respond with JSON:
{
  "tool_quality": [
    {
      "tool_call_sequence": 1,
      "tool": "shell",
      "response_parseable": true,
      "response_human_readable": true,
      "response_actionable": true,
      "response_within_budget": true,
      "tool_quality_score": 1.0,
      "notes": "Clear error message"
    }
  ],
  "model_behavior": [
    {
      "tool_call_sequence": 1,
      "correct_tool": true,
      "correct_args": true,
      "correct_interpretation": true,
      "unnecessary_retry": false,
      "model_behavior_score": 1.0,
      "notes": "Model handled the error perfectly"
    }
  ],
  "assertions": [
    {"id": "correct-tool", "passed": true, "evidence": "..."},
    ...
  ],
  "first_call_success_rate": 1.0,
  "context_pollution_score": 0.0,
  "overall_notes": "..."
}
```

## CLI Report v2

```
nebo test prompt — shell-path-not-found

  Component: tool.shell (v2)
  Model:     qwen3.5-flash
  Grader:    claude-sonnet-4-6
  Mode:      mock
  Runs:      3

  ═══ FIRST-CALL SUCCESS RATE ═══

    Run 1: 1/1 (100%)    Run 2: 1/1 (100%)    Run 3: 1/1 (100%)
    Average: 100%

  ═══ CONTEXT POLLUTION ═══

    Run 1: 0.00    Run 2: 0.03    Run 3: 0.00
    Average: 0.01 (clean)

  ═══ TOOL QUALITY ═══
  ┌──────────────────────┬───────┬───────┬───────┐
  │ Check                │ Run 1 │ Run 2 │ Run 3 │
  ├──────────────────────┼───────┼───────┼───────┤
  │ Response parseable   │  ✓    │  ✓    │  ✓    │
  │ Human-readable       │  ✓    │  ✓    │  ✓    │
  │ Actionable           │  ✓    │  ✓    │  ✓    │
  │ Within size budget   │  ✓    │  ✓    │  ✓    │
  └──────────────────────┴───────┴───────┴───────┘

  ═══ MODEL BEHAVIOR ═══
  ┌──────────────────────┬───────┬───────┬───────┬─────────┐
  │ Assertion            │ Run 1 │ Run 2 │ Run 3 │ Rate    │
  ├──────────────────────┼───────┼───────┼───────┼─────────┤
  │ correct-tool         │  ✓    │  ✓    │  ✓    │ 100%    │
  │ correct-args         │  ✓    │  ✓    │  ✓    │ 100%    │
  │ correct-interpret    │  ✓    │  ✓    │  ✓    │ 100%    │
  │ max-retries          │  ✓    │  ✓    │  ✓    │ 100%    │
  │ graceful-stop        │  ✓    │  ✓    │  ✓    │ 100%    │
  │ helpful-alternative  │  ✓    │  ✗    │  ✓    │  67%    │
  │ no-path-invention    │  ✓    │  ✓    │  ✓    │ 100%    │
  │ token-budget         │  ✓    │  ✓    │  ✓    │ 100%    │
  │ tool-call-budget     │  ✓    │  ✓    │  ✓    │ 100%    │
  │ no-context-pollution │  ✓    │  ✓    │  ✓    │ 100%    │
  └──────────────────────┴───────┴───────┴───────┴─────────┘

  ═══ COST ═══

    Avg tokens: 3,412  |  Avg tool calls: 1.0  |  Avg latency: 1,180ms
    vs production baseline: -86% tokens, -90% tool calls

  ═══ COMPARISON (if --baseline provided) ═══

    Metric              Baseline (prod)    Candidate (v2)    Delta
    ──────────────────  ─────────────────  ────────────────  ──────
    First-call success  10%                100%              +90%
    Context pollution   0.62               0.01              -98%
    Avg tokens          24,609             3,412             -86%
    Avg tool calls      8.3                1.0               -88%
    Pass rate           11%                94%               +83%
```

## Tool Response Quality Testing (Live Mode)

In live mode, the harness runs the actual tool and evaluates the response quality — independent of any model. This catches tool-side problems before they ever reach the prompt.

```bash
nebo test tool \
  --tool shell \
  --scenarios fixtures/tool-scenarios/shell.yaml \
  --grader claude-sonnet-4-6
```

Tool scenario fixture:

```yaml
id: shell-response-quality
tool: shell

scenarios:
  - id: path-not-found
    command: "ls /nonexistent/path"
    response_assertions:
      - "Exit code is non-zero"
      - "Stderr contains the path that was not found"
      - "Stderr is a single line, not a stack trace"
      - "Total response is under 200 characters"

  - id: permission-denied
    command: "cat /etc/shadow"
    response_assertions:
      - "Exit code is non-zero"
      - "Stderr mentions 'permission' or 'denied'"
      - "Stderr does NOT suggest using sudo"
      - "Total response is under 200 characters"

  - id: large-directory
    command: "ls /usr/lib"
    response_assertions:
      - "Output is truncated or paginated if over 50 lines"
      - "A summary line indicates total count if truncated"
      - "Total response is under 2,000 characters"

  - id: command-not-found
    command: "nonexistent_command --flag"
    response_assertions:
      - "Error clearly states the command was not found"
      - "Error does NOT include shell initialization noise"
      - "Total response is under 200 characters"

  - id: success-simple
    command: "echo hello"
    response_assertions:
      - "Exit code is 0"
      - "Stdout contains 'hello'"
      - "No extraneous output (no shell prompts, no MOTD)"
```

When a tool response fails quality assertions, the fix is on the tool side — wrapping responses, truncating large output, formatting error messages. This is infrastructure work that pays dividends across every model and every prompt.

## Integration with Janus Stats

The harness feeds back into production monitoring. After deploying a prompt change:

```bash
# Check first-call success rate in production over last 24h
nebo test fcsr --production --since 24h

# Compare before/after a prompt deployment
nebo test fcsr --production \
  --before 2026-05-30T00:00:00Z \
  --after 2026-05-31T00:00:00Z
```

This queries Janus stats and request logs to compute the real-world first-call success rate. The harness validates the prompt in isolation. Janus validates it in production. If they diverge — if the harness says 100% but production shows 60% — the fixtures don't cover a real-world scenario and need to be expanded.

## The Full Optimization Loop

```
1. OBSERVE    Janus stats show high retry rate or token bloat for an agent
              └─→ Pull request_logs, identify the tool call that started the spiral

2. REPRODUCE  Write a fixture that replays the failing scenario
              └─→ nebo test prompt --fixture new-case.yaml
              └─→ Confirm production prompt fails the assertions

3. DIAGNOSE   Is this a tool problem or a prompt problem?
              └─→ nebo test tool --scenarios tool-check.yaml (live mode)
              └─→ If tool response is garbage → fix the tool
              └─→ If tool response is clean → fix the prompt

4. FIX        Make ONE change
              └─→ Tool side: fix error formatting, add truncation, improve response
              └─→ Prompt side: add one sentence to the tool prompt

5. VALIDATE   Test the fix in isolation
              └─→ nebo test prompt --baseline production --candidate v2
              └─→ First-call success rate improved?
              └─→ Context pollution score decreased?

6. REGRESS    Run the full suite to check for side effects
              └─→ nebo test prompt --suite suites/all-tools.yaml --override ...
              └─→ No existing tests broken?

7. DEPLOY     Ship the change to production

8. VERIFY     Monitor Janus stats post-deploy
              └─→ nebo test fcsr --production --since 24h
              └─→ Real-world first-call success rate matches harness results?
              └─→ If not → fixtures are missing a scenario → go to step 2
```

## Implementation Phases

### Phase 1: Prompt Inspection
- `nebo test prompt --dry-run`
- Hook into PromptAssembler, dump annotated prompt
- Support `--override` for component swapping
- **Value**: See exactly what the model sees. Often reveals the bug immediately.

### Phase 2: Mock Execution + Trace Capture
- Fixture YAML parsing
- Tool call interception with mock response matching
- Full trace capture (tool calls, responses, model output, token counts)
- `--runs` for variance measurement
- **Value**: Reproduce failures deterministically.

### Phase 3: Grader Integration
- Grader prompt template (evaluates both tool quality and model behavior)
- First-call success rate computation
- Context pollution score computation
- CLI report output
- **Value**: Automated pass/fail on prompt changes.

### Phase 4: Live Tool Testing
- `nebo test tool` command
- Real tool execution with response quality assertions
- Tool response budget enforcement
- **Value**: Catch tool-side problems before they reach the model.

### Phase 5: Comparison Mode + Suites
- `--baseline` / `--candidate` comparison
- Suite runner
- Historical results in `.nebo/test-results/`
- Regression detection
- **Value**: Safe, measured prompt iteration.

### Phase 6: Production Integration
- `nebo test fcsr --production` queries Janus stats
- Before/after deployment comparison
- Alert on first-call success rate regression
- **Value**: Close the loop between testing and production.

## Files

### New
| File | Purpose |
|---|---|
| `crates/agent/src/testing/mod.rs` | Module entry |
| `crates/agent/src/testing/fixture.rs` | Fixture parsing |
| `crates/agent/src/testing/mock_tools.rs` | Tool call interception + mock responses |
| `crates/agent/src/testing/live_tools.rs` | Live tool execution + response quality |
| `crates/agent/src/testing/trace.rs` | Trace capture and serialization |
| `crates/agent/src/testing/grader.rs` | Grader prompt + result parsing |
| `crates/agent/src/testing/metrics.rs` | FCSR + context pollution computation |
| `crates/agent/src/testing/reporter.rs` | CLI output formatting |
| `crates/agent/src/testing/suite.rs` | Suite runner |
| `crates/cli/src/commands/test_prompt.rs` | `nebo test prompt` command |
| `crates/cli/src/commands/test_tool.rs` | `nebo test tool` command |
| `crates/cli/src/commands/test_fcsr.rs` | `nebo test fcsr` command |

### Modified
| File | Change |
|---|---|
| `crates/agent/src/prompt.rs` | Add `replace_component()` + section annotations |
| `crates/cli/src/main.rs` | Register test subcommands |
| `Cargo.toml` | Add `serde_yaml` |

### Artifacts
```
fixtures/                          # Test scenarios
  tools/
    shell-path-not-found.yaml
    shell-permission-denied.yaml
    shell-large-output.yaml
    file-write-permission.yaml
    ...
  agents/
    donna-research-loop.yaml
    ...
  tool-scenarios/                  # Live tool response quality tests
    shell.yaml
    file-write.yaml
    browser.yaml
    ...
overrides/                         # Candidate prompt versions
  tool-shell-v2.md
  tool-shell-v3.md
  ...
suites/                            # Grouped test suites
  tool-shell.yaml
  tool-file-write.yaml
  all-tools.yaml
  ...
.nebo/test-results/                # Run history (gitignored)
```
