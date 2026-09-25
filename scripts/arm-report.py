#!/usr/bin/env python3
"""Side-by-side report of two harness-gate runs: arm A (the old loop) and
arm P (the rewrite), or any two runs of the same fixtures.

    scripts/arm-report.py A P [--out DIR] [--no-janus] [--title TEXT]

A and P are gate run ids (the kept run is fetched from the stadium VM,
~/harness-runs/<id>, into ~/.cache/nebo-arm-runs/<id>) or local directories
holding a kept run (traces/<entry>/<fixture>_run-N.json, optional run.json).

The report (markdown, written to --out, default ~/Desktop/NeboAI-Brand):
  - per suite and per fixture: program-check and judged pass rates, critical
    failures, runs that never completed;
  - per turn, from each trace's `turns` rows: first-reply latency, model calls
    (steps) and tool calls per task, largest request (prompt size), cards and
    permission asks;
  - lost-thread and wrong-continuation counts (the fixtures below);
  - Janus usage by bot over each run's window (usage_history in ScyllaDB,
    read-only SELECT through kubectl): requests, tokens, prompt size, cost,
    cost per completed fixture run. Needs run.json (bot id and window);
  - with --pairwise SET (a replay set from scripts/export-threads.py): a blind
    judge (the claude CLI) reads each replayed thread's owner messages and the
    two arms' replies as X and Y, in an order it cannot infer, and picks the
    one that serves the owner better. Tool calls are not shown: their names
    would tell the arms apart.

Severity comes from the fixtures in this checkout, so run it from the commit
that dispatched both runs.

An A-vs-P comparison (turn-controller design WP3.1). Arm A is the branch
baseline/pre-harness, pinned at c34e3970 (main before any harness move) and
locked: it never moves until the cutover stack lands. Both arms are dispatched
from one commit, which supplies the runner and fixtures; `ref` builds only the
server, and `arm` picks the CI bot (two bots, so both run at once):

    S="suites/smoke.yaml suites/error-correction.yaml suites/turn-controller.yaml"
    gh workflow run harness-gate.yml --ref main -f arm=a -f ref=baseline/pre-harness    -f suites="$S" -f judge=true
    gh workflow run harness-gate.yml --ref main -f arm=p -f ref=harness/turn-controller -f suites="$S" -f judge=true
    scripts/arm-report.py <A run id> <P run id>

Real threads (scripts/export-threads.py export + push) run as
suites="replays/<set>/suite.yaml" on both arms, then
`scripts/arm-report.py A P --pairwise <set>`. An A-vs-A (the same ref twice)
checks the instrument: it should read about equal.
"""
import argparse
import collections
import datetime as dt
import glob
import hashlib
import json
import os
import re
import statistics
import subprocess
import sys

import yaml

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.expanduser("~/.cache/nebo-arm-runs")
DEFAULT_OUT = os.path.expanduser("~/Desktop/NeboAI-Brand")

# A run of one of these fixtures that fails a critical assertion lost the
# owner's thread: the referent, the objective, a message, a stage.
LOST_THREAD = {
    "naming-thread-keeps-referent", "objective-keeps-its-referent",
    "goal-persistence-across-segue", "mid-turn-owner-message",
    "checkpoint-keeps-every-owner-message", "long-multistage-no-drift",
    "helper-finishes-while-owner-chats",
}
# ... continued when it should have stopped, or stopped when it should have
# gone on.
WRONG_CONTINUATION = {
    "no-continue-after-question", "announced-step-not-done", "proactive-multistep-action",
}

VM_FETCH = ("ssh -o ConnectTimeout=20 stadium 'export PATH=/opt/homebrew/bin:$PATH; "
            "limactl shell ci -- bash -c \"tar -C ~/harness-runs -cz {id}\"' | tar -xz -C {dest}")
REPLAYS = os.path.expanduser("~/.cache/nebo-replays")
JUDGE_MODEL = "claude-opus-5"
JUDGE_PROMPT = """You are judging two assistants that each received the same messages from the owner of a small business, one message per turn, in order. You see the owner's messages and each assistant's replies (all turns, joined). Tool calls are not shown.

Which assistant served the owner better? Judge: does it answer each message in context, keep what the owner said earlier, do the work asked or say plainly what blocks it, stay on the owner's thread, and never claim a result it could not have had? Length and polish count only as far as they help the owner.

Answer with one JSON object and nothing else: {{"winner": "X" | "Y" | "tie", "reason": "<one or two sentences>"}}

OWNER'S MESSAGES
{owner}

ASSISTANT X ({x_calls} tool calls)
{x}

ASSISTANT Y ({y_calls} tool calls)
{y}
"""
KUBE = ["kubectl", "--context", "do-nyc3-nebo-doks", "-n", "scylla", "exec",
        "scylla-nyc3-rack1-0", "-c", "scylla", "--", "cqlsh", "-e"]


# ---------------------------------------------------------------- loading

def resolve(arg):
    if os.path.isdir(arg):
        return os.path.abspath(arg)
    if not arg.isdigit():
        sys.exit(f"{arg}: neither a directory nor a gate run id")
    dest = os.path.join(CACHE, arg)
    if not os.path.isdir(os.path.join(dest, "traces")):
        os.makedirs(CACHE, exist_ok=True)
        for attempt in range(3):
            if subprocess.run(VM_FETCH.format(id=arg, dest=CACHE), shell=True).returncode == 0:
                break
        else:
            sys.exit(f"could not fetch run {arg} from the stadium VM")
    return dest


def severities():
    """(fixture id, assertion id) -> severity, from this checkout's fixtures."""
    out = {}
    for path in glob.glob(os.path.join(ROOT, "fixtures", "**", "*.yaml"), recursive=True):
        try:
            fx = yaml.safe_load(open(path))
        except Exception:
            continue
        if not isinstance(fx, dict) or "id" not in fx:
            continue
        groups = list((fx.get("prompt_assertions") or {}).values()) + [fx.get("integrated_assertions") or []]
        for group in groups:
            for a in group or []:
                if isinstance(a, dict) and "id" in a:
                    out[(fx["id"], a["id"])] = a.get("severity", "important")
    return out


class Run:
    def __init__(self, label, path):
        self.label, self.path = label, path
        meta = os.path.join(path, "run.json")
        self.meta = json.load(open(meta)) if os.path.exists(meta) else {}
        self.name = os.path.basename(path.rstrip("/"))
        # entry (suite or fixture) -> fixture -> [trace]
        self.traces = collections.defaultdict(lambda: collections.defaultdict(list))
        for f in sorted(glob.glob(os.path.join(path, "traces", "*", "*.json"))):
            try:
                t = json.load(open(f))
            except Exception:
                continue
            self.traces[os.path.basename(os.path.dirname(f))][t["fixture_id"]].append(t)

    def all(self):
        for entry, fxs in self.traces.items():
            for fx, ts in fxs.items():
                for t in ts:
                    yield entry, fx, t

    def window(self):
        """[start, end] of the run: run.json, else the traces' own times."""
        if self.meta.get("started_at") and self.meta.get("ended_at"):
            return self.meta["started_at"], self.meta["ended_at"]
        spans = []
        for _, _, t in self.all():
            if t.get("timestamp"):
                end = parse_ts(t["timestamp"])
                spans.append((end - dt.timedelta(milliseconds=t["metrics"].get("total_latency_ms", 0)), end))
        if not spans:
            return None, None
        starts, ends = [a for a, _ in spans], [b for _, b in spans]
        return iso(min(starts) - dt.timedelta(minutes=1)), iso(max(ends) + dt.timedelta(minutes=1))


def parse_ts(s):
    """RFC 3339 with any fraction (chrono writes nanoseconds)."""
    m = re.match(r"^(.*?T\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)?$", s.strip())
    if not m:
        raise ValueError(f"not a timestamp: {s}")
    frac = (m.group(2) or "0")[:6].ljust(6, "0")
    tz = m.group(3) or "Z"
    return dt.datetime.fromisoformat(f"{m.group(1)}.{frac}{'+00:00' if tz == 'Z' else tz}")


def iso(t):
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# ---------------------------------------------------------------- scoring

def judged_traces(run):
    """How many completed runs a judge graded, by which model, and how many
    judge attempts failed: read from the traces, since a run dispatched with
    judge off is judged afterwards by `nebo-cli test grade`."""
    done = judged = failed = 0
    models = collections.Counter()
    for _, _, t in run.all():
        if t.get("failure_reason"):
            continue
        done += 1
        g = t.get("grade") or {}
        if g.get("judge_error"):
            failed += 1
        elif g.get("judge") or any(a.get("mode", "judged") != "verified" for a in g.get("assertions") or []):
            judged += 1
            models[g.get("judge") or "in the run"] += 1
    by = ", ".join(f"{m} {c}" for m, c in models.most_common())
    return f"{judged} / {done}" + (f" ({by})" if by else "") + (f", judge failed {failed}" if failed else "")


def grade(t, sev, fx):
    """(verified pass, verified total, judged pass, judged total, critical fails)"""
    vp = vt = jp = jt = 0
    crit = []
    for a in ((t.get("grade") or {}).get("assertions") or []):
        judged = a.get("mode", "judged") != "verified"
        if judged:
            jt += 1
            jp += bool(a["passed"])
        else:
            vt += 1
            vp += bool(a["passed"])
        if not a["passed"] and sev.get((fx, a["id"])) == "critical":
            crit.append(a["id"])
    return vp, vt, jp, jt, crit


def rate(p, n):
    return f"{p}/{n} ({100 * p / n:.0f}%)" if n else "–"


def pct(p, n):
    return 100 * p / n if n else None


def delta(a, p, fmt="{:+.0f}", lower_is_better=False):
    if a is None or p is None:
        return ""
    d = p - a
    if abs(d) < 1e-9:
        return "="
    good = (d < 0) if lower_is_better else (d > 0)
    return fmt.format(d) + (" ✓" if good else " ✗")


def med(xs):
    return statistics.median(xs) if xs else None


def p90(xs):
    if not xs:
        return None
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(0.9 * (len(xs) - 1))))]


def num(x, unit=""):
    if x is None:
        return "–"
    if isinstance(x, float):
        return f"{x:,.1f}{unit}"
    return f"{x:,}{unit}"


def per_turn(run):
    """Owner turns for latency and the turn count; every turn, woken ones
    included (the session's own turns after a helper reported), for steps,
    prompt size and cards: those are the task's cost."""
    first, steps, tools, prompt, cards, approvals, turns, woken = [], [], [], [], 0, 0, 0, 0
    runs_with_turns = 0
    for _, _, t in run.all():
        rows = t.get("turns") or []
        if rows:
            runs_with_turns += 1
            steps.append(sum(r.get("model_calls", 0) for r in rows))
        tools.append(t["metrics"].get("total_tool_calls", 0))
        for r in rows:
            if r.get("woken"):
                woken += 1
            else:
                turns += 1
            if r.get("first_reply_ms") is not None and not r.get("woken"):
                first.append(r["first_reply_ms"] / 1000)
            prompt.append(r.get("max_prompt_tokens", 0))
            cards += r.get("cards", 0)
            approvals += r.get("approvals", 0)
    return dict(first=first, steps=steps, tools=tools, prompt=[p for p in prompt if p],
                cards=cards, approvals=approvals, turns=turns, woken=woken, runs_with_turns=runs_with_turns)


def thread_counts(run, sev):
    lost = wrong = 0
    for _, fx, t in run.all():
        bad = bool(t.get("failure_reason")) or bool(grade(t, sev, fx)[4])
        lost += bad and fx in LOST_THREAD
        wrong += bad and fx in WRONG_CONTINUATION
    return lost, wrong


# ---------------------------------------------------------------- janus

def janus_usage(run):
    bot = run.meta.get("bot_id", "")
    if not re.fullmatch(r"[0-9a-f-]{36}", bot):
        return None, "no bot id (run.json, or --bot for an older run)"
    start, end = run.window()
    if not bot or not start:
        return None, "no run.json bot id or window"
    q = ("SELECT JSON model, timestamp, request_tokens, response_tokens, cache_read_tokens, "
         "cost_micro, duration_ms, status FROM janus.usage_history "
         f"WHERE bot_id='{bot}' AND timestamp >= '{start}' AND timestamp <= '{end}' ALLOW FILTERING")
    try:
        out = subprocess.run(KUBE + [q], capture_output=True, text=True, timeout=300)
    except Exception as e:
        return None, str(e)
    if out.returncode != 0:
        return None, out.stderr.strip()[:200]
    rows = []
    for line in out.stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            rows.append(json.loads(line))
    return rows, None


def janus_summary(rows, completed):
    n = len(rows)
    inp = sum(r.get("request_tokens") or 0 for r in rows)
    out = sum(r.get("response_tokens") or 0 for r in rows)
    cache = sum(r.get("cache_read_tokens") or 0 for r in rows)
    cost = sum(r.get("cost_micro") or 0 for r in rows)
    models = collections.Counter(r.get("model") for r in rows)
    return dict(
        requests=n, input=inp, output=out, cache=cache, cost_usd=cost / 1e6,
        prompt_avg=inp / n if n else None,
        cost_per_fixture=(cost / 1e6 / completed) if completed else None,
        errors=sum(1 for r in rows if (r.get("status") or "ok") != "ok"),
        models=", ".join(f"{m} {c}" for m, c in models.most_common(4)),
    )


# ---------------------------------------------------------------- blind pairwise judge

def judge_pair(owner, x, y):
    prompt = JUDGE_PROMPT.format(owner=owner, x=x["final_response"]["content"].strip() or "(no reply)",
                                 y=y["final_response"]["content"].strip() or "(no reply)",
                                 x_calls=x["metrics"].get("total_tool_calls", 0),
                                 y_calls=y["metrics"].get("total_tool_calls", 0))
    ws = os.path.join(CACHE, "judge-workspace")
    os.makedirs(ws, exist_ok=True)
    env = dict(os.environ, CLAUDE_CODE_DISABLE_AUTO_MEMORY="1", CLAUDE_CODE_DISABLE_CLAUDE_MDS="1")
    out = subprocess.run(["claude", "--print", "--output-format", "json", "--model", JUDGE_MODEL],
                         input=prompt, capture_output=True, text=True, cwd=ws, env=env, timeout=600)
    if out.returncode != 0:
        return None, f"judge failed: {out.stderr.strip()[:160]}"
    try:
        text = json.loads(out.stdout).get("result", "")
        verdict = json.loads(re.search(r"\{.*\}", text, re.S).group(0))
        return verdict.get("winner"), verdict.get("reason", "")
    except (ValueError, AttributeError) as e:
        return None, f"unreadable verdict ({e})"


def pairwise(A, P, sets):
    L = ["## Blind pairwise judge\n",
         f"`{JUDGE_MODEL}` via the claude CLI. Each pair is shown as X and Y in an order fixed by a hash of the "
         "fixture and run, so neither the judge nor a rerun can tell which arm is which.\n"]
    for name in sets:
        set_dir = os.path.join(REPLAYS, name)
        convs = {}
        for f in glob.glob(os.path.join(set_dir, "*.yaml")):
            fx = yaml.safe_load(open(f))
            if isinstance(fx, dict) and fx.get("conversation"):
                convs[fx["id"]] = [t["content"] for t in fx["conversation"]]
        wins = collections.Counter()
        rows = []
        for fx in sorted(set(A.traces.get(name, {})) & set(P.traces.get(name, {}))):
            owner = "\n\n".join(f"{i}. {m}" for i, m in enumerate(convs.get(fx, []), 1)) or "(not in the local set)"
            pa = {t["run_id"]: t for t in A.traces[name][fx]}
            pp = {t["run_id"]: t for t in P.traces[name][fx]}
            for run in sorted(set(pa) & set(pp)):
                a_is_x = int(hashlib.sha256(f"{fx}/{run}".encode()).hexdigest(), 16) % 2 == 0
                x, y = (pa[run], pp[run]) if a_is_x else (pp[run], pa[run])
                winner, reason = judge_pair(owner, x, y)
                arm = {"X": "A" if a_is_x else "P", "Y": "P" if a_is_x else "A", "tie": "tie"}.get(winner, "–")
                wins[arm] += 1
                rows.append(f"| {fx} | {run} | {arm} | {reason.replace('|', '/')} |")
        L.append(f"### {name}\n")
        L.append(f"A better: **{wins['A']}** · P better: **{wins['P']}** · tie: **{wins['tie']}**"
                 + (f" · no verdict: {wins['–']}" if wins["–"] else "") + "\n")
        L.append("| thread | run | better | judge's reason |\n|---|---|---|---|")
        L.extend(rows)
        L.append("")
    return L


# ---------------------------------------------------------------- report

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("a")
    ap.add_argument("p")
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--no-janus", action="store_true")
    ap.add_argument("--title", default="")
    ap.add_argument("--bot", default="", help="bot id for a run kept without run.json (older runs)")
    ap.add_argument("--pairwise", action="append", default=[], metavar="SET",
                    help="replay set (~/.cache/nebo-replays/SET) to judge blind, A against P")
    args = ap.parse_args()

    A, P = Run("A", resolve(args.a)), Run("P", resolve(args.p))
    for r in (A, P):
        if args.bot and not r.meta.get("bot_id"):
            r.meta["bot_id"] = args.bot
    sev = severities()
    L = []
    w = L.append
    title = args.title or f"Arm comparison: {A.name} (A) vs {P.name} (P)"
    w(f"# {title}\n")
    w(f"Generated {dt.datetime.now(dt.timezone.utc):%Y-%m-%d %H:%M} UTC by `scripts/arm-report.py`.\n")
    w("| | A | P |\n|---|---|---|")
    for key, name in [("arm", "arm"), ("code_sha", "code under test"), ("runner_sha", "runner / fixtures"),
                      ("suites", "suites"), ("judge", "judged"), ("model", "model"),
                      ("started_at", "started"), ("ended_at", "ended")]:
        va, vp = A.meta.get(key, "–"), P.meta.get(key, "–")
        if key.endswith("sha"):
            va, vp = str(va)[:10], str(vp)[:10]
        w(f"| {name} | {va} | {vp} |")
    w(f"| judged traces (in the run or by `nebo-cli test grade`) | {judged_traces(A)} | {judged_traces(P)} |")
    w("")

    # ---- per suite
    w("## Per suite\n")
    w("| suite | fixtures | runs A / P | program A | program P | Δ pts | judged A | judged P | Δ pts | critical fails A / P | incomplete A / P |")
    w("|---|---|---|---|---|---|---|---|---|---|---|")
    totals = {r.label: [0, 0, 0, 0, 0, 0, 0] for r in (A, P)}
    for entry in sorted(set(A.traces) | set(P.traces)):
        row = {}
        for r in (A, P):
            vp = vt = jp = jt = crit = inc = runs = 0
            for fx, ts in r.traces.get(entry, {}).items():
                for t in ts:
                    runs += 1
                    g = grade(t, sev, fx)
                    vp, vt, jp, jt = vp + g[0], vt + g[1], jp + g[2], jt + g[3]
                    crit += len(g[4])
                    inc += bool(t.get("failure_reason"))
            row[r.label] = (vp, vt, jp, jt, crit, inc, runs)
            for i, v in enumerate(row[r.label]):
                totals[r.label][i] += v
        a, p = row["A"], row["P"]
        nfx = len(set(A.traces.get(entry, {})) | set(P.traces.get(entry, {})))
        w(f"| {entry} | {nfx} | {a[6]} / {p[6]} | {rate(a[0], a[1])} | {rate(p[0], p[1])} | "
          f"{delta(pct(a[0], a[1]), pct(p[0], p[1]))} | {rate(a[2], a[3])} | {rate(p[2], p[3])} | "
          f"{delta(pct(a[2], a[3]), pct(p[2], p[3]))} | {a[4]} / {p[4]} | {a[5]} / {p[5]} |")
    a, p = totals["A"], totals["P"]
    w(f"| **all** | | {a[6]} / {p[6]} | {rate(a[0], a[1])} | {rate(p[0], p[1])} | "
      f"{delta(pct(a[0], a[1]), pct(p[0], p[1]))} | {rate(a[2], a[3])} | {rate(p[2], p[3])} | "
      f"{delta(pct(a[2], a[3]), pct(p[2], p[3]))} | {a[4]} / {p[4]} | {a[5]} / {p[5]} |\n")

    # ---- thread and continuation
    la, wa = thread_counts(A, sev)
    lp, wp = thread_counts(P, sev)
    w("## Thread and continuation\n")
    w("Runs of the named fixtures that failed a critical assertion or never completed.\n")
    w("| | A | P |\n|---|---|---|")
    w(f"| lost thread ({', '.join(sorted(LOST_THREAD))}) | {la} | {lp} |")
    w(f"| wrong continuation ({', '.join(sorted(WRONG_CONTINUATION))}) | {wa} | {wp} |\n")

    # ---- per turn
    ta, tp = per_turn(A), per_turn(P)
    w("## Per turn\n")
    w(f"From the traces' per-turn rows ({ta['runs_with_turns']} of A's and {tp['runs_with_turns']} of P's runs carry them; "
      "older traces have run totals only).\n")
    w("| | A | P | Δ |\n|---|---|---|---|")
    for name, xa, xp, unit, lower in [
        ("first reply, median (s)", med(ta["first"]), med(tp["first"]), "", True),
        ("first reply, p90 (s)", p90(ta["first"]), p90(tp["first"]), "", True),
        ("model calls (steps) per task, mean", statistics.mean(ta["steps"]) if ta["steps"] else None,
         statistics.mean(tp["steps"]) if tp["steps"] else None, "", True),
        ("tool calls per task, mean", statistics.mean(ta["tools"]) if ta["tools"] else None,
         statistics.mean(tp["tools"]) if tp["tools"] else None, "", True),
        ("largest request per turn, median (tokens)", med(ta["prompt"]), med(tp["prompt"]), "", True),
        ("largest request per turn, p90 (tokens)", p90(ta["prompt"]), p90(tp["prompt"]), "", True),
    ]:
        w(f"| {name} | {num(xa)} | {num(xp)} | {delta(xa, xp, '{:+,.1f}', lower)} |")
    w(f"| owner turns | {ta['turns']} | {tp['turns']} | |")
    w(f"| turns woken by the run's own background work | {ta['woken']} | {tp['woken']} | |")
    w(f"| permission asks (approval cards) | {ta['approvals']} | {tp['approvals']} | |")
    w(f"| other cards (install, connect, plan) | {ta['cards']} | {tp['cards']} | |\n")

    # ---- janus
    w("## Janus usage by bot\n")
    if args.no_janus:
        w("Skipped (--no-janus).\n")
    else:
        sums = {}
        for r in (A, P):
            rows, err = janus_usage(r)
            completed = sum(1 for _, _, t in r.all() if not t.get("failure_reason"))
            sums[r.label] = janus_summary(rows, completed) if rows is not None else err
        if all(isinstance(v, dict) for v in sums.values()):
            ja, jp_ = sums["A"], sums["P"]
            w("Every request the run's bot made in the run's window (usage_history). "
              "Both arms on one bot must not overlap in time.\n")
            w("| | A | P | Δ |\n|---|---|---|---|")
            for key, name, lower in [("requests", "requests", True), ("input", "input tokens", True),
                                     ("output", "output tokens", True), ("cache", "cache-read tokens", False),
                                     ("prompt_avg", "prompt size, mean input tokens", True),
                                     ("cost_usd", "cost (USD)", True),
                                     ("cost_per_fixture", "cost per completed fixture run (USD)", True),
                                     ("errors", "error responses", True)]:
                fmt = "{:+,.4f}" if "cost" in key else "{:+,.0f}"
                va, vp = ja[key], jp_[key]
                show = (lambda x: "–" if x is None else f"{x:,.4f}") if "cost" in key else num
                w(f"| {name} | {show(va)} | {show(vp)} | {delta(va, vp, fmt, lower)} |")
            w(f"| models | {ja['models']} | {jp_['models']} | |\n")
        else:
            for k, v in sums.items():
                if not isinstance(v, dict):
                    w(f"- {k}: not available ({v})")
            w("")

    if args.pairwise:
        L.extend(pairwise(A, P, args.pairwise))

    # ---- per fixture
    w("## Per fixture\n")
    w("Program and judged: assertions passed / graded across runs. Calls and latency: mean per run.\n")
    for entry in sorted(set(A.traces) | set(P.traces)):
        w(f"### {entry}\n")
        w("| fixture | program A | program P | judged A | judged P | critical fails A | critical fails P | calls A / P | latency s A / P |")
        w("|---|---|---|---|---|---|---|---|---|")
        for fx in sorted(set(A.traces.get(entry, {})) | set(P.traces.get(entry, {}))):
            cells = []
            for r in (A, P):
                ts = r.traces.get(entry, {}).get(fx, [])
                g = [grade(t, sev, fx) for t in ts]
                crit = collections.Counter(c for x in g for c in x[4])
                inc = sum(1 for t in ts if t.get("failure_reason"))
                calls = statistics.mean([t["metrics"].get("total_tool_calls", 0) for t in ts]) if ts else None
                lat = statistics.mean([t["metrics"].get("total_latency_ms", 0) / 1000 for t in ts]) if ts else None
                cells.append(dict(
                    prog=rate(sum(x[0] for x in g), sum(x[1] for x in g)),
                    jud=rate(sum(x[2] for x in g), sum(x[3] for x in g)),
                    crit=(", ".join(f"{k}×{v}" for k, v in crit.items()) or "–") + (f" (+{inc} incomplete)" if inc else ""),
                    calls=num(calls), lat=num(lat)))
            a, p = cells
            w(f"| {fx} | {a['prog']} | {p['prog']} | {a['jud']} | {p['jud']} | {a['crit']} | {p['crit']} | "
              f"{a['calls']} / {p['calls']} | {a['lat']} / {p['lat']} |")
        w("")

    os.makedirs(args.out, exist_ok=True)
    out = os.path.join(args.out, f"Arm-Comparison-{A.name}-vs-{P.name}.md")
    open(out, "w").write("\n".join(L) + "\n")
    print(out)


if __name__ == "__main__":
    main()
