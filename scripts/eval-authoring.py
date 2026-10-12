#!/usr/bin/env python3
"""The authoring eval: owner requests -> the workflows Nebo's authoring
guidance produces -> a score per request, per model.

  make eval-authoring                         # MODELS=claude:haiku by default
  make eval-authoring MODELS=janus:nebo-1-flash,janus:nebo-1-medium,janus:nebo-1-pro
  make eval-authoring ONLY=crm-sync,invoice-run
  make eval-authoring SCORE=<run dir>         # re-score a run without generating
  make eval-authoring SURFACES=<dir>          # generate from surfaces exported elsewhere (a baseline)
  make eval-authoring TRACK=run               # the run track only (can a model RUN each step type)

Authoring track:
1. The surfaces (create_workflow, update_workflow, create_employee,
   update_employee) are exported from the Rust tool definitions, exactly as
   the model is shown them.
2. Each request in fixtures/authoring/requests.json goes to each model with
   the bot's context (plugins, MCP servers, coworkers).
   - janus:<model> (OpenAI-compatible, JANUS_URL + JANUS_TOKEN): the surfaces
     are real tools; the model calls them over rounds, and a save comes back
     created, drafted or refused by the save path's validator, as in Nebo.
   - claude:<alias> (the claude CLI): the surfaces are listed and the model
     answers with its calls as JSON; a refused save goes back to it once.
3. crates/workflow/tests/authoring_eval.rs validates every workflow the
   calls create and checks the design rules; report.md per model, and
   matrix.md with each model's pass rate per rule.

Run track (fixtures/authoring/steps.json, janus models): fixed steps of
each type an engine runs, a lean `tools: []` judge (one call, JSON answer)
and a multi-round tool-calling step against mock tools, scored on the
answer and the calls.

The guidance under test is only what the exported surfaces say: this
script adds the bot's facts, never rules.
"""

import argparse
import concurrent.futures
import datetime
import json
import os
import pathlib
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
REQUESTS = ROOT / "fixtures" / "authoring" / "requests.json"

FRAME = """You are {employee}, an AI employee in Nebo, working for your owner. You act only by calling tools.

What this bot has:
- Installed plugins: {plugins}. A plugin's tool is plugin__<slug>; its skill is named <slug>, and its program is ${{plugin.<SLUG>_BIN}} in a command step (slug upper-cased, '-' as '_').
- Connected MCP servers: {mcp}.
- Built-in tools: recall (memory), read_file, read_session (past chats), app_data, use_skill, message_owner, send_message, run_command, http_request, search_web, find_tools, find_plugins.
- Coworkers and other experts you know of (list_employees, the store): {experts}.

Your tools for this (name, description, input JSON schema):
{tools}

Answer with ONLY one JSON object, no prose around it:
{{"calls": [{{"tool": "<tool name>", "args": {{...}}}}], "reply": "<what you tell the owner>"}}
Make every call needed to set this up completely now; the owner has already said yes to it.

The owner says: {request}"""


def export_surfaces(out: pathlib.Path) -> list:
    surf = out / "surfaces"
    surf.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, AUTHORING_EXPORT=str(surf))
    subprocess.run(
        ["cargo", "test", "-q", "-p", "nebo-tools", "--lib", "export_authoring_surfaces", "--", "--ignored"],
        cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL,
    )
    return [json.loads((surf / f"{n}.json").read_text())
            for n in ("create_workflow", "update_workflow", "create_employee", "update_employee")]


def describe_experts(experts: list) -> str:
    if not experts:
        return "none"
    kinds = {"coworker": "coworker on this bot", "shared": "shared expert on another owner's bot", "store": "store employee, not hired"}
    return "; ".join(f"{e['name']} ({kinds.get(e['kind'], e['kind'])}): {e['about']}" for e in experts)


def prompt_for(req: dict, surfaces: list) -> str:
    mcp = "; ".join(f"{m['server']} (tools: {', '.join(m['tools'])})" for m in req.get("mcp", [])) or "none"
    return FRAME.format(
        employee=req["employee"],
        plugins=", ".join(req.get("plugins", [])) or "none",
        mcp=mcp,
        experts=describe_experts(req.get("experts", [])),
        tools=json.dumps(surfaces, indent=1),
        request=req["request"],
    )


def first_json_object(text: str):
    text = re.sub(r"^```(?:json)?\s*|\s*```$", "", text.strip())
    start = text.find("{")
    dec = json.JSONDecoder()
    while start != -1:
        try:
            obj, _ = dec.raw_decode(text[start:])
            return obj
        except json.JSONDecodeError:
            start = text.find("{", start + 1)
    return None


def as_json(v):
    if isinstance(v, str):
        try:
            return json.loads(v)
        except json.JSONDecodeError:
            return None
    return v


def automation_def(auto: dict) -> dict:
    """An automation as the workflow it becomes (agent_tool's
    build_agent_json_from_automations): steps are one activity."""
    trigger = {"type": "manual"}
    if auto.get("schedule"):
        trigger = {"type": "schedule", "cron": auto["schedule"]}
    elif auto.get("interval"):
        trigger = {"type": "heartbeat", "interval": auto["interval"]}
    elif auto.get("sources"):
        trigger = {"type": "event", "sources": auto["sources"]}
    if isinstance(auto.get("activities"), list):
        acts = auto["activities"]
    else:
        run = {"id": "run", "intent": auto.get("description") or auto.get("name", "run"), "steps": auto.get("steps", [])}
        if isinstance(auto.get("tools"), list):
            run["tools"] = auto["tools"]
        acts = [run]
    d = {"trigger": trigger, "activities": acts}
    if isinstance(auto.get("connections"), list) and auto["connections"]:
        d["connections"] = auto["connections"]
    return d


def extract(calls: list):
    defs, ceilings = [], []
    for c in calls or []:
        tool, args = c.get("tool", ""), c.get("args") or {}
        if tool in ("create_workflow", "update_workflow"):
            d = as_json(args.get("definition"))
            if isinstance(d, dict):
                defs.append({"name": args.get("name") or d.get("name") or args.get("workflow") or "workflow", "def": d})
        elif tool in ("create_employee", "update_employee"):
            for key in ("automations", "add_automations"):
                for a in args.get(key) or []:
                    if isinstance(a, dict):
                        defs.append({"name": a.get("name", "automation"), "def": automation_def(a)})
            if isinstance(args.get("update_automation"), dict):
                a = args["update_automation"]
                defs.append({"name": a.get("name", "automation"), "def": automation_def(a)})
            aj = as_json(args.get("agent_json"))
            if isinstance(aj, dict):
                for name, wf in (aj.get("workflows") or {}).items():
                    if isinstance(wf, dict):
                        defs.append({"name": name, "def": wf})
                if isinstance(aj.get("ceiling"), dict):
                    ceilings += list(aj["ceiling"].keys())
            if isinstance(args.get("ceiling"), dict):
                ceilings += list(args["ceiling"].keys())
    return defs, ceilings


def ask(prompt: str, model: str) -> str:
    for _ in range(3):  # retries for an empty or timed-out answer
        try:
            proc = subprocess.run(
                ["claude", "-p", "--model", model, "--tools", "", "--setting-sources", "", "--no-session-persistence", "--strict-mcp-config",
                 "--system-prompt", "You are the model behind an AI employee. Follow the user turn exactly."],
                input=prompt, capture_output=True, text=True, timeout=600,
            )
            if proc.stdout.strip():
                return proc.stdout
        except subprocess.TimeoutExpired:
            pass
    return ""


JANUS_URL = os.environ.get("JANUS_URL", "https://janus.neboloop.com/v1")
_checker = None


def checker() -> str:
    """The authoring_eval test binary, built once (calling it directly keeps
    parallel requests off cargo's build lock)."""
    global _checker
    if _checker is None:
        out = subprocess.run(
            ["cargo", "test", "-q", "-p", "nebo-workflow", "--test", "authoring_eval", "--no-run", "--message-format=json"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout
        for line in out.splitlines():
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == "authoring_eval" and m.get("executable"):
                _checker = m["executable"]
    return _checker


def check_defs(defs: list, work: pathlib.Path) -> list:
    """The save path's validator errors (napp::workflow_check) per definition."""
    if not defs:
        return []
    f = work / f"check-{os.getpid()}-{threading.get_ident()}-{time.monotonic_ns()}.json"
    f.write_text(json.dumps(defs))
    subprocess.run([checker(), "authoring_eval_check", "--ignored", "--exact"], env=dict(os.environ, AUTHORING_CHECK=str(f)),
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out = pathlib.Path(f"{f}.out")
    errors = json.loads(out.read_text()) if out.exists() else [None] * len(defs)
    f.unlink(missing_ok=True)
    out.unlink(missing_ok=True)
    return errors


def janus(model: str, messages: list, tools=None, timeout=600) -> dict:
    body = {"model": model, "messages": messages}
    if tools:
        body["tools"] = tools
    for attempt in range(4):
        req = urllib.request.Request(
            f"{JANUS_URL}/chat/completions", data=json.dumps(body).encode(),
            headers={"Authorization": f"Bearer {os.environ['JANUS_TOKEN']}", "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.load(r)
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as e:
            if attempt == 3:
                return {"error": str(e)}
            time.sleep(5 * (attempt + 1))
    return {}


DISCOVERY = [
    {"name": "find_tools", "description": "Search the tools this bot has, by words.", "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}},
    {"name": "find_plugins", "description": "List the plugins installed on this bot.", "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}},
    {"name": "list_employees", "description": "The employees on this bot: name and what each does.", "input_schema": {"type": "object", "properties": {}}},
    {"name": "use_skill", "description": "Read a skill (a plugin's skill is named after its slug).", "input_schema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
]


def lookup_answer(tool: str, args: dict, req: dict) -> str:
    """The bot's facts through Nebo's own discovery tools, as Nebo would answer."""
    plugins = req.get("plugins", [])
    if tool == "find_plugins":
        return json.dumps({"installed": [{"slug": p, "tool": f"plugin__{p}", "program": "${plugin." + p.upper().replace('-', '_') + "_BIN}"} for p in plugins]})
    if tool == "find_tools":
        mcp = [t for m in req.get("mcp", []) for t in m["tools"]]
        return json.dumps({"tools": [f"plugin__{p}" for p in plugins] + mcp + ["recall", "read_file", "read_session", "app_data", "message_owner", "send_message", "run_command", "http_request"]})
    if tool == "list_employees":
        return json.dumps({"employees": [{"name": e["name"], "about": e["about"]} for e in req.get("experts", []) if e["kind"] == "coworker"]})
    if tool == "use_skill":
        slug = str(args.get("name", ""))
        if slug in plugins:
            b = "${plugin." + slug.upper().replace('-', '_') + "_BIN}"
            return (f"Skill {slug}. Program: {b}. Every command prints JSON. Reads: {b} <resource> list [--since <date>] [--status <s>] --all --json; "
                    f"{b} <resource> get --ids <id,...> --json. Writes: {b} <resource> create|update --file <json file> --json. Tool: plugin__{slug} (same commands).")
        return f"error: no skill named {slug}"
    return ""


def tool_answer(tool: str, args: dict, work: pathlib.Path, req: dict) -> tuple:
    """What Nebo's tool answers, simulated: (content, refused)."""
    if tool in {t["name"] for t in DISCOVERY}:
        return lookup_answer(tool, args, req), False
    if tool in ("create_workflow", "update_workflow"):
        d = args.get("definition")
        if isinstance(d, str):
            try:
                d = json.loads(d)
            except json.JSONDecodeError as e:
                return f"error: definition is not valid JSON: {e}", True
        if not isinstance(d, dict):
            return "error: Give the workflow a `definition`.", True
        err = check_defs([{"name": args.get("name") or "workflow", "def": d}], work)[0]
        if err:
            return f"error: The workflow was not saved; fix these steps and send it again:\n{err}", True
        return json.dumps({"created" if tool == "create_workflow" else "updated": True, "workflow": {"name": args.get("name") or d.get("name")}}), False
    if tool in ("create_employee", "update_employee"):
        if set(args) <= {"draft_id"}:
            return json.dumps({"created": True}), False
        # The tool validates its input against its schema, then the duties'
        # workflows with the save path's validator (validated_frontmatter).
        for key in ("automations", "add_automations"):
            if key in args and not isinstance(args[key], list):
                return f"error: `{key}` must be an array of duties, not {type(args[key]).__name__}.", True
        defs = [{"name": a.get("name", "duty"), "def": automation_def(a)} for key in ("automations", "add_automations")
                for a in args.get(key) or [] if isinstance(a, dict)]
        errors = [e for e in check_defs(defs, work) if e]
        if errors:
            return "error: agent.json was not saved; fix these workflow steps:\n" + "\n".join(errors), True
        return json.dumps({"draft_id": "draft-1", "line": "Drafted. The owner already said yes: call again with only the draft_id."}), False
    return f"error: {tool} is not available while setting up; the bot's facts are above and in find_tools, find_plugins, list_employees and use_skill.", True


def generate_janus(req: dict, surfaces: list, model: str, gen_dir: pathlib.Path) -> dict:
    frame = prompt_for(req, surfaces)
    head, tail = frame.split("Your tools for this", 1)
    system = head.rstrip() + "\nMake every call needed to set this up completely now; the owner has already said yes to it. When done, reply to the owner in a few lines."
    messages = [{"role": "system", "content": system}, {"role": "user", "content": req["request"]}]
    tools = [{"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["input_schema"]}} for t in surfaces + DISCOVERY]
    calls, refusals, reply, error = [], [], "", ""
    for _ in range(14):
        resp = janus(model, messages, tools)
        if "error" in resp or not resp.get("choices"):
            error = str(resp.get("error") or "no choices")
            break
        msg = resp["choices"][0]["message"]
        tcs = msg.get("tool_calls") or []
        messages.append({k: v for k, v in msg.items() if k in ("role", "content", "tool_calls")})
        if not tcs:
            reply = msg.get("content") or ""
            break
        for tc in tcs:
            name = tc["function"]["name"]
            try:
                args = json.loads(tc["function"].get("arguments") or "{}")
            except json.JSONDecodeError:
                args = {}
            content, refused = tool_answer(name, args, gen_dir.parent, req)
            if refused:
                refusals.append(content)
            elif name in {t["name"] for t in DISCOVERY}:
                pass
            elif name in ("create_workflow", "update_workflow") or set(args) - {"draft_id"}:
                calls.append({"tool": name, "args": args})
            messages.append({"role": "tool", "tool_call_id": tc["id"], "content": content})
    return {"calls": calls, "reply": reply, "refusals": refusals, "first_try": not refusals, "raw": error}


def tool_refusals(calls: list, work: pathlib.Path) -> list:
    """What create_workflow/update_workflow would answer: a definition that
    is not JSON, or the save path's validator errors (napp::workflow_check)."""
    refusals, to_check = [], []
    for i, c in enumerate(calls or []):
        if c.get("tool") not in ("create_workflow", "update_workflow"):
            continue
        d = (c.get("args") or {}).get("definition")
        if isinstance(d, str):
            try:
                d = json.loads(d)
            except json.JSONDecodeError as e:
                refusals.append(f"call {i + 1} ({c['tool']}): definition is not valid JSON: {e}")
                continue
        if isinstance(d, dict):
            to_check.append((i, c["tool"], {"name": (c.get("args") or {}).get("name") or "workflow", "def": d}))
    errors = check_defs([x[2] for x in to_check], work)
    for (i, tool, _), err in zip(to_check, errors):
        if err:
            refusals.append(f"call {i + 1} ({tool}): The workflow was not saved; fix these steps and send it again:\n{err}")
    return refusals


def generate(req: dict, surfaces: list, model: str, gen_dir: pathlib.Path) -> str:
    kind, _, name = model.partition(":") if ":" in model else ("claude", "", model)
    if kind == "janus":
        out = generate_janus(req, surfaces, name, gen_dir)
        calls, reply, raw, refusals, first_try = out["calls"], out["reply"], out["raw"], out["refusals"], out["first_try"]
    else:
        prompt = prompt_for(req, surfaces)
        raw = ask(prompt, name)
        answer = first_json_object(raw) or {}
        calls = answer.get("calls") if isinstance(answer.get("calls"), list) else []
        # As in Nebo, a refused save comes back to the model once to fix.
        refusals = tool_refusals(calls, gen_dir.parent)
        first_try = not refusals
        if refusals:
            retry = (prompt + "\n\nYou answered:\n" + raw + "\n\nThe tools answered:\n" + "\n".join(refusals)
                     + "\n\nSend your corrected complete answer, the whole JSON object again.")
            raw2 = ask(retry, name)
            fixed = first_json_object(raw2)
            if fixed and isinstance(fixed.get("calls"), list):
                raw, answer, calls = raw2, fixed, fixed["calls"]
        reply = answer.get("reply", "")
        raw = raw if not answer else ""
    defs, ceilings = extract(calls)
    record = {
        "id": req["id"], "request": req["request"], "context": {k: req.get(k, []) for k in ("plugins", "mcp", "experts")},
        "expect": req.get("expect", {}), "calls": calls, "reply": reply,
        "defs": defs, "ceilings": ceilings, "raw": raw,
        "first_try": first_try, "refusals": refusals,
    }
    (gen_dir / f"{req['id']}.json").write_text(json.dumps(record, indent=1))
    return req["id"]


def score(run: pathlib.Path) -> int:
    env = dict(os.environ, AUTHORING_EVAL_DIR=str(run))
    return subprocess.run([checker(), "authoring_eval_report", "--ignored", "--exact", "--nocapture"], env=env).returncode


def matrix(run: pathlib.Path, models: list) -> str:
    """Each model's pass rate per rule (check) and per request."""
    table, checks = {}, []
    for m in models:
        f = run / slug(m) / "scores.json"
        if not f.exists():
            continue
        scores = json.loads(f.read_text())
        per = {}
        for s in scores:
            for c in s["checks"]:
                per.setdefault(c["check"], [0, 0])
                per[c["check"]][0] += c["ok"]
                per[c["check"]][1] += 1
                if c["check"] not in checks:
                    checks.append(c["check"])
        table[m] = (sum(s["passed"] for s in scores), len(scores), per)
    if not table:
        return ""
    out = ["# Authoring eval: model matrix", "", "Requests passed, then each rule's pass rate (checks passed / checked).", "",
           "| Check | " + " | ".join(table) + " |", "|---|" + "---|" * len(table)]
    out.append("| **requests passed** | " + " | ".join(f"**{p}/{n}**" for p, n, _ in table.values()) + " |")
    for c in checks:
        cells = []
        for _, _, per in table.values():
            ok, n = per.get(c, [0, 0])
            cells.append(f"{ok}/{n}" if n else "-")
        out.append(f"| {c} | " + " | ".join(cells) + " |")
    return "\n".join(out) + "\n"


def slug(model: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", model)


# ── Run track ─────────────────────────────────────────────────────────

STEPS = ROOT / "fixtures" / "authoring" / "steps.json"


def run_step(step: dict, model: str) -> dict:
    name = model.partition(":")[2] or model
    messages = [{"role": "system", "content": step["system"]}, {"role": "user", "content": step["task"]}]
    if step["kind"] == "judge":
        resp = janus(name, messages)
        if "error" in resp or not resp.get("choices"):
            return {"id": step["id"], "ok": False, "detail": f"no answer: {resp.get('error')}"}
        text = resp["choices"][0]["message"].get("content") or ""
        try:
            answer = json.loads(text)  # the engine reads the reply as JSON as it stands
        except json.JSONDecodeError:
            return {"id": step["id"], "ok": False, "detail": "reply is not bare JSON: " + text[:120].replace("\n", " ")}
        got = {str(d.get("id")): str(d.get("choice", "")).lower() for d in answer.get("decisions", []) if isinstance(d, dict)}
        want = step["expect"]["decisions"]
        right = sum(got.get(k) == v for k, v in want.items())
        ok = right == len(want) and isinstance(answer.get("summary"), str)
        return {"id": step["id"], "ok": ok, "detail": f"{right}/{len(want)} decisions right" + ("" if isinstance(answer.get("summary"), str) else ", no summary")}
    # A multi-round tool step against mock tools.
    tools = [{"type": "function", "function": t} for t in step["tools"]]
    mocks = step["mocks"]
    seen, made, rounds, reply = [], [], 0, ""
    for rounds in range(1, step.get("max_rounds", 10) + 1):
        resp = janus(name, messages, tools)
        if "error" in resp or not resp.get("choices"):
            return {"id": step["id"], "ok": False, "detail": f"no answer: {resp.get('error')}"}
        msg = resp["choices"][0]["message"]
        messages.append({k: v for k, v in msg.items() if k in ("role", "content", "tool_calls")})
        tcs = msg.get("tool_calls") or []
        if not tcs:
            reply = msg.get("content") or ""
            break
        for tc in tcs:
            fn = tc["function"]["name"]
            try:
                args = json.loads(tc["function"].get("arguments") or "{}")
            except json.JSONDecodeError:
                args = {}
            key = fn + json.dumps(args, sort_keys=True)
            seen.append(key)
            made.append({"tool": fn, "args": args})
            mock = mocks.get(fn, {"error": f"unknown tool {fn}"})
            if isinstance(mock, dict) and "by" in mock:
                mock = mock["answers"].get(str(args.get(mock["by"])), mock.get("default", {"error": "not found"}))
            messages.append({"role": "tool", "tool_call_id": tc["id"], "content": json.dumps(mock)})
    else:
        return {"id": step["id"], "ok": False, "detail": f"no final reply in {rounds} rounds ({len(made)} calls)"}
    exp = step["expect"]
    problems = []
    for want in exp.get("calls", []):
        if not any(c["tool"] == want["tool"] and all(str(c["args"].get(k)) == str(v) for k, v in want.get("args", {}).items()) for c in made):
            problems.append(f"missing {want['tool']} {want.get('args', {})}")
    for bad in exp.get("never", []):
        if any(c["tool"] == bad for c in made):
            problems.append(f"called {bad}")
    if len(seen) != len(set(seen)):
        problems.append("repeated an identical call")
    if len(made) > exp.get("max_calls", 99):
        problems.append(f"{len(made)} calls (max {exp['max_calls']})")
    for word in exp.get("reply_has", []):
        if word.lower() not in reply.lower():
            problems.append(f"reply lacks {word!r}")
    return {"id": step["id"], "ok": not problems, "detail": "; ".join(problems) or f"{len(made)} calls, {rounds} rounds"}


def run_track(run: pathlib.Path, models: list, jobs: int, repeats: int) -> str:
    steps = json.loads(STEPS.read_text())
    janus_models = [m for m in models if m.startswith("janus:")]
    results = {}
    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        futs = {(m, s["id"], r): pool.submit(run_step, s, m) for m in janus_models for s in steps for r in range(repeats)}
        for (m, sid, r), f in futs.items():
            results.setdefault(m, {}).setdefault(sid, []).append(f.result())
    (run / "run-track.json").write_text(json.dumps(results, indent=1))
    out = ["# Run track: can the model RUN each step type", "",
           f"Each fixed step {repeats}x per model; cell = runs passed (first failure's reason).", "",
           "| Step | Kind | " + " | ".join(janus_models) + " |", "|---|---|" + "---|" * len(janus_models)]
    for s in steps:
        cells = []
        for m in janus_models:
            rs = results.get(m, {}).get(s["id"], [])
            ok = sum(r["ok"] for r in rs)
            why = next((r["detail"] for r in rs if not r["ok"]), "")
            cells.append(f"{ok}/{len(rs)}" + (f" ({why[:80]})" if why else ""))
        out.append(f"| {s['id']} | {s['kind']} | " + " | ".join(cells) + " |")
    return "\n".join(out) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", default=os.environ.get("MODELS") or os.environ.get("MODEL") or "claude:haiku")
    ap.add_argument("--only", default=os.environ.get("ONLY", ""))
    ap.add_argument("--score", default=os.environ.get("SCORE", ""), help="re-score this run dir")
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("JOBS", "6")))
    ap.add_argument("--surfaces", default=os.environ.get("SURFACES", ""), help="use these exported surfaces (e.g. another branch's) instead of this tree's")
    ap.add_argument("--track", default=os.environ.get("TRACK", "author"), choices=["author", "run", "both"])
    ap.add_argument("--repeats", type=int, default=int(os.environ.get("REPEATS", "3")), help="run-track repeats per step")
    a = ap.parse_args()
    models = [m.strip() for m in a.models.split(",") if m.strip()]
    if any(m.startswith("janus:") for m in models) and not os.environ.get("JANUS_TOKEN"):
        print("janus:<model> needs JANUS_TOKEN (and JANUS_URL, default https://janus.neboloop.com/v1)", file=sys.stderr)
        return 2

    if a.score:
        run = pathlib.Path(a.score).resolve()
        if (run / "gen").is_dir():
            return score(run)
        rc = 0
        done = [m for m in models if (run / slug(m) / "gen").is_dir()] or [d.name for d in run.iterdir() if (d / "gen").is_dir()]
        for m in done:
            rc |= score(run / slug(m))
        (run / "matrix.md").write_text(matrix(run, done))
        print(matrix(run, done))
        return rc

    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run = ROOT / "target" / "eval-authoring" / stamp
    run.mkdir(parents=True)
    latest = run.parent / "latest"
    if latest.is_symlink() or latest.exists():
        latest.unlink()
    latest.symlink_to(run.name)
    checker()
    rc = 0
    if a.track in ("author", "both"):
        if a.surfaces:
            src = pathlib.Path(a.surfaces)
            surfaces = [json.loads((src / f"{n}.json").read_text())
                        for n in ("create_workflow", "update_workflow", "create_employee", "update_employee")]
        else:
            surfaces = export_surfaces(run)
        requests = json.loads(REQUESTS.read_text())
        if a.only:
            wanted = set(a.only.split(","))
            requests = [r for r in requests if r["id"] in wanted]
        for m in models:
            mdir = run / slug(m)
            (mdir / "gen").mkdir(parents=True)
            (mdir / "meta.json").write_text(json.dumps({"model": m, "requests": len(requests), "surfaces": a.surfaces or "this tree"}))
        print(f"eval-authoring: {len(requests)} requests x {len(models)} models -> {run}", file=sys.stderr)
        with concurrent.futures.ThreadPoolExecutor(a.jobs) as pool:
            jobs = [pool.submit(generate, r, surfaces, m, run / slug(m) / "gen") for m in models for r in requests]
            for f in concurrent.futures.as_completed(jobs):
                print(f"  generated {f.result()}", file=sys.stderr)
        for m in models:
            rc |= score(run / slug(m))
        mx = matrix(run, models)
        (run / "matrix.md").write_text(mx)
        print(mx)
    if a.track in ("run", "both"):
        rt = run_track(run, models, a.jobs, a.repeats)
        (run / "run-track.md").write_text(rt)
        print(rt)
    return rc


if __name__ == "__main__":
    sys.exit(main())
