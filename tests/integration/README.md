# Integration Tests

Structured integration tests for Nebo's artifact lifecycle — skills, tools, workflows, and roles.

## Structure

```
tests/integration/
├── plan.md              # Test plan — what to test, how, expected results
├── README.md            # This file
└── results/
    ├── TEMPLATE.md      # Copy this for each test run
    └── YYYY-MM-DD-{platform}-{run}.md   # Completed results
```

## Running a Test

1. Copy the template:
   ```
   cp results/TEMPLATE.md results/$(date +%Y-%m-%d)-macos-001.md
   ```

2. Open the results file and fill in the header (platform, version, commit hash).

3. Follow `plan.md` section by section. For each test:
   - Execute the command exactly as written
   - Record `PASS`, `FAIL`, or `SKIP` in the Result column
   - If `FAIL`: write the error/unexpected behavior in Notes — **do not attempt to fix**
   - If `SKIP`: write the reason (e.g., "no NeboLoop code available")

4. After completing all sections, fill in the Summary table.

5. Compare against the previous results file:
   - Any test that was `PASS` before and is now `FAIL` is a **regression**
   - Log regressions in the "Regressions from Previous Run" section

## Naming Convention

```
YYYY-MM-DD-{platform}-{run}.md
```

- `platform`: `macos` or `windows`
- `run`: sequential number for that day/platform (001, 002, ...)

Examples:
- `2026-03-06-macos-001.md` — First macOS run on March 6
- `2026-03-06-macos-002.md` — Second macOS run (after fixes)
- `2026-03-07-windows-001.md` — First Windows run

## Tracking Regressions

Each results file has two tracking sections:

- **Regressions from Previous Run** — Tests that passed before but fail now. This is the most critical section. If this table has entries, something broke.
- **New Failures** — Tests failing for the first time (not regressions, just newly discovered issues).

To diff two runs quickly:
```
diff results/2026-03-06-macos-001.md results/2026-03-06-macos-002.md
```

## Rules

1. **Observe and document only** — never attempt to fix, retry, or self-heal during a test run
2. **One result file per run** — never overwrite a previous result
3. **Every check gets a verdict** — PASS, FAIL, or SKIP. No blank results.
4. **Failures get notes** — always record what went wrong
5. **Capture raw output** — paste or link full CLI/API output at the bottom
