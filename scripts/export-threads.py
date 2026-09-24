#!/usr/bin/env python3
"""Export real owner threads as replay fixtures for an A-vs-P comparison.

    scripts/export-threads.py list   --source SRC [--limit N]
    scripts/export-threads.py export --source SRC --chat ID --set NAME [--turns N] [--names FILE]
    scripts/export-threads.py push   --set NAME

SRC is `desktop` (the owner's own Nebo database) or `cloud:<bot id>` (a cloud
bot's database, through kubectl exec). Both are opened READ-ONLY: sqlite
`mode=ro` on the desktop, `sqlite3 -readonly` in the pod. Nothing is written
to either.

A thread becomes a fixture whose conversation is the owner's own messages, in
order (the first N with --turns). The runner sends them one by one to the arm
under test, so each arm writes its own replies; `scripts/arm-report.py
--pairwise <set>` then has a blind judge compare the two arms' replies.

Real threads never go into git. A set is written to ~/.cache/nebo-replays/<set>
and `push` copies it to the stadium VM (~/harness-replays/<set>), where the
gate's sweep runs it as `replays/<set>/suite.yaml`: the job copies the set into
its own directory, so no other run can read it.

Names are replaced with generic ones before anything is written:
  - the owner (users.name / email, user_profiles.display_name) -> "Sam Rivera"
  - employees (agents.name) -> "Employee 1", "Employee 2", ...
  - every name in --names FILE (one per line, `Real Name` or `Real Name => Generic`)
  - email addresses, phone numbers and web domains -> example.com / 555-01xx
After writing, the capitalised words still in the set are printed: read them,
add any person or company to the names file and export again.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

import yaml

DESKTOP_DB = os.path.expanduser("~/Library/Application Support/Nebo/data/nebo.db")
CLOUD_DB = "/data/data/nebo.db"
KUBE = ["kubectl", "--context", "do-nyc3-nebo-doks", "-n", "nebo-bots"]
OUT = os.path.expanduser("~/.cache/nebo-replays")
VM_PUSH = ("COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C {src} -cz {set} | ssh -o ConnectTimeout=20 stadium 'export PATH=/opt/homebrew/bin:$PATH; "
           "limactl shell ci -- bash -c \"mkdir -p ~/harness-replays && rm -rf ~/harness-replays/{set} && "
           "tar -xz -C ~/harness-replays\"'")
CONTINUATION_PREFIX = "Continue — your previous response committed to more work that isn't done yet:"
# User-role rows the owner never typed: a helper's brief (orchestrator.rs
# `agent_type_prefix`) and the budget-exhausted summary request older builds
# stored (runner.rs BUDGET_SUMMARY_REQUEST).
NOT_OWNER_PREFIXES = (CONTINUATION_PREFIX, "[Execute the task using whatever tools are needed.]",
                      "[EXPLORE agent", "[PLANNING agent",
                      "You've reached the maximum number of tool-calling iterations allowed.")
# An owner message that arrived mid-turn is stored framed; the owner's words
# are what gets replayed. Current builds (runner.rs frame_mid_turn_message)
# and older ones ("[Arrived while you were working, via web] ...").
MID_TURN_FRAMES = (re.compile(r"^The owner sent a new message while you were working \(via [^)]*\):\n(.*?)\n\nIMPORTANT: reply to the owner now", re.S),
                   re.compile(r"^\[Arrived while you were working, via [^\]]*\]\s*(.*)$", re.S))
OWNER_GENERIC = "Sam Rivera"

# Threads worth replaying are the owner's own: not helpers, workflows or evals.
THREAD_FILTER = ("c.session_name NOT LIKE 'subagent:%' AND c.session_name NOT LIKE '%:workflow:%' "
                 "AND c.session_name NOT LIKE '%eval:%' AND c.session_name NOT LIKE 'cron-%'")


# ---------------------------------------------------------------- read-only access

def query(source, sql):
    """Rows as dicts. The database is never opened for writing."""
    if source == "desktop":
        import sqlite3
        db = sqlite3.connect(f"file:{DESKTOP_DB}?mode=ro", uri=True)
        db.row_factory = sqlite3.Row
        return [dict(r) for r in db.execute(sql)]
    if source.startswith("cloud:"):
        bot = source.split(":", 1)[1]
        pods = subprocess.run(KUBE + ["get", "pods", "-o", "name"], capture_output=True, text=True, check=True).stdout
        pod = next((p for p in pods.split() if f"nebo-bot-{bot}" in p), None)
        if not pod:
            sys.exit(f"no running pod for bot {bot}")
        out = subprocess.run(KUBE + ["exec", pod, "--", "sqlite3", "-readonly", "-json", CLOUD_DB, sql],
                             capture_output=True, text=True, check=True).stdout
        return json.loads(out) if out.strip() else []
    sys.exit(f"--source is desktop or cloud:<bot id>, not {source}")


def owner_turns(source, chat_id):
    rows = query(source, "SELECT content, metadata FROM chat_messages WHERE chat_id = '"
                 + chat_id.replace("'", "''") + "' AND role = 'user' ORDER BY created_at, rowid")
    turns = []
    for r in rows:
        text = (r.get("content") or "").strip()
        if not text or text.startswith(NOT_OWNER_PREFIXES):
            continue
        try:
            if json.loads(r.get("metadata") or "{}").get("isMeta"):
                continue
        except (ValueError, AttributeError):
            pass
        for frame in MID_TURN_FRAMES:
            m = frame.match(text)
            if m:
                text = m.group(1).strip()
                break
        # Voice stores each growing transcript of one utterance as its own row
        # ("Okay, let's say" then "Okay, let's say we built this"): keep the
        # last. A message sent twice ("try again", "try again") is two turns.
        if turns and len(text) > len(turns[-1]) and text.startswith(turns[-1]):
            turns[-1] = text
            continue
        turns.append(text)
    return turns


# ---------------------------------------------------------------- scrubbing

class Scrubber:
    def __init__(self, source, names_file):
        self.pairs = []  # (compiled pattern, replacement), longest names first
        names = {}
        for r in query(source, "SELECT name, email FROM users"):
            for v in (r.get("name"), r.get("email")):
                if v:
                    names[v] = OWNER_GENERIC if "@" not in v else "sam@example.com"
                    for part in (v.split("@")[0] if "@" in v else v).split():
                        if len(part) > 2:
                            names.setdefault(part, OWNER_GENERIC.split()[0])
        for r in query(source, "SELECT display_name FROM user_profiles"):
            if r.get("display_name"):
                names[r["display_name"]] = OWNER_GENERIC
        for i, r in enumerate(query(source, "SELECT DISTINCT name FROM agents ORDER BY name"), 1):
            if r.get("name"):
                names.setdefault(r["name"], f"Employee {i}")
        if names_file:
            for line in open(names_file):
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                real, _, generic = line.partition("=>")
                names[real.strip()] = generic.strip() or f"Person {len(names) + 1}"
        for real in sorted(names, key=len, reverse=True):
            self.pairs.append((re.compile(r"(?<!\w)" + re.escape(real) + r"(?!\w)", re.I), names[real]))

    def __call__(self, text):
        text = re.sub(r"[\w.+-]+@[\w-]+(\.[\w-]+)+", "person@example.com", text)
        text = re.sub(r"https?://[^\s/]+", "https://example.com", text)
        text = re.sub(r"\b[\w-]+\.(com|net|org|io|co|ai|app|dev|us)\b", "example.com", text)
        text = re.sub(r"(?<!\d)(\+?1[\s.-]?)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}(?!\d)", "555-0100", text)
        for pattern, generic in self.pairs:
            text = pattern.sub(generic, text)
        return text


COMMON = set("""I A An The This That These Those It Its We You Your My Our Can Could Would Should Will Please Thanks Thank
Yes No Ok Okay Hi Hello Hey What When Where Why How Who Which If And But Or So Then Also Now Let Lets Do Does Did Is Are Was
Were Be Not Just Here There Monday Tuesday Wednesday Thursday Friday Saturday Sunday January February March April May June
July August September October November December Nebo NeboAI Employee Sam Rivera Person""".split())


def review(set_dir):
    """Capitalised words left in the set, for a person to read before a run."""
    words = {}
    for f in sorted(os.listdir(set_dir)):
        if f.endswith(".yaml") and f != "suite.yaml":
            for turn in yaml.safe_load(open(os.path.join(set_dir, f))).get("conversation", []):
                for w in re.findall(r"\b[A-Z][a-zA-Z'’-]{2,}\b", turn["content"]):
                    if w not in COMMON:
                        words[w] = words.get(w, 0) + 1
    return sorted(words.items(), key=lambda kv: -kv[1])


# ---------------------------------------------------------------- commands

def cmd_list(args):
    rows = query(args.source, f"""
        SELECT c.id, c.session_name, c.updated_at,
               SUM(m.role = 'user') AS owner_msgs, COUNT(*) AS msgs
        FROM chats c JOIN chat_messages m ON m.chat_id = c.id
        WHERE {THREAD_FILTER}
        GROUP BY c.id HAVING owner_msgs >= 2
        ORDER BY c.updated_at DESC LIMIT {int(args.limit)}""")
    for r in rows:
        print(f"{r['id']}  owner messages {r['owner_msgs']:>3}  all {r['msgs']:>4}  {r['session_name']}")


def cmd_export(args):
    turns = owner_turns(args.source, args.chat)
    if not turns:
        sys.exit(f"chat {args.chat} has no owner message to replay")
    if args.turns:
        turns = turns[: args.turns]
    scrub = Scrubber(args.source, args.names)
    tag = hashlib.sha256(f"{args.source}/{args.chat}".encode()).hexdigest()[:8]
    fixture = {
        "id": f"replay-thread-{tag}",
        "name": f"Replayed owner thread {tag} ({len(turns)} owner turns)",
        "description": "An owner thread replayed turn by turn; the arms' replies are compared by a blind judge "
                       "(scripts/arm-report.py --pairwise). Names are replaced; nothing here is from git.",
        "conversation": [{"role": "user", "content": scrub(t)} for t in turns],
        "integrated_assertions": [{
            "id": "serves-the-owner",
            "text": "Each reply answers the owner's message in context, keeps what the owner said earlier in the "
                    "thread, does the work asked or says plainly what blocks it, and does not wander off.",
            "severity": "important",
        }],
    }
    set_dir = os.path.join(args.out, args.set)
    os.makedirs(set_dir, exist_ok=True)
    path = os.path.join(set_dir, f"{fixture['id']}.yaml")
    with open(path, "w") as f:
        yaml.safe_dump(fixture, f, sort_keys=False, allow_unicode=True, width=100)
    fixtures = sorted(x for x in os.listdir(set_dir) if x.endswith(".yaml") and x != "suite.yaml")
    with open(os.path.join(set_dir, "suite.yaml"), "w") as f:
        yaml.safe_dump({"name": f"replay-{args.set}", "description": "Real owner threads, names replaced.",
                        "fixtures": fixtures}, f, sort_keys=False)
    print(f"wrote {path} ({len(turns)} owner turns)")
    left = review(set_dir)
    if left:
        print("capitalised words still in the set (add people and companies to --names, then export again):")
        print("  " + ", ".join(f"{w}×{n}" for w, n in left[:80]))


def cmd_push(args):
    if not os.path.isdir(os.path.join(args.out, args.set)):
        sys.exit(f"no set {args.set} under {args.out}")
    rc = subprocess.run(VM_PUSH.format(src=args.out, set=args.set), shell=True).returncode
    if rc:
        sys.exit("push failed")
    print(f"on the VM: ~/harness-replays/{args.set}; run it as suites=\"replays/{args.set}/suite.yaml\"")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    ls = sub.add_parser("list")
    ls.add_argument("--source", default="desktop")
    ls.add_argument("--limit", default=30, type=int)
    ex = sub.add_parser("export")
    ex.add_argument("--source", default="desktop")
    ex.add_argument("--chat", required=True)
    ex.add_argument("--set", required=True)
    ex.add_argument("--turns", type=int, default=0)
    ex.add_argument("--names")
    ex.add_argument("--out", default=OUT)
    pu = sub.add_parser("push")
    pu.add_argument("--set", required=True)
    pu.add_argument("--out", default=OUT)
    args = ap.parse_args()
    if getattr(args, "set", None) and not re.fullmatch(r"[a-z0-9][a-z0-9-]*", args.set):
        sys.exit("--set is lowercase letters, digits and dashes")
    {"list": cmd_list, "export": cmd_export, "push": cmd_push}[args.cmd](args)


if __name__ == "__main__":
    main()
