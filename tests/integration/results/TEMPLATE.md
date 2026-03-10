# Integration Test Results

**Run ID:** `___`
**Date:** ___
**Platform:** ___ (e.g., macOS 15.4 arm64 / Windows 11 x64)
**Nebo Version:** ___ (git commit hash)
**Tester:** ___
**NeboLoop Codes Available:** SKILL: SKIL-RFBM-XCYT / WORK: WORK-SW4Z-5XKN / ROLE: ROLE-KG82-KM2G

---

## Pre-Flight Checks (PF)

### PF-01: Doctor Check

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Data dir exists | Path printed | | |
| Setup complete | `true` | | |
| Bot ID | UUID present | | |
| Database | `OK` | | |
| Skills dir | Path printed, count >= 0 | | |

### PF-02: Server Starts

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Server binds | `Listening on 127.0.0.1:27895` | | |
| No panic/crash | Process stays alive | | |

### PF-03: Directory Structure

| Path | Expected | Result | Notes |
|------|----------|--------|-------|
| `<data_dir>/nebo/skills/` | Directory exists | | |
| `<data_dir>/nebo/tools/` | Directory exists | | |
| `<data_dir>/nebo/workflows/` | Directory exists | | |
| `<data_dir>/nebo/roles/` | Directory exists | | |
| `<data_dir>/user/skills/` | Directory exists | | |
| `<data_dir>/user/tools/` | Directory exists | | |
| `<data_dir>/user/workflows/` | Directory exists | | |
| `<data_dir>/user/roles/` | Directory exists | | |
| `<data_dir>/data/nebo.db` | File exists | | |

### PF-04: Providers Ready

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| At least 1 provider listed | Provider name + status shown | | |

### PF-05: Interactive Chat Responds

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Agent responds | Non-empty text response | | |
| No tool errors | Clean response | | |

---

## Section 1: Agent Built-in Tools (AT)

### AT-01: os (file) — Write + Read Round-Trip

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Write completes | File created at `/tmp/nebo-at01-test.txt` | | |
| Read returns exact content | Output contains `AT01_ROUND_TRIP_PASS` | | |
| Cleanup | File deleted | | |

### AT-02: os (shell) — Piped Command + Exit Code

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Piped output correct | Output is `AT02_PASS` | | |
| Exit code 0 | No error | | |

### AT-03: os (app) — Verify Known App Running

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| App list returned | Non-empty array | | |
| Finder present | Always running on macOS | | |
| Structure | Each entry has app name | | |

### AT-04: os (settings) — Verify Battery Data

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Percentage present | Number 0-100 | | |
| Power source | AC/battery/charging shown | | |

### AT-05: os (music) — Graceful When Nothing Playing

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| No crash | `is_error: false` | | |
| Meaningful output | Track info or "Not playing" | | |

### AT-06: os (search) — Find the Database File

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Results returned | At least 1 result | | |
| Correct path | Contains Nebo data directory | | |

### AT-07: os (window) — Verify Window Structure

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Windows listed | At least 1 window | | |
| Structure | App name + title per window | | |
| Position data | x, y, width, height present | | |

### AT-08: os (clipboard) — Write + Read Round-Trip

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Write completes | Clipboard updated | | |
| Read returns exact text | Contains `AT08_CLIPBOARD_PASS` | | |

### AT-09: os (keychain) — Store + Find + Delete Credential

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Store completes | Credential saved | | |
| Find returns it | Contains `nebo-at09-test` | | |
| Delete completes | Credential removed | | |
| Find after delete | Returns "not found" | | |

### AT-10: os (mail) — Verify Unread Count

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Count returned | Contains a number | | |
| Format | Mailboxes or total count | | |

### AT-11: os (calendar) — Verify Date Awareness

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Today's date | References today or "no events" | | |
| Format | If events: title, time, calendar | | |

### AT-12: web (http) — Fetch + Verify JSON

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| HTTP 200 | Response received | | |
| JSON body | Contains `origin` field | | |
| Headers | Contains `Host: httpbin.org` | | |

### AT-13: web (search) — Verify Results Have URLs

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Results returned | At least 1 result | | |
| Title present | Non-empty title string | | |
| URL present | Valid URL starting with `http` | | |
| Snippet present | Description text | | |

### AT-14: agent (memory) — Full Lifecycle

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Store V1 | Memory saved | | |
| Recall V1 | Returns exactly `AT_PASS_V1` | | |
| Update to V2 | Overwrite succeeds | | |
| Recall V2 | Returns `AT_PASS_V2` (not V1) | | |
| Delete | Memory removed | | |
| Recall after delete | "not found" or empty | | |

### AT-15: agent (session) — Verify Current Session

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| At least 1 session | Current test session visible | | |
| Session has ID | UUID or integer | | |
| Message count | At least 1 message | | |

### AT-16: agent (context) — Verify Summary Content

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Session info | Session ID present | | |
| Message count | Number shown | | |

### AT-17: event — Full CRUD Cycle

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Create succeeds | Event created with ID | | |
| List shows event | `at-test-event` with schedule | | |
| Delete succeeds | Event removed | | |
| List after delete | Event is gone | | |

### AT-18: event — Run a One-Shot Task

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Create succeeds | Event created | | |
| Manual run triggers | Task executes | | |
| Output meaningful | Related to prompt | | |
| Cleanup | Event deleted | | |

### AT-19: skill — Create + Trigger + Delete

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill created | In `user/skills/at-test-skill/` | | |
| In catalog | Listed, enabled, source: user | | |
| Trigger fires | "at nineteen test" activates skill | | |
| Response influenced | Contains skill content | | |
| Delete succeeds | Skill removed | | |
| Gone from catalog | Absent after delete | | |

### AT-20: role — Verify Persona Activation

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role created | In `user/roles/at-test-pirate/` | | |
| Activate succeeds | Confirmation message | | |
| Persona active | Response contains pirate-speak | | |
| Deactivate succeeds | Reverted to default | | |
| Persona gone | Normal response (no pirate) | | |

### AT-21: role — List Shows Created Roles

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| List includes role | `at-test-pirate` present | | |
| Info returns details | "Pirate persona" in description | | |
| Version shown | `1.0.0` | | |

### AT-22: work — Create + Run + Verify Output + Delete

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Workflow created | In `user/workflows/` | | |
| Run starts | Returns run_id | | |
| Status shows result | completed/failed with output | | |
| Output meaningful | Related to prompt | | |
| Delete succeeds | Removed from list | | |

### AT-23: message — Toggle DND + Verify State Change

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Initial status | Returns current DND state | | |
| DND on | Status changes to enabled | | |
| DND off | Status changes to disabled | | |
| State persists | Each check reflects toggle | | |

---

## Section 2: Skills (S)

### S-01: Catalog — List Skills

**Agent:**

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Returns skill list | | |
| Format | JSON with names, enabled, source | | |
| No error | `is_error: false` | | |

**REST API:** `GET /api/v1/extensions`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| HTTP 200 | Response received | | |
| JSON array | Skills listed | | |

### S-02: Create Skill

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Skill created successfully | | |
| Location | `user/skills/test-integration/SKILL.md` | | |
| File exists | SKILL.md present on disk | | |
| Content matches | Frontmatter + template intact | | |

### S-03: Verify Skill in Catalog

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| `test-integration` listed | Present in catalog | | |
| Enabled | `enabled: true` | | |
| Source | `user` | | |
| Description | `Integration test skill` | | |

### S-04: Get Skill Help/Content

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Returns content | Full SKILL.md body | | |
| Template present | Contains `SKILL_TEST_PASS` | | |
| Frontmatter parsed | Name, description, triggers | | |

### S-05: Trigger Matching

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill activated | Trigger phrase matches | | |
| Response references skill | Contains skill-influenced content | | |

### S-06: Unload Skill

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill disabled | Success message | | |
| Catalog shows disabled | `enabled: false` | | |

### S-07: Load Skill

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill re-enabled | Success message | | |
| Catalog shows enabled | `enabled: true` | | |

### S-08: Update Skill

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill updated | Success message | | |
| New description | `Updated integration test` | | |
| File on disk updated | SKILL.md reflects changes | | |

### S-09: Delete Skill

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Skill removed | Success message | | |
| Not in catalog | Absent | | |
| File removed | Directory gone | | |

### S-10: Install Skill from NeboLoop (`SKIL-RFBM-XCYT`)

**Method:** `POST /api/v1/codes {"code": "SKIL-RFBM-XCYT"}`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Code accepted | 200, success: true | | |
| Download succeeds | Skill installed | | |
| Installed to `nebo/skills/` | Sealed .napp present | | |
| Appears in catalog | Source: `nebo` | | |
| Content readable | SKILL.md from sealed archive | | |

### S-11: Installed Skill is Read-Only

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Delete blocked | Error returned | | |
| Skill still present | Remains in catalog | | |

### S-12: REST API — Skill CRUD

| Operation | Endpoint | Expected | Result | Notes |
|-----------|----------|----------|--------|-------|
| Create | `POST /skills` | 200/201 | | |
| Read | `GET /skills/api-test-skill` | 200 | | |
| Update | `PUT /skills/api-test-skill` | 200 | | |
| Toggle off | `POST /skills/api-test-skill/toggle` | disabled | | |
| Toggle on | `POST /skills/api-test-skill/toggle` | enabled | | |
| Delete | `DELETE /skills/api-test-skill` | 200 | | |
| Verify gone | `GET /skills/api-test-skill` | 404 | | |

---

## Section 3: Built-in Tools (T)

### T-01: List Built-in Tools

**REST API:** `GET /integrations/tools`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| HTTP 200 | Response received | | |
| JSON array | Built-in tools listed | | |
| Contains `os` | Present | | |
| Contains `web` | Present | | |
| Contains `agent` | Present | | |
| Contains `skill` | Present | | |
| Contains `role` | Present | | |
| Contains `work` | Present | | |
| Contains `event` | Present | | |
| Contains `message` | Present | | |
| Contains `execute` | Present | | |
| Contains `loop` | Present | | |
| Each has name | Non-empty | | |
| Each has description | Non-empty | | |

### T-02: Verify Tool Schema

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Schema present | Each tool has JSON schema | | |
| Properties defined | Has `action` in properties | | |
| Type is object | `"type": "object"` | | |

---

## Section 4: Workflows (W)

### W-01: List Workflows

**Agent:**

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Returns workflow list | | |
| Format | JSON with id, name, version | | |

**REST API:** `GET /api/v1/workflows`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| HTTP 200 | Response received | | |
| JSON array | Workflows listed | | |

### W-02: Create Workflow

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Workflow created | Success + ID | | |
| Stored in `user/workflows/` | workflow.json on disk | | |
| DB row created | Metadata persisted | | |

### W-03: Verify Workflow in List

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| `test-workflow` listed | Present | | |
| Status | Enabled | | |
| Definition intact | Name, description match | | |

### W-04: Run Workflow

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Run started | Returns run_id | | |
| Status transitions | pending → running → completed/failed | | |
| Result captured | Output stored | | |

### W-05: Check Workflow Run Status

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Status returned | Latest run status | | |
| Run ID matches | Same from W-04 | | |
| Final status | completed / failed | | |

### W-06: List Workflow Runs

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Run history | At least 1 run | | |
| Run details | id, status, timestamps | | |

### W-07: Toggle Workflow

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Workflow disabled | Status changed | | |
| Toggle back | Re-enabled | | |

### W-08: Delete Workflow

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Workflow removed | Success | | |
| Not in list | Absent | | |
| Files cleaned | Directory removed | | |

### W-09: Install Workflow from NeboLoop (`WORK-SW4Z-5XKN`)

**Method:** `POST /api/v1/codes {"code": "WORK-SW4Z-5XKN"}`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Code accepted | 200, success: true | | |
| Download succeeds | Workflow installed | | |
| Installed to `nebo/workflows/` | Sealed .napp | | |
| Appears in list | Present | | |
| Definition readable | From sealed archive | | |

### W-10: REST API — Workflow CRUD

| Operation | Endpoint | Expected | Result | Notes |
|-----------|----------|----------|--------|-------|
| List | `GET /workflows` | 200 | | |
| Create | `POST /workflows` | 200/201 | | |
| Get | `GET /workflows/{id}` | 200 | | |
| Update | `PUT /workflows/{id}` | 200 | | |
| Run | `POST /workflows/{id}/run` | 200 | | |
| Toggle | `POST /workflows/{id}/toggle` | 200 | | |
| Delete | `DELETE /workflows/{id}` | 200 | | |
| List runs | `GET /workflows/{id}/runs` | 200 | | |

---

## Section 5: Roles (R)

### R-01: List Roles (REST)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| HTTP 200 | Response received | | |
| JSON array | Roles listed | | |

### R-01a: List Roles (Agent Tool)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Role list returned | | |
| Format | JSON with names, enabled | | |
| No error | `is_error: false` | | |

### R-02: Create Role (REST)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role created | 200/201 | | |
| Stored in `user/roles/` | Files on disk | | |
| DB row created | Metadata persisted | | |

### R-02a: Create Role (Agent Tool)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Role created | | |
| Location | `user/roles/test-role-agent/` | | |
| No error | `is_error: false` | | |

### R-03: Get Role (REST)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role returned | Full definition | | |
| ROLE.md content | Contains `ROLE_TEST_PASS` | | |
| role.json content | Workflows, skills, tools present | | |

### R-03a: Get Role Info (Agent Tool)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Tool executes | Role info returned | | |
| Content | Contains role description | | |
| No error | `is_error: false` | | |

### R-04: Update Role

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role updated | 200 | | |
| New description | `Updated integration test role` | | |
| Version bumped | `1.1.0` | | |

### R-05: Toggle Role

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role disabled | `is_enabled: false` | | |
| Toggle again | `is_enabled: true` | | |

### R-05a: Activate/Deactivate Role (Agent Tool)

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Activate executes | Role active | | |
| Deactivate executes | Role deactivated | | |
| No error | `is_error: false` | | |

### R-06: Delete Role

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role removed | 200 | | |
| Not in list | Absent | | |
| Files cleaned | Directory removed | | |

### R-06a: Cleanup Agent Tool Role

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Directory removed | `user/roles/test-role-agent/` gone | | |
| Not in list | Absent | | |

### R-07: Install Role from NeboLoop (`ROLE-KG82-KM2G`)

**Method:** `POST /api/v1/codes {"code": "ROLE-KG82-KM2G"}`

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Code accepted | 200, success: true | | |
| Download succeeds | Role installed | | |
| Cascading install | Dependencies also installed | | |
| Role in list | Present | | |
| Dependencies present | Skills, workflows | | |

### R-08: Install Dependencies

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Dependency check | Missing identified | | |
| Install triggered | Deps downloaded | | |
| All resolved | No missing deps | | |

### R-09: Role with Triggers

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Role created | With trigger definitions | | |
| Triggers registered | Parsed correctly | | |
| Manual trigger | Invocable on demand | | |

---

## Section 6: Cross-Cutting (X)

### X-01: Code Format Validation

**Method:** `POST /api/v1/codes` with invalid codes

| Input | Expected | Result | Notes |
|-------|----------|--------|-------|
| `NEBO-IIIL-OOOU` | 400 "invalid code format" | | |
| `NEBO-A1B2` | 400 "invalid code format" | | |
| `INVALID-A1B2-C3D4` | 400 "invalid code format" | | |

### X-02: Hot Reload — Skills

| Step | Check | Expected | Result | Notes |
|------|-------|----------|--------|-------|
| Create file on disk | Detected | Appears in catalog | | |
| Edit file on disk | Updated | New content in help | | |
| Delete directory | Removed | Gone from catalog | | |

### X-03: Sealed .napp Integrity

**After S-10 (skill install):**

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| .napp file exists | In `nebo/skills/` | | |
| Is valid tar.gz | Archive readable | | |
| Contains SKILL.md | Entry present | | |
| Not extracted | No loose SKILL.md | | |

**After W-09 or R-07 (marketplace install):**

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| .napp file exists | In `nebo/` subdir | | |
| Sealed archive | Not extracted to loose files | | |

### X-04: nebo/ vs user/ Namespace Isolation

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| User skills → `user/skills/` | Correct namespace | | |
| Marketplace skills → `nebo/skills/` | Correct namespace | | |
| User CRUD → `user/` only | Never touches `nebo/` | | |
| Installed artifacts read-only | Cannot modify/delete | | |

### X-05: Memory Operations

| Operation | Expected | Result | Notes |
|-----------|----------|--------|-------|
| Store | Memory saved | | |
| Recall | Correct value returned | | |
| Delete | Memory removed | | |

### X-06: Session Management

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Sessions listed | At least 1 from test runs | | |

### X-07: WebSocket Events

| Event | Expected | Result | Notes |
|-------|----------|--------|-------|
| `code_processing` | Received on code entry | | |
| `code_result` | Received on completion | | |
| `tool_installed` | Received for tool install | | |

### X-08: Settings Consistency Check

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Battery | Percentage + power source | | |
| Displays | Resolution + display name | | |
| Volume | Level 0-100 or muted state | | |
| Cross-consistency | No contradictions | | |

### X-09: Desktop Round-Trip — Clipboard + Windows

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| Shell writes clipboard | `pbcopy` succeeds | | |
| Clipboard reads back | Contains `X09_DESKTOP_PASS` | | |
| Window list | Terminal/IDE visible | | |
| Window structure | App, title, position, size | | |

### X-10: Browser — Navigate + Verify Tab

| Check | Expected | Result | Notes |
|-------|----------|--------|-------|
| URL opened | Browser tab created | | |
| Tab list | At least 1 tab | | |
| httpbin tab present | `httpbin.org/html` in list | | |
| Tab structure | id, title, url per tab | | |

---

## Section 7: Cleanup

| Artifact | Cleanup Action | Done | Notes |
|----------|----------------|------|-------|
| `at_test` memory key | Delete via agent tool | | |
| `at-test-event` | Delete via event tool | | |
| `at-run-test` event | Delete via event tool | | |
| `at-test-skill` | Delete via skill tool | | |
| `at-test-pirate` role | `rm -rf user/roles/at-test-pirate/` | | |
| `at-test-workflow` | Uninstall + rm files | | |
| `test-integration` skill | Delete via skill tool (S-09) | | |
| `api-test-skill` | DELETE REST API (S-12) | | |
| `hot-reload-test` skill | `rm -rf user/skills/hot-reload-test/` | | |
| `test-workflow` | Uninstall + rm files (W-08) | | |
| `test-role` | DELETE REST API (R-06) | | |
| `test-role-agent` | `rm -rf user/roles/test-role-agent/` (R-06a) | | |
| `trigger-test-role` | DELETE REST API (R-09) | | |
| Memory `test_key` | Delete via agent tool (X-05) | | |
| **SKIL-RFBM-XCYT** skill | Uninstall installed skill from `nebo/skills/` | | |
| **WORK-SW4Z-5XKN** workflow | Uninstall installed workflow from `nebo/workflows/` | | |
| **ROLE-KG82-KM2G** role | Uninstall installed role from `nebo/roles/` | | |
| httpbin browser tab | Close tab (X-10) | | |

---

## Summary

| Section | Total | Pass | Fail | Skip | Notes |
|---------|-------|------|------|------|-------|
| Pre-Flight (PF) | 5 | | | | |
| Agent Tools (AT) | 23 | | | | |
| Skills (S) | 12 | | | | |
| Built-in Tools (T) | 2 | | | | |
| Workflows (W) | 10 | | | | |
| Roles (R) | 14 | | | | |
| Cross-Cutting (X) | 10 | | | | |
| **Total** | **76** | | | | |

## Regressions from Previous Run

> Compare against the previous results file. List any tests that previously passed but now fail.

| Test ID | Previous | Current | Description |
|---------|----------|---------|-------------|
| | | | |

## New Failures

> Tests that failed for the first time in this run.

| Test ID | Error Details |
|---------|---------------|
| | |

## Raw Output

> Attach or link to full CLI/API output captured during the run.

```
[paste or link here]
```
