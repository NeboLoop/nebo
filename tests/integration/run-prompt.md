# Integration Test Run Prompt

Give this prompt to Nebo (via `nebo chat -i` or Claude Code) to execute a full integration test run.

---

## The Prompt

```
You are running a structured integration test. Your ONLY job is to execute each test, observe the result, and document it. You must NEVER attempt to fix, debug, retry, or self-heal anything. If something fails, record the failure and move on.

Rules:
1. OBSERVE AND DOCUMENT ONLY — never fix anything
2. Record every check as PASS, FAIL, or SKIP — no blanks
3. For FAIL: write exactly what happened in Notes
4. For SKIP: ONLY allowed if the test literally cannot run (e.g., no test binary compiled for sideload). "Covered by another test" is NOT a valid skip reason.
5. Do not retry failed tests
6. Do not modify any source code
7. Do not suggest fixes
8. Capture raw output for every test

CRITICAL — DO NOT SKIP TESTS:
- You MUST execute EVERY SINGLE TEST in EVERY section. No exceptions.
- Each test in the plan is a SEPARATE test that MUST be executed independently.
- Do NOT group, merge, or claim tests are "covered by" other sections.
- Section 2 (Skills) tests S-01 through S-12 are SEPARATE from AT-19. Execute ALL of them.
- Section 4 (Workflows) tests W-01 through W-10 are SEPARATE from AT-22. Execute ALL of them.
- Section 5 (Roles) tests R-01 through R-09 are SEPARATE from AT-20/AT-21. Execute ALL of them.
- Section 3 (Tools) T-01 and T-02 are REST API calls to /integrations/tools — execute both.
- If a test has both "Agent tool" and "REST API" methods, execute BOTH.
- For marketplace installs (S-10, W-09, R-07): use `curl -X POST http://localhost:27895/api/v1/codes -H "Content-Type: application/json" -d '{"code": "CODE-HERE"}'`
- The ONLY valid SKIP reasons are: "no test binary compiled", "requires hardware not available" (e.g., external display), or "would cause destructive system change" (e.g., toggling DND).

Start by reading the test plan and results template:
- Read tests/integration/plan.md
- Copy tests/integration/results/TEMPLATE.md to tests/integration/results/YYYY-MM-DD-{platform}-{run}.md (use today's date, detect the platform, auto-increment run number if a file for today already exists)
- Fill in the header: date, platform (run `uname -a`), Nebo version (run `git rev-parse --short HEAD`), tester: "nebo-agent"

Then execute every test in plan.md, section by section, in order:
1. Pre-Flight Checks (PF-01 through PF-05) — ALL 5
2. Agent Tools (AT-01 through AT-23) — ALL 23
3. Skills (S-01 through S-15) — ALL 15. Create test-integration, test lifecycle, bundled resources (browse + read_resource), script execution (execute tool), capability declarations, install SKIL-RFBM-XCYT, test REST CRUD.
4. Built-in Tools (T-01, T-02) — REST calls to /integrations/tools. Verify all 10 tools registered.
5. Workflows (W-01 through W-14) — ALL 14. Create test-workflow, run it, check status, multi-activity chaining (W-11), retry/fallback (W-12), token budget (W-13), tools restriction (W-14), install WORK-SW4Z-5XKN, test REST CRUD.
6. Roles (R-01 through R-09d + R-01a through R-06a) — ALL tests. Create test-role, activate, schedule/heartbeat/event triggers (R-09a/b/c), event emit fan-out (R-09d), install ROLE-KG82-KM2G, test REST CRUD.
7. Cross-Cutting (X-01 through X-13) — ALL 13. Includes MCP integration (X-11), qualified names/versioning (X-12), dependency cascade (X-13).
8. Cleanup

For agent tool tests: use the tool calls exactly as specified in the plan.
For REST API tests: use os(resource: "shell", action: "exec", command: "curl ...") to call the API.
For filesystem checks: use os(resource: "file", action: "read" or "glob") to verify files.

After each test, immediately write the result to the results file using os(resource: "file", action: "edit"). Do not batch — write results as you go so nothing is lost if the run is interrupted.

After all tests complete:
- Fill in the Summary table with totals
- If a previous results file exists in tests/integration/results/, compare against it and fill in the Regressions table
- Fill in New Failures table
- Write the final results file

NeboLoop test codes (use POST /api/v1/codes to install):
- SKILL code: SKIL-RFBM-XCYT
- WORK code: WORK-SW4Z-5XKN
- ROLE code: ROLE-KG82-KM2G
```
