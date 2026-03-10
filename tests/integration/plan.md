# Nebo Integration Test Plan

**Purpose:** Structured, exhaustive integration test of every artifact lifecycle operation. Run manually via the CLI (`nebo chat -i`) and REST API. Tests are **observe-and-document only** — never attempt to fix, retry, or self-heal. If something fails, log the failure with full details and move on to the next test.

**Platforms:** macOS (primary), then Windows.

**Results:** Each run produces a results file in `results/`. Copy `results/TEMPLATE.md` to `results/YYYY-MM-DD-{platform}-{run}.md` and fill it in. See `README.md` for naming and regression tracking.

---

## Pre-Requisites

Before running any tests:

1. **Fresh build** — `cargo build` completes with zero errors
2. **NeboLoop account** — Bot is connected (`NEBO-XXXX-XXXX` code redeemed)
3. **AI provider configured** — At least one provider active (run `nebo providers list`)
4. **Test codes** — Real NeboLoop codes for marketplace tests:
   - `SKIL-RFBM-XCYT` — Published skill
   - ~~`TOOL-XXXX-XXXX`~~ — Tools merged into Skills; no separate tool codes
   - `WORK-SW4Z-5XKN` — Published workflow
   - `ROLE-KG82-KM2G` — Published role
5. **Record platform** — Note OS, version, architecture before starting

---

## Pre-Flight Checks (PF)

Run these first. All must pass before proceeding.

### PF-01: Doctor Check

```
nebo doctor
```

| Check | Expected | Result |
|-------|----------|--------|
| Data dir exists | Path printed | |
| Setup complete | `true` | |
| Bot ID | UUID present | |
| Database | `OK` | |
| Skills dir | Path printed, count >= 0 | |

### PF-02: Server Starts

```
nebo serve
```

| Check | Expected | Result |
|-------|----------|--------|
| Server binds | `Listening on 127.0.0.1:27895` | |
| No panic/crash | Process stays alive | |

### PF-03: Directory Structure

After first startup, verify the data directory:

```
GET http://localhost:27895/api/v1/agent/status
```

Also manually inspect:

| Path | Expected | Result |
|------|----------|--------|
| `<data_dir>/nebo/skills/` | Directory exists | |
| `<data_dir>/nebo/tools/` | Directory exists | |
| `<data_dir>/nebo/workflows/` | Directory exists | |
| `<data_dir>/nebo/roles/` | Directory exists | |
| `<data_dir>/user/skills/` | Directory exists | |
| `<data_dir>/user/tools/` | Directory exists | |
| `<data_dir>/user/workflows/` | Directory exists | |
| `<data_dir>/user/roles/` | Directory exists | |
| `<data_dir>/data/nebo.db` | File exists | |

### PF-04: Providers Ready

```
nebo providers list
```

| Check | Expected | Result |
|-------|----------|--------|
| At least 1 provider listed | Provider name + status shown | |

### PF-05: Interactive Chat Responds

```
nebo chat "say hello"
```

| Check | Expected | Result |
|-------|----------|--------|
| Agent responds | Non-empty text response | |
| No tool errors | Clean response | |

---

## Section 1: Agent Built-in Tools (AT)

Functional tests for each of the 10 built-in agent tools. Every test must verify **actual behavior**, not just "tool didn't crash." Write data, read it back, verify content. Create things, verify they exist, clean them up.

### AT-01: os (file) — Write + Read Round-Trip

Write a test file, read it back, verify contents match, then delete it.

```
nebo chat "use os(resource: \"file\", action: \"write\", path: \"/tmp/nebo-at01-test.txt\", content: \"AT01_ROUND_TRIP_PASS\")"
```

```
nebo chat "use os(resource: \"file\", action: \"read\", path: \"/tmp/nebo-at01-test.txt\")"
```

```
nebo chat "use os(resource: \"shell\", action: \"exec\", command: \"rm /tmp/nebo-at01-test.txt\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Write completes | File created at `/tmp/nebo-at01-test.txt` | |
| Read returns exact content | Output contains `AT01_ROUND_TRIP_PASS` | |
| Cleanup | File deleted | |

### AT-02: os (shell) — Piped Command + Exit Code

Execute a multi-step shell command and verify structured output.

```
nebo chat "use os(resource: \"shell\", action: \"exec\", command: \"echo '{\"test\": \"AT02_PASS\", \"pid\": '$$'}' | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d[\"test\"])'\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Output is `AT02_PASS` | Piped command parsed JSON and extracted field | |
| Exit code 0 | No error in output | |

### AT-03: os (app) — Verify Known App Running

List running apps and verify `Finder` appears (always running on macOS).

```
nebo chat "use os(resource: \"app\", action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| App list returned | Non-empty array | |
| Finder present | `Finder` appears in list (always running on macOS) | |
| Structure | Each entry has app name | |

### AT-04: os (settings) — Verify Battery Data Structure

```
nebo chat "use os(resource: \"settings\", action: \"battery\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Percentage present | Output contains a number 0-100 | |
| Power source | Output mentions AC/battery/charging status | |

### AT-05: os (music) — Graceful When Nothing Playing

```
nebo chat "use os(resource: \"music\", action: \"status\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| No crash | `is_error: false` — must not crash when nothing playing | |
| Meaningful output | Returns either track info (`name - artist [state]`) or `Not playing` / `stopped` | |

### AT-06: os (search) — Find the Database File

Search for `nebo.db` and verify the data directory path appears in results.

```
nebo chat "use os(resource: \"search\", action: \"search\", query: \"nebo.db\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Results returned | At least 1 result | |
| Correct file found | Path contains `Library/Application Support/Nebo` (macOS) or equivalent | |

### AT-07: os (window) — Verify Window Structure

```
nebo chat "use os(resource: \"window\", action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Windows listed | At least 1 window (test is running in a terminal) | |
| Structure | Each window has `app` name and `title` | |
| Position data | Windows have position (x, y) and size (w, h) | |

### AT-08: os (clipboard) — Write + Read Round-Trip

Write a known string to clipboard, read it back, verify it matches.

```
nebo chat "use os(resource: \"clipboard\", action: \"write\", text: \"AT08_CLIPBOARD_PASS\")"
```

```
nebo chat "use os(resource: \"clipboard\", action: \"read\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Write completes | Clipboard updated | |
| Read returns exact text | Output contains `AT08_CLIPBOARD_PASS` | |

### AT-09: os (keychain) — Store + Find + Delete Credential

Full keychain round-trip. Store a test credential, find it, delete it.

```
nebo chat "use os(resource: \"keychain\", action: \"store\", label: \"nebo-at09-test\", account: \"testuser\", password: \"AT09_PASS\")"
```

```
nebo chat "use os(resource: \"keychain\", action: \"find\", label: \"nebo-at09-test\")"
```

```
nebo chat "use os(resource: \"keychain\", action: \"delete\", label: \"nebo-at09-test\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Store completes | Credential saved to keychain | |
| Find returns it | Output contains `nebo-at09-test` or credential details | |
| Delete completes | Credential removed | |
| Find after delete | Returns "not found" or empty | |

### AT-10: os (mail) — Verify Unread Count Is Numeric

```
nebo chat "use os(resource: \"mail\", action: \"unread\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Count returned | Output contains a number (0 or more) | |
| Format | Lists mailboxes or total count | |

### AT-11: os (calendar) — Verify Date Awareness

```
nebo chat "use os(resource: \"calendar\", action: \"today\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Today's date | Output references today's date or says "no events" | |
| Format | If events exist: title, time, calendar name | |

### AT-12: web (http) — Fetch + Verify JSON Structure

Fetch httpbin.org and verify the response is parseable JSON with expected fields.

```
nebo chat "use web(resource: \"http\", action: \"fetch\", url: \"https://httpbin.org/get\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| HTTP 200 | Response received | |
| JSON body | Contains `origin` field (requester IP) | |
| Headers present | Contains `Host: httpbin.org` in response | |

### AT-13: web (search) — Verify Results Have URLs

Search for a well-known term and verify structured results are returned.

```
nebo chat "use web(resource: \"search\", action: \"search\", query: \"rust programming language\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Results returned | At least 1 result | |
| Each result has title | Non-empty title string | |
| Each result has URL | Valid URL starting with `http` | |
| Each result has snippet | Description text present | |

### AT-14: agent (memory) — Full Lifecycle: Store + Recall + Search + Update + Delete

```
nebo chat "use agent(resource: \"memory\", action: \"store\", key: \"at_test\", value: \"AT_PASS_V1\")"
```

```
nebo chat "use agent(resource: \"memory\", action: \"recall\", key: \"at_test\")"
```

```
nebo chat "use agent(resource: \"memory\", action: \"store\", key: \"at_test\", value: \"AT_PASS_V2\")"
```

```
nebo chat "use agent(resource: \"memory\", action: \"recall\", key: \"at_test\")"
```

```
nebo chat "use agent(resource: \"memory\", action: \"delete\", key: \"at_test\")"
```

```
nebo chat "use agent(resource: \"memory\", action: \"recall\", key: \"at_test\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Store V1 | Memory saved | |
| Recall V1 | Returns exactly `AT_PASS_V1` | |
| Update to V2 | Overwrite succeeds | |
| Recall V2 | Returns exactly `AT_PASS_V2` (not V1) | |
| Delete | Memory removed | |
| Recall after delete | Returns "not found" or empty | |

### AT-15: agent (session) — Verify Current Session Exists

```
nebo chat "use agent(resource: \"session\", action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| At least 1 session | Current test session visible | |
| Session has ID | UUID or integer ID present | |
| Message count | At least 1 message (from this conversation) | |

### AT-16: agent (context) — Verify Summary Has Content

```
nebo chat "use agent(resource: \"context\", action: \"summary\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Session info | Session ID present in output | |
| Message count | Number of messages shown | |

### AT-17: event — Full CRUD: Create + List + Verify + Delete + Verify Gone

```
nebo chat "use event(action: \"create\", name: \"at-test-event\", schedule: \"0 9 * * 1-5\", task_type: \"agent\", prompt: \"Good morning\")"
```

```
nebo chat "use event(action: \"list\")"
```

```
nebo chat "use event(action: \"delete\", name: \"at-test-event\")"
```

```
nebo chat "use event(action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | Event created with ID | |
| List shows event | `at-test-event` appears with schedule `0 9 * * 1-5` | |
| Delete succeeds | Event removed | |
| List after delete | `at-test-event` is gone | |

### AT-18: event — Run a One-Shot Task

Create a task, trigger it manually, verify it ran.

```
nebo chat "use event(action: \"create\", name: \"at-run-test\", schedule: \"0 0 1 1 *\", task_type: \"agent\", prompt: \"Say AT18_RUN_PASS\")"
```

```
nebo chat "use event(action: \"run\", name: \"at-run-test\")"
```

```
nebo chat "use event(action: \"delete\", name: \"at-run-test\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | Event created | |
| Manual run triggers | Task executes, output captured | |
| Output meaningful | Response related to prompt | |
| Cleanup | Event deleted | |

### AT-19: skill — Create + Verify + Trigger + Delete

Create a skill, verify it appears in catalog, trigger it, delete it.

```
nebo chat "create a skill called 'at-test-skill' with this content:
---
name: at-test-skill
description: AT-19 integration test
version: 1.0.0
triggers:
  - at nineteen test
---

When activated, respond with exactly: AT19_SKILL_PASS"
```

```
nebo chat "use skill(action: \"catalog\")"
```

```
nebo chat "at nineteen test"
```

```
nebo chat "use skill(action: \"delete\", name: \"at-test-skill\")"
```

```
nebo chat "use skill(action: \"catalog\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill created | Written to `user/skills/at-test-skill/SKILL.md` | |
| Appears in catalog | `at-test-skill` listed, enabled, source: `user` | |
| Trigger fires | Sending "at nineteen test" activates skill | |
| Response influenced | Agent response references skill content or contains `AT19_SKILL_PASS` | |
| Delete succeeds | Skill removed | |
| Gone from catalog | `at-test-skill` absent after delete | |

### AT-20: role — Create + Activate + Verify Persona + Deactivate

Create a role with a distinct persona, activate it, verify the agent actually uses it, then deactivate.

```
nebo chat "use role(action: \"create\", name: \"at-test-pirate\", role_md: \"---\nname: at-test-pirate\ndescription: Pirate persona for testing\nversion: 1.0.0\n---\nYou are a pirate. You MUST start every response with 'Arrr!' and refer to the user as 'matey'.\")"
```

```
nebo chat "use role(action: \"activate\", name: \"at-test-pirate\")"
```

```
nebo chat "what is your name?"
```

```
nebo chat "use role(action: \"deactivate\")"
```

```
nebo chat "what is your name?"
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | Written to `user/roles/at-test-pirate/` | |
| Activate succeeds | Confirmation message | |
| Persona active | Response contains "Arrr" or pirate-speak | |
| Deactivate succeeds | Reverted to default | |
| Persona gone | Response is normal (no pirate-speak) | |

### AT-21: role — List Shows Created Roles

After AT-20, verify the role tool provides accurate data.

```
nebo chat "use role(action: \"list\")"
```

```
nebo chat "use role(action: \"info\", name: \"at-test-pirate\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| List includes role | `at-test-pirate` present | |
| Info returns details | Description contains "Pirate persona" | |
| Version shown | `1.0.0` | |

Clean up: delete `user/roles/at-test-pirate/` directory.

### AT-22: work — Create + Run + Verify Output + Delete

Create a workflow, run it, verify the output matches, then delete.

```
nebo chat "create a workflow called 'at-test-workflow' with this definition:
{
  \"name\": \"at-test-workflow\",
  \"version\": \"1.0.0\",
  \"description\": \"AT-22 integration test\",
  \"activities\": [
    {
      \"id\": \"step1\",
      \"type\": \"agent\",
      \"prompt\": \"Say exactly: AT22_WORKFLOW_PASS\",
      \"tools\": []
    }
  ]
}"
```

```
nebo chat "use work(resource: \"at-test-workflow\", action: \"run\")"
```

```
nebo chat "use work(resource: \"at-test-workflow\", action: \"status\")"
```

```
nebo chat "use work(action: \"uninstall\", id: \"at-test-workflow\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Workflow created | Stored in `user/workflows/` | |
| Run starts | Returns run_id | |
| Status shows result | completed/failed with output | |
| Output meaningful | Response related to prompt | |
| Delete succeeds | Workflow removed from list | |

### AT-23: message — Toggle DND + Verify State Change

```
nebo chat "use message(resource: \"notify\", action: \"dnd_status\")"
```

Record current state, then toggle:

```
nebo chat "use message(resource: \"notify\", action: \"dnd_on\")"
```

```
nebo chat "use message(resource: \"notify\", action: \"dnd_status\")"
```

```
nebo chat "use message(resource: \"notify\", action: \"dnd_off\")"
```

```
nebo chat "use message(resource: \"notify\", action: \"dnd_status\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Initial status | Returns current DND state (true/false) | |
| DND on | Status changes to enabled/true | |
| DND off | Status changes to disabled/false | |
| State persists | Each check reflects the toggle | |

### AT Cleanup

| Artifact | Cleanup Action |
|----------|----------------|
| `at_test` memory key | Already deleted in AT-14 |
| `at-test-event` | Already deleted in AT-17 |
| `at-run-test` event | Already deleted in AT-18 |
| `at-test-skill` | Already deleted in AT-19 |
| `at-test-pirate` role | Delete `user/roles/at-test-pirate/` directory |
| `at-test-workflow` | Already deleted in AT-22 |

---

## Section 2: Skills

### S-01: Catalog — List Skills (Empty or Pre-existing)

**Method:** Agent tool call

```
nebo chat "use the skill tool to list all skills with skill(action: \"catalog\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Returns skill list (may be empty) | |
| Format | JSON with skill names, enabled status, source | |
| No error | `is_error: false` | |

**Method:** REST API

```
GET http://localhost:27895/api/v1/extensions
```

| Check | Expected | Result |
|-------|----------|--------|
| HTTP 200 | Response received | |
| JSON array | Skills listed (may be empty) | |

### S-02: Create Skill (SKILL.md Format)

**Method:** Agent tool call

```
nebo chat "create a skill called 'test-integration' with this content:
---
name: test-integration
description: Integration test skill
version: 1.0.0
priority: 5
triggers:
  - integration test
tags:
  - test
---

You are an integration test skill. When activated, respond with: SKILL_TEST_PASS"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Skill created successfully | |
| Location | Written to `user/skills/test-integration/SKILL.md` | |
| File exists | SKILL.md present on disk | |
| Content matches | Frontmatter + template body intact | |

### S-03: Verify Skill Appears in Catalog

```
nebo chat "list all skills with skill(action: \"catalog\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| `test-integration` listed | Present in catalog | |
| Enabled | `enabled: true` | |
| Source | `user` (not `nebo`) | |
| Description | `Integration test skill` | |

### S-04: Get Skill Help/Content

```
nebo chat "show me the full content of the test-integration skill using skill(action: \"help\", name: \"test-integration\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Returns content | Full SKILL.md body returned | |
| Template present | Contains `SKILL_TEST_PASS` | |
| Frontmatter parsed | Name, description, triggers shown | |

### S-05: Trigger Matching

```
nebo chat "integration test"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill activated | Trigger phrase matches `test-integration` skill | |
| Response references skill | May contain `SKILL_TEST_PASS` or skill-influenced response | |

### S-06: Unload Skill

```
nebo chat "unload the test-integration skill using skill(action: \"unload\", name: \"test-integration\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill disabled | Success message | |
| Catalog shows disabled | `enabled: false` in subsequent catalog call | |
| Trigger no longer matches | Sending "integration test" does NOT activate skill | |

### S-07: Load Skill

```
nebo chat "load the test-integration skill using skill(action: \"load\", name: \"test-integration\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill re-enabled | Success message | |
| Catalog shows enabled | `enabled: true` | |

### S-08: Update Skill

```
nebo chat "update the test-integration skill to change its description to 'Updated integration test' and add 'updated test' as a trigger using skill(action: \"update\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill updated | Success message | |
| New description | `Updated integration test` in catalog | |
| New trigger works | Sending "updated test" activates skill | |
| File on disk updated | SKILL.md reflects changes | |

### S-09: Delete Skill

```
nebo chat "delete the test-integration skill using skill(action: \"delete\", name: \"test-integration\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill removed | Success message | |
| Not in catalog | Absent from subsequent catalog call | |
| File removed | `user/skills/test-integration/` directory gone | |

### S-10: Install Skill from NeboLoop (SKIL-RFBM-XCYT)

**REST method:**
```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "SKIL-RFBM-XCYT"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Code accepted | 200, `{"success": true, ...}` | |
| Download succeeds | Skill installed | |
| Installed to `nebo/skills/` | Sealed .napp in `nebo/skills/` | |
| Appears in catalog | `skill(action: "catalog")` lists it with source `nebo` | |
| Content readable | `skill(action: "help")` returns SKILL.md | |

### S-11: Verify Installed Skill is Read-Only

```
nebo chat "delete the [installed-skill-name] skill"
```

| Check | Expected | Result |
|-------|----------|--------|
| Delete blocked | Error: cannot delete installed (nebo) skill | |
| Skill still present | Remains in catalog | |

### S-12: REST API — Skill CRUD

```
POST http://localhost:27895/api/v1/skills
{
  "name": "api-test-skill",
  "content": "---\nname: api-test-skill\ndescription: REST API test\n---\nAPI test body"
}
```

| Check | Expected | Result |
|-------|----------|--------|
| POST creates skill | HTTP 200/201 | |
| GET returns skill | `GET /skills/api-test-skill` returns content | |
| PUT updates skill | `PUT /skills/api-test-skill` succeeds | |
| POST toggle disables | `POST /skills/api-test-skill/toggle` → disabled | |
| POST toggle enables | `POST /skills/api-test-skill/toggle` → enabled | |
| DELETE removes skill | `DELETE /skills/api-test-skill` succeeds | |
| GET returns 404 | `GET /skills/api-test-skill` → not found | |

### S-13: Bundled Resources — Browse + Read

> **Requires:** S-02 created `test-integration` skill. Add a resource file first.

**Setup:** Create a resource file inside the skill directory:
```
os(resource: "file", action: "write", path: "{data_dir}/user/skills/test-integration/scripts/helper.py", content: "print('hello from bundled script')")
```

**Agent tool — Browse:**
```
skill(action: "browse", name: "test-integration")
```

| Check | Expected | Result |
|-------|----------|--------|
| Browse returns file list | Contains `scripts/helper.py` | |
| Lists SKILL.md | SKILL.md also listed | |

**Agent tool — Read resource:**
```
skill(action: "read_resource", name: "test-integration", path: "scripts/helper.py")
```

| Check | Expected | Result |
|-------|----------|--------|
| Content returned | Contains `print('hello from bundled script')` | |
| No error | 200 / success | |

### S-14: Script Execution via Execute Tool

> **Requires:** S-13 created a bundled Python script in `test-integration`

**Agent tool:**
```
execute(skill: "test-integration", script: "scripts/helper.py")
```

| Check | Expected | Result |
|-------|----------|--------|
| Script runs | Returns output containing `hello from bundled script` | |
| Exit code | 0 (success) | |
| Runtime resolved | Python runtime found (system or /tmp/nebo-runtimes/) | |

### S-15: Capability Declarations

> **Tests:** Create a skill with explicit capabilities and verify they're stored/parsed

**Setup:** Create skill with capabilities in frontmatter:
```
skill(action: "create", name: "cap-test", content: "---\nname: cap-test\ndescription: Capability test\ncapabilities:\n  - storage\n  - network\n---\nSkill with declared capabilities.")
```

| Check | Expected | Result |
|-------|----------|--------|
| Skill created | Success | |
| Help shows capabilities | `skill(action: "help", name: "cap-test")` includes `capabilities: [storage, network]` | |
| Catalog entry | Catalog lists the skill | |

**Cleanup:**
```
skill(action: "delete", name: "cap-test")
```

---

## Section 3: Built-in Tools (T)

> **Note:** Tools and skills have been merged. The old `/api/v1/tools` CRUD endpoints are removed. Built-in tools (os, web, agent, etc.) are registered in-process. This section tests built-in tool discovery via the integrations endpoint.

### T-01: List Built-in Tools

**Method:** REST API

```
GET http://localhost:27895/integrations/tools
```

| Check | Expected | Result |
|-------|----------|--------|
| HTTP 200 | Response received | |
| JSON array | Built-in tools listed | |
| Contains `os` | The os tool is in the list | |
| Contains `web` | The web tool is in the list | |
| Contains `agent` | The agent tool is in the list | |
| Contains `skill` | The skill tool is in the list | |
| Contains `role` | The role tool is in the list | |
| Contains `work` | The work tool is in the list | |
| Contains `event` | The event tool is in the list | |
| Contains `message` | The message tool is in the list | |
| Contains `execute` | The execute tool is in the list | |
| Contains `loop` | The loop tool is in the list | |
| Each has name | Non-empty name field | |
| Each has description | Non-empty description | |

### T-02: Verify Tool Schema Endpoint

For each built-in tool, verify the schema is well-formed:

```
GET http://localhost:27895/integrations/tools
```

| Check | Expected | Result |
|-------|----------|--------|
| Schema present | Each tool has a JSON schema object | |
| Properties defined | Schema has `properties` with at least `action` | |
| Type is object | Schema `type` is `"object"` | |

---

## Section 4: Workflows

### W-01: List Workflows (Empty or Pre-existing)

**Method:** Agent tool call

```
nebo chat "list all workflows using work(action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Returns workflow list (may be empty) | |
| Format | JSON with id, name, version, status | |

**Method:** REST API

```
GET http://localhost:27895/api/v1/workflows
```

| Check | Expected | Result |
|-------|----------|--------|
| HTTP 200 | Response received | |
| JSON array | Workflows listed | |

### W-02: Create Workflow (User Path)

**Method:** Agent tool call

```
nebo chat "create a workflow called 'test-workflow' with this definition:
{
  \"name\": \"test-workflow\",
  \"version\": \"1.0.0\",
  \"description\": \"Integration test workflow\",
  \"activities\": [
    {
      \"id\": \"step1\",
      \"type\": \"agent\",
      \"prompt\": \"Say WORKFLOW_TEST_PASS\",
      \"tools\": []
    }
  ]
}"
```

| Check | Expected | Result |
|-------|----------|--------|
| Workflow created | Success message with workflow ID | |
| Stored in `user/workflows/` | workflow.json on disk | |
| DB row created | Metadata in `workflows` table | |

### W-03: Verify Workflow in List

```
nebo chat "list all workflows using work(action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| `test-workflow` listed | Present in list | |
| Status | Enabled | |
| Definition intact | Name, description match | |

### W-04: Run Workflow

```
nebo chat "run the test-workflow using work(resource: \"test-workflow\", action: \"run\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Run started | Returns run_id | |
| Status transitions | pending → running → completed/failed | |
| Result captured | Output stored in workflow_runs | |

### W-05: Check Workflow Run Status

```
nebo chat "check the status of test-workflow using work(resource: \"test-workflow\", action: \"status\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Status returned | Shows latest run status | |
| Run ID matches | Same run_id from W-04 | |
| Final status | `completed` or `failed` (document which) | |

### W-06: List Workflow Runs

```
nebo chat "list runs for test-workflow using work(resource: \"test-workflow\", action: \"runs\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Run history | At least 1 run listed | |
| Run details | Each run has id, status, timestamps | |

### W-07: Toggle Workflow

```
nebo chat "disable test-workflow using work(resource: \"test-workflow\", action: \"toggle\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Workflow disabled | Status changes to disabled | |
| Toggle back | Re-enable succeeds | |

### W-08: Delete Workflow (via Uninstall)

```
nebo chat "uninstall the test-workflow using work(action: \"uninstall\", id: \"[workflow-id]\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Workflow removed | Success message | |
| Not in list | Absent from subsequent list | |
| Files cleaned | `user/workflows/test-workflow/` removed | |
| DB row updated/removed | No longer queryable | |

### W-09: Install Workflow from NeboLoop (WORK-SW4Z-5XKN)

**REST method:**
```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "WORK-SW4Z-5XKN"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Code accepted | 200, `{"success": true, ...}` | |
| Download succeeds | Workflow installed | |
| Installed to `nebo/workflows/` | Sealed .napp stored | |
| Appears in list | `work(action: "list")` shows it | |
| Definition readable | workflow.json readable | |

### W-10: REST API — Workflow CRUD

| Operation | Endpoint | Expected | Result |
|-----------|----------|----------|--------|
| List | `GET /workflows` | 200, JSON array | |
| Create | `POST /workflows` | 200/201, workflow info | |
| Get | `GET /workflows/{id}` | 200, full workflow | |
| Update | `PUT /workflows/{id}` | 200 | |
| Run | `POST /workflows/{id}/run` | 200, run_id | |
| Toggle | `POST /workflows/{id}/toggle` | 200 | |
| Delete | `DELETE /workflows/{id}` | 200 | |
| List runs | `GET /workflows/{id}/runs` | 200, run array | |

### W-11: Multi-Activity Workflow — Chained Steps

> **Tests:** Workflow with 2+ activities where step 2 uses step 1's output

**REST method:**
```
POST http://localhost:27895/api/v1/workflows
{
  "name": "multi-step-test",
  "definition": {
    "name": "multi-step-test",
    "description": "Tests chained activities",
    "interface": {
      "inputs": { "topic": { "type": "string", "description": "Research topic" } },
      "outputs": { "result": { "type": "string" } }
    },
    "activities": [
      {
        "id": "step1",
        "type": "agent",
        "prompt": "Generate a single fun fact about {{inputs.topic}}. Output only the fact.",
        "tools": ["agent"],
        "token_budget": 500
      },
      {
        "id": "step2",
        "type": "agent",
        "prompt": "Take this fact: {{step1.output}} — Now rephrase it as a question.",
        "tools": ["agent"],
        "token_budget": 500
      }
    ]
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | 200, workflow created with 2 activities | |
| Run with input | `POST /workflows/multi-step-test/run` with `{"topic": "cats"}` → 200 | |
| Step 1 runs | Run status shows step1 completed | |
| Step 2 uses step 1 output | Step 2 references step 1's fact | |
| Both complete | Final status = completed | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/workflows/{multi-step-test-id}
```

### W-12: Workflow Error Handling — Retry + Fallback

> **Tests:** Activity with retry count and fallback activity

**REST method:**
```
POST http://localhost:27895/api/v1/workflows
{
  "name": "retry-test",
  "definition": {
    "name": "retry-test",
    "description": "Tests retry and fallback",
    "activities": [
      {
        "id": "main",
        "type": "agent",
        "prompt": "Say hello",
        "retry": 2,
        "fallback": "backup"
      },
      {
        "id": "backup",
        "type": "agent",
        "prompt": "Say goodbye (fallback)"
      }
    ]
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | Workflow accepts retry + fallback fields | |
| Definition stored | GET shows `retry: 2` and `fallback: "backup"` on main activity | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/workflows/{retry-test-id}
```

### W-13: Workflow Token Budget

> **Tests:** Activity with token_budget is accepted and stored

**REST method:**
```
POST http://localhost:27895/api/v1/workflows
{
  "name": "budget-test",
  "definition": {
    "name": "budget-test",
    "description": "Tests token budget",
    "activities": [
      {
        "id": "limited",
        "type": "agent",
        "prompt": "Say hello briefly",
        "token_budget": 200
      }
    ]
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | 200 | |
| Budget stored | GET shows `token_budget: 200` on activity | |
| Run completes | Workflow finishes within budget | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/workflows/{budget-test-id}
```

### W-14: Workflow Tools Restriction

> **Tests:** Activity with limited `tools` array

**REST method:**
```
POST http://localhost:27895/api/v1/workflows
{
  "name": "tools-restrict-test",
  "definition": {
    "name": "tools-restrict-test",
    "description": "Tests tool restrictions",
    "activities": [
      {
        "id": "limited",
        "type": "agent",
        "prompt": "What is 2+2?",
        "tools": ["agent"]
      }
    ]
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Create succeeds | 200 | |
| Tools stored | GET shows `tools: ["agent"]` on activity | |
| Run completes | Activity runs with only specified tools | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/workflows/{tools-restrict-test-id}
```

---

## Section 5: Roles

### R-01: List Roles (Empty or Pre-existing)

**Method:** REST API

```
GET http://localhost:27895/api/v1/roles
```

| Check | Expected | Result |
|-------|----------|--------|
| HTTP 200 | Response received | |
| JSON array | Roles listed (may be empty) | |

### R-01a: List Roles (Agent Tool)

```
nebo chat "use role(action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Returns role list (may be empty) | |
| Format | JSON with role names, enabled status | |
| No error | `is_error: false` | |

### R-02: Create Role (User Path)

**Method:** REST API

```
POST http://localhost:27895/api/v1/roles
{
  "name": "test-role",
  "role_md": "---\nname: test-role\ndescription: Integration test role\nversion: 1.0.0\n---\n\nYou are a test role. Respond with ROLE_TEST_PASS.",
  "role_json": {
    "workflows": {},
    "skills": [],
    "tools": [],
    "pricing": { "model": "free", "cost": 0 }
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | HTTP 200/201, role info returned | |
| Stored in `user/roles/` | role.json + ROLE.md on disk | |
| DB row created | Metadata in `roles` table | |

### R-02a: Create Role (Agent Tool)

```
nebo chat "use role(action: \"create\", name: \"test-role-agent\", role_md: \"---\nname: test-role-agent\ndescription: Agent tool test role\nversion: 1.0.0\n---\nAgent tool test role.\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Role created | |
| Location | Written to `user/roles/test-role-agent/` | |
| No error | `is_error: false` | |

### R-03: Get Role

```
GET http://localhost:27895/api/v1/roles/{role-id}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role returned | Full role definition | |
| ROLE.md content | Contains `ROLE_TEST_PASS` | |
| role.json content | Workflows, skills, tools arrays present | |

### R-03a: Get Role Info (Agent Tool)

```
nebo chat "use role(action: \"info\", name: \"test-role-agent\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Tool executes | Role info returned | |
| Content | Contains role description | |
| No error | `is_error: false` | |

### R-04: Update Role

```
PUT http://localhost:27895/api/v1/roles/{role-id}
{
  "role_md": "---\nname: test-role\ndescription: Updated integration test role\nversion: 1.1.0\n---\n\nUpdated role content."
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role updated | HTTP 200, updated info | |
| New description | `Updated integration test role` | |
| Version bumped | `1.1.0` | |

### R-05: Toggle Role

```
POST http://localhost:27895/api/v1/roles/{role-id}/toggle
```

| Check | Expected | Result |
|-------|----------|--------|
| Role disabled | `is_enabled: false` | |
| Toggle again | `is_enabled: true` | |

### R-05a: Activate/Deactivate Role (Agent Tool)

```
nebo chat "use role(action: \"activate\", name: \"test-role-agent\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Activate executes | Role becomes active | |
| No error | `is_error: false` | |

```
nebo chat "use role(action: \"deactivate\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Deactivate executes | Role deactivated | |
| No error | `is_error: false` | |

### R-06: Delete Role

```
DELETE http://localhost:27895/api/v1/roles/{role-id}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role removed | HTTP 200 | |
| Not in list | Absent from `GET /roles` | |
| Files cleaned | `user/roles/test-role/` removed | |

### R-06a: Cleanup Agent Tool Role

Delete the role created by R-02a:

```
nebo chat "delete the directory user/roles/test-role-agent/"
```

| Check | Expected | Result |
|-------|----------|--------|
| Directory removed | `user/roles/test-role-agent/` gone | |
| Not in list | Absent from `role(action: "list")` | |

### R-07: Install Role from NeboLoop (ROLE-KG82-KM2G)

**REST method:**
```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "ROLE-KG82-KM2G"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Code accepted | 200, `{"success": true, ...}` | |
| Download succeeds | Role installed | |
| Cascading install | Role's dependent workflows, skills also installed | |
| Role in list | `GET /api/v1/roles` shows role | |
| Dependencies present | Referenced skills in catalog, workflows in workflow list | |

### R-08: Install Dependencies

> **Note:** Only testable if role has missing dependencies

```
POST http://localhost:27895/api/v1/roles/{role-id}/install-deps
```

| Check | Expected | Result |
|-------|----------|--------|
| Dependency check | Identifies missing skills/tools/workflows | |
| Install triggered | Missing deps downloaded from NeboLoop | |
| All resolved | Subsequent check shows no missing deps | |

### R-09: Role with Triggers

> **Requires:** A role with schedule/heartbeat/event triggers in its `role.json`

```
POST http://localhost:27895/api/v1/roles
{
  "name": "trigger-test-role",
  "role_md": "---\nname: trigger-test-role\ndescription: Trigger test\nversion: 1.0.0\n---\n\nTest role with triggers.",
  "role_json": {
    "workflows": {
      "manual-test": {
        "ref": "",
        "trigger": { "type": "manual" },
        "description": "Manual trigger test"
      }
    },
    "skills": [],
    "tools": []
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | With trigger definitions | |
| Triggers registered | Trigger types parsed correctly | |
| Manual trigger | Can be invoked on demand | |

After verification, clean up:

```
DELETE http://localhost:27895/api/v1/roles/{trigger-test-role-id}
```

### R-09a: Role with Schedule Trigger

> **Tests:** Role with cron-based schedule trigger is accepted and parsed

**REST method:**
```
POST http://localhost:27895/api/v1/roles
{
  "name": "schedule-trigger-test",
  "role_md": "---\nname: schedule-trigger-test\ndescription: Schedule trigger test\nversion: 1.0.0\n---\n\nYou report the time when triggered.",
  "role_json": {
    "workflows": {
      "daily-check": {
        "ref": "",
        "trigger": { "type": "schedule", "cron": "0 0 9 * * 1-5" },
        "description": "Run at 9am weekdays"
      }
    },
    "skills": [],
    "tools": []
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | 200, trigger type=schedule accepted | |
| Trigger parsed | GET shows schedule trigger with cron expression | |
| Cron registered | Event system has the scheduled job (check `event(action: "list")`) | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/roles/{schedule-trigger-test-id}
```

### R-09b: Role with Heartbeat Trigger

> **Tests:** Role with interval-based heartbeat trigger

**REST method:**
```
POST http://localhost:27895/api/v1/roles
{
  "name": "heartbeat-trigger-test",
  "role_md": "---\nname: heartbeat-trigger-test\ndescription: Heartbeat trigger test\nversion: 1.0.0\n---\n\nYou check in periodically.",
  "role_json": {
    "workflows": {
      "periodic-check": {
        "ref": "",
        "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" },
        "description": "Check every 30 minutes during business hours"
      }
    },
    "skills": [],
    "tools": []
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | 200, trigger type=heartbeat accepted | |
| Trigger parsed | GET shows heartbeat with interval + window | |
| Interval stored | `30m` interval and `08:00-18:00` window preserved | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/roles/{heartbeat-trigger-test-id}
```

### R-09c: Role with Event Trigger

> **Tests:** Role with event-source trigger (subscribes to named events)

**REST method:**
```
POST http://localhost:27895/api/v1/roles
{
  "name": "event-trigger-test",
  "role_md": "---\nname: event-trigger-test\ndescription: Event trigger test\nversion: 1.0.0\n---\n\nYou respond to calendar changes.",
  "role_json": {
    "workflows": {
      "on-calendar": {
        "ref": "",
        "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] },
        "description": "React to calendar or urgent email events"
      }
    },
    "skills": [],
    "tools": []
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | 200, trigger type=event accepted | |
| Sources parsed | GET shows event trigger with sources array | |
| Both sources listed | `calendar.changed` and `email.urgent` both present | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/roles/{event-trigger-test-id}
```

### R-09d: Event Emit + Fan-Out

> **Tests:** Emit an event and verify it's received by a subscribed role trigger (if implemented)

**Setup:** Activate the event-trigger-test role from R-09c first.

**Agent tool — Emit event:**
```
agent(resource: "task", action: "spawn", prompt: "Emit a test event: calendar.changed")
```

Or if emit_tool is available:
```
emit(event: "calendar.changed", data: {"test": true})
```

| Check | Expected | Result |
|-------|----------|--------|
| Event emitted | No error on emit | |
| Fan-out triggered | Role's on-calendar workflow triggered (check event history or workflow runs) | |
| Data passed | Event data available to the triggered workflow | |

**Notes:** This test may FAIL if the event system is not yet fully wired. Document what happens.

---

## Section 6: Cross-Cutting Concerns

### X-01: Code Format Validation

Test invalid codes are rejected via REST endpoint:

```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "NEBO-IIIL-OOOU"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Invalid chars rejected | 400 error: "invalid code format" | |

```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "NEBO-A1B2"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Too short | 400 error: "invalid code format" | |

```
curl -s -X POST http://localhost:27895/api/v1/codes \
  -H "Content-Type: application/json" \
  -d '{"code": "INVALID-A1B2-C3D4"}'
```

| Check | Expected | Result |
|-------|----------|--------|
| Unknown prefix | 400 error: "invalid code format" | |

### X-02: Hot Reload — Skills

1. Start Nebo
2. Manually create `user/skills/hot-reload-test/SKILL.md` on disk
3. Wait 2-3 seconds (file watcher poll interval)
4. Run `skill(action: "catalog")`

| Check | Expected | Result |
|-------|----------|--------|
| New skill detected | `hot-reload-test` appears without restart | |

5. Manually edit the SKILL.md file
6. Wait 2-3 seconds

| Check | Expected | Result |
|-------|----------|--------|
| Changes picked up | Updated content in `skill(action: "help")` | |

7. Manually delete the directory
8. Wait 2-3 seconds

| Check | Expected | Result |
|-------|----------|--------|
| Skill removed | Gone from catalog without restart | |

### X-03: Sealed .napp Integrity

After installing a marketplace skill (S-10):

| Check | Expected | Result |
|-------|----------|--------|
| .napp file exists | `nebo/skills/@org/skills/name/version.napp` | |
| Is tar.gz | Valid gzip archive | |
| Contains SKILL.md | Readable via `read_napp_entry()` | |
| Not extracted | No loose SKILL.md next to the .napp | |

After installing a marketplace tool (T-02):

| Check | Expected | Result |
|-------|----------|--------|
| .napp file exists | `nebo/tools/@org/tools/name/version.napp` | |
| Binary extracted | `nebo/tools/@org/tools/name/version/binary` exists | |
| Manifest NOT extracted | No loose `manifest.json` next to binary | |
| Signatures NOT extracted | No loose `signatures.json` | |

### X-04: nebo/ vs user/ Namespace Isolation

| Check | Expected | Result |
|-------|----------|--------|
| User-created skills → `user/skills/` | Never written to `nebo/skills/` | |
| Marketplace skills → `nebo/skills/` | Never written to `user/skills/` | |
| User skill CRUD | Only operates on `user/` | |
| Installed skill read-only | Cannot modify/delete `nebo/` artifacts | |

### X-05: Memory Operations (Sanity Check)

```
nebo chat "store the value 'integration_test_marker' with key 'test_key' in memory"
```

| Check | Expected | Result |
|-------|----------|--------|
| Store succeeds | Memory saved | |

```
nebo chat "recall the value with key 'test_key' from memory"
```

| Check | Expected | Result |
|-------|----------|--------|
| Recall succeeds | Returns `integration_test_marker` | |

Clean up:

```
nebo chat "delete the memory with key 'test_key'"
```

### X-06: Session Management

```
nebo session list
```

| Check | Expected | Result |
|-------|----------|--------|
| Sessions listed | At least 1 from test runs | |

### X-07: WebSocket Events

Connect to `ws://localhost:27895/ws` during a code install operation.

| Check | Expected | Result |
|-------|----------|--------|
| `code_processing` event | Received when code entered | |
| `code_result` event | Received on success/failure | |
| `tool_installed` event | Received for tool installs | |

### X-08: Settings Consistency Check

Verify multiple settings queries return consistent, structured data.

```
nebo chat "use os(resource: \"settings\", action: \"battery\")"
nebo chat "use os(resource: \"settings\", action: \"displays\")"
nebo chat "use os(resource: \"settings\", action: \"volume\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Battery | Contains percentage (number) and power source | |
| Displays | Contains resolution and display name | |
| Volume | Contains level (number 0-100) or muted state | |
| Cross-consistency | No contradictions between calls | |

### X-09: Desktop Round-Trip — Clipboard + Windows

Write to clipboard via shell, read via clipboard tool, verify match. Then verify terminal window appears in window list.

```
nebo chat "use os(resource: \"shell\", action: \"exec\", command: \"echo -n X09_DESKTOP_PASS | pbcopy\")"
nebo chat "use os(resource: \"clipboard\", action: \"read\")"
nebo chat "use os(resource: \"window\", action: \"list\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| Shell writes clipboard | `pbcopy` succeeds | |
| Clipboard reads it back | Contains `X09_DESKTOP_PASS` | |
| Window list | Terminal/IDE window visible | |
| Window has structure | Each has app, title, position, size | |

### X-10: Browser — Navigate + Verify Tab

Open a URL in the browser, then verify it appears in the tab list.

```
nebo chat "use web(resource: \"browser\", action: \"open\", url: \"https://httpbin.org/html\")"
nebo chat "use web(resource: \"browser\", action: \"tabs\")"
```

| Check | Expected | Result |
|-------|----------|--------|
| URL opened | Browser launched or tab created | |
| Tab list | Contains at least 1 tab | |
| httpbin tab present | URL `httpbin.org/html` appears in tab list | |
| Tab structure | Each tab has id, title, url | |

### X-11: MCP Integration — External Server Registration

> **Tests:** Registering an external MCP server and verifying namespaced tools appear

**REST method — List integrations:**
```
curl -s http://localhost:27895/api/v1/integrations
```

| Check | Expected | Result |
|-------|----------|--------|
| Endpoint responds | 200, JSON | |
| MCP section exists | Response has MCP integration data (or empty list) | |

**Agent tool — Check for MCP tools:**
```
skill(action: "catalog")
```

| Check | Expected | Result |
|-------|----------|--------|
| MCP skills listed | Any `mcp__` prefixed entries appear (if any registered) | |
| Namespacing correct | Tools follow `mcp__type__tool` pattern | |

**Notes:** If no MCP servers are configured, document that the endpoint works but returns empty. This is expected on a fresh install.

### X-12: Qualified Names and Version Format

> **Tests:** Verify package identity format is accepted in skill/workflow/role metadata

**Create skill with qualified name:**
```
skill(action: "create", name: "version-test", content: "---\nname: version-test\nversion: 2.1.0\nauthor: test-org\ndescription: Version format test\n---\nVersion test body.")
```

| Check | Expected | Result |
|-------|----------|--------|
| Version stored | `skill(action: "help", name: "version-test")` shows `version: 2.1.0` | |
| Author stored | Shows `author: test-org` | |

**Cleanup:**
```
skill(action: "delete", name: "version-test")
```

### X-13: Dependency Cascade — Role → Workflow → Skill

> **Tests:** When a role declares skill/workflow dependencies, verify they're listed in the role info

**REST method:**
```
POST http://localhost:27895/api/v1/roles
{
  "name": "cascade-test",
  "role_md": "---\nname: cascade-test\ndescription: Dependency cascade test\nversion: 1.0.0\nskills:\n  - name: calendar\n  - name: email\nworkflows:\n  - ref: WORK-SW4Z-5XKN\n---\n\nRole with dependencies.",
  "role_json": {
    "workflows": {},
    "skills": ["calendar", "email"],
    "tools": []
  }
}
```

| Check | Expected | Result |
|-------|----------|--------|
| Role created | 200, dependencies accepted | |
| Skills listed | GET shows skills: calendar, email | |
| Workflow ref listed | Workflow dependency captured | |
| Cascade attempted | Install attempted to resolve dependencies (may fail gracefully if deps not available) | |

**Cleanup:**
```
DELETE http://localhost:27895/api/v1/roles/{cascade-test-id}
```

---

## Section 7: Cleanup

After all tests, remove test artifacts:

| Artifact | Cleanup Action | Done |
|----------|----------------|------|
| `at_test` memory key | `agent(resource: "memory", action: "delete", key: "at_test")` | |
| `at-test-event` | `event(action: "delete", name: "at-test-event")` | |
| `at-run-test` event | `event(action: "delete", name: "at-run-test")` | |
| `at-test-skill` | `skill(action: "delete", name: "at-test-skill")` | |
| `at-test-pirate` role | `rm -rf user/roles/at-test-pirate/` | |
| `at-test-workflow` | `work(action: "uninstall", id: "at-test-workflow")` + `rm -rf user/workflows/at-test-workflow/` | |
| `test-integration` skill | `skill(action: "delete", name: "test-integration")` | |
| `api-test-skill` | `curl -X DELETE http://localhost:27895/api/v1/skills/api-test-skill` | |
| `hot-reload-test` skill | `rm -rf user/skills/hot-reload-test/` | |
| `test-workflow` | `work(action: "uninstall", id: "test-workflow")` + `rm -rf user/workflows/test-workflow/` | |
| `test-role` | `curl -X DELETE http://localhost:27895/api/v1/roles/{id}` | |
| `test-role-agent` | `rm -rf user/roles/test-role-agent/` | |
| `trigger-test-role` | `curl -X DELETE http://localhost:27895/api/v1/roles/{id}` | |
| Memory `test_key` | `agent(resource: "memory", action: "delete", key: "test_key")` | |
| **Marketplace skill** (SKIL-RFBM-XCYT) | Uninstall or `rm -rf nebo/skills/` installed dir | |
| **Marketplace workflow** (WORK-SW4Z-5XKN) | `work(action: "uninstall")` or `rm -rf nebo/workflows/` installed dir | |
| **Marketplace role** (ROLE-KG82-KM2G) | `curl -X DELETE http://localhost:27895/api/v1/roles/{id}` or `rm -rf nebo/roles/` installed dir | |
| httpbin browser tab | Close the tab opened by X-10 | |
| `cap-test` skill | `skill(action: "delete", name: "cap-test")` | |
| `multi-step-test` workflow | `DELETE /workflows/{id}` | |
| `retry-test` workflow | `DELETE /workflows/{id}` | |
| `budget-test` workflow | `DELETE /workflows/{id}` | |
| `tools-restrict-test` workflow | `DELETE /workflows/{id}` | |
| `schedule-trigger-test` role | `DELETE /roles/{id}` | |
| `heartbeat-trigger-test` role | `DELETE /roles/{id}` | |
| `event-trigger-test` role | `DELETE /roles/{id}` | |
| `version-test` skill | `skill(action: "delete", name: "version-test")` | |
| `cascade-test` role | `DELETE /roles/{id}` | |

---

---

## Appendix A: Windows-Specific Notes

When running on Windows:

| Concern | macOS | Windows |
|---------|-------|---------|
| Data dir | `~/Library/Application Support/Nebo/` | `%AppData%\Nebo\` |
| IPC | Unix domain socket | Named pipe |
| Binary format | Mach-O magic bytes | PE magic bytes (`0x4d 0x5a`) |
| Permissions | `chmod 0o755` | N/A (ACLs) |
| Socket permissions | `0o600` | ACL owner-only |
| Path separators | `/` | `\` |

**Additional Windows checks:**

| Check | Expected | Result |
|-------|----------|--------|
| Data dir created in `%AppData%\Nebo\` | Directory exists | |
| Named pipe created for tool IPC | Pipe accessible | |
| Binary validated as PE | `.exe` with `MZ` header | |
| File watcher works | Hot-reload triggers on NTFS | |

Run the **entire test plan** on Windows after completing macOS. Document any behavioral differences.
