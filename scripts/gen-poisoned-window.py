#!/usr/bin/env python3
"""Seed a fully SYNTHETIC poisoned conversation window into a Nebo DB.

Reproduces the forged-tool-result outage shape offline: a long window of
assistant tool_calls whose results were rewritten to the literal string
"[os] 0 lines", interleaved with "I'm stuck in a loop" narration about a file
under /tmp/nebo-replay/. No customer data — every byte is generated here.

Writes:
  - /tmp/nebo-replay/project/main.py  (~600 lines of real, generated Python)
  - one `chats` row + poisoned `chat_messages` rows in the target DB

Used by scripts/test-replay.sh (make test-replay).
"""

import argparse
import json
import sqlite3
import sys
import time
import uuid


def build_main_py(path: str) -> int:
    """Generate ~600 lines of real (valid, runnable) Python at `path`."""
    lines = [
        '"""Task-queue simulator — synthetic fixture program for nebo test-replay.',
        "",
        "Simulates a bounded work queue with retrying workers and a tiny",
        "scheduler. Generated code: real, runnable, and entirely synthetic.",
        '"""',
        "",
        "import random",
        "import time",
        "",
        "QUEUE_DEPTH_LIMIT = 128",
        "RETRY_LIMIT = 3",
        "WORKER_COUNT = 8",
        "TICK_SECONDS = 0.01",
        "",
        "",
        "class Task:",
        '    """One unit of simulated work."""',
        "",
        "    def __init__(self, task_id, payload, priority=0):",
        "        self.task_id = task_id",
        "        self.payload = payload",
        "        self.priority = priority",
        "        self.attempts = 0",
        "        self.done = False",
        "",
        "    def __repr__(self):",
        '        return f"Task({self.task_id}, prio={self.priority}, attempts={self.attempts})"',
        "",
        "",
        "class Queue:",
        '    """Priority-ordered bounded queue."""',
        "",
        "    def __init__(self, limit=QUEUE_DEPTH_LIMIT):",
        "        self.items = []",
        "        self.limit = limit",
        "        self.dropped = 0",
        "",
        "    def push(self, task):",
        "        if len(self.items) >= self.limit:",
        "            self.dropped += 1",
        "            return False",
        "        self.items.append(task)",
        "        self.items.sort(key=lambda t: -t.priority)",
        "        return True",
        "",
        "    def pop(self):",
        "        return self.items.pop(0) if self.items else None",
        "",
    ]
    # 44 small generated stage functions (~12 lines each) — the bulk of the file.
    for n in range(1, 45):
        lines += [
            "",
            f"def stage_{n:02d}(task, rng):",
            f'    """Simulated processing stage {n:02d}: mixes payload state."""',
            f"    salt = {n * 7919}",
            "    acc = salt",
            "    for ch in str(task.payload):",
            "        acc = (acc * 31 + ord(ch)) % 1000003",
            f"    jitter = rng.randrange({n + 1})",
            "    task.payload = (acc + jitter) % 999983",
            f"    if acc % {n + 3} == 0:",
            "        task.priority += 1",
            "    return acc",
        ]
    lines += [
        "",
        "",
        "STAGES = [",
    ]
    for n in range(1, 45):
        lines.append(f"    stage_{n:02d},")
    lines += [
        "]",
        "",
        "",
        "def run_worker(worker_id, queue, rng, results):",
        '    """Drain the queue through every stage, retrying failures."""',
        "    while True:",
        "        task = queue.pop()",
        "        if task is None:",
        "            return",
        "        task.attempts += 1",
        "        try:",
        "            for stage in STAGES:",
        "                stage(task, rng)",
        "            task.done = True",
        "            results.append((worker_id, task.task_id, task.payload))",
        "        except Exception:",
        "            if task.attempts < RETRY_LIMIT:",
        "                queue.push(task)",
        "",
        "",
        "def main():",
        "    rng = random.Random(45817)",
        "    queue = Queue()",
        "    for i in range(96):",
        '        queue.push(Task(f"task-{i:03d}", i * 17, priority=i % 5))',
        "    results = []",
        "    start = time.time()",
        "    for w in range(WORKER_COUNT):",
        "        run_worker(w, queue, rng, results)",
        "    elapsed = time.time() - start",
        '    print(f"processed={len(results)} dropped={queue.dropped} elapsed={elapsed:.3f}s")',
        "",
        "",
        'if __name__ == "__main__":',
        "    main()",
        "",
    ]
    with open(path, "w") as f:
        f.write("\n".join(lines))
    return len(lines)


def seed(db_path, chat_id, session_key, title, main_py, pairs, narration):
    conn = sqlite3.connect(db_path, timeout=30)
    conn.execute("PRAGMA busy_timeout = 30000")

    now = int(time.time())
    base = now - 7200  # window starts two hours ago
    t = [base]  # mutable timestamp counter

    def next_ts():
        t[0] += 1
        return t[0]

    rows = []  # (id, role, content, tool_calls, tool_results, created_at)

    def add(role, content, tool_calls=None, tool_results=None):
        rows.append((str(uuid.uuid4()), role, content, tool_calls, tool_results, next_ts()))

    add("user", f"can you read {main_py} and tell me what it does?")

    narration_text = (
        "I'm stuck in a loop. Every read of "
        f"{main_py} keeps coming back as \"[os] 0 lines\" — the file appears "
        "to be empty no matter how I read it. I can't read this file. "
        "Trying the read again."
    )

    narrated = 0
    for i in range(pairs):
        call_id = f"call_read_{i:04d}"
        tc = json.dumps([{
            "id": call_id,
            "name": "os",
            "input": {"resource": "file", "action": "read", "path": main_py},
        }])
        tr = json.dumps([{"tool_call_id": call_id, "content": "[os] 0 lines"}])
        add("assistant", "", tool_calls=tc)
        add("tool", "[os] 0 lines", tool_results=tr)
        # interleave ~`narration` stuck-loop narration turns across the window
        if narrated < narration and i % max(1, pairs // narration) == 0:
            add("assistant", narration_text)
            narrated += 1

    with conn:
        conn.execute(
            "INSERT INTO chats (id, title, session_name, user_id, created_at, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (chat_id, title, session_key, "replay", base, now),
        )
        for mid, role, content, tc, tr, ts in rows:
            conn.execute(
                "INSERT INTO chat_messages "
                "(id, chat_id, role, content, tool_calls, tool_results, token_estimate, created_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (mid, chat_id, role, content, tc, tr,
                 (len(content) + len(tc or "") + len(tr or "")) // 4, ts),
            )
    conn.close()
    return len(rows), narrated


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True, help="path to nebo.db")
    ap.add_argument("--chat-id", required=True, help="bare UUID; becomes chats.id")
    ap.add_argument("--session-key", required=True,
                    help="thread session key, e.g. agent:assistant:thread:<uuid>")
    ap.add_argument("--title", required=True)
    ap.add_argument("--main-py", default="/tmp/nebo-replay/project/main.py")
    ap.add_argument("--pairs", type=int, default=110,
                    help="forged tool_call/tool_result pairs")
    ap.add_argument("--narration", type=int, default=50,
                    help="stuck-in-a-loop assistant narration turns")
    args = ap.parse_args()

    import os
    os.makedirs(os.path.dirname(args.main_py), exist_ok=True)
    file_lines = build_main_py(args.main_py)

    msg_count, narrated = seed(
        args.db, args.chat_id, args.session_key, args.title,
        args.main_py, args.pairs, args.narration,
    )
    print(json.dumps({
        "chat_id": args.chat_id,
        "session_key": args.session_key,
        "messages_seeded": msg_count,
        "forged_results": args.pairs,
        "narration_turns": narrated,
        "main_py_lines": file_lines,
    }))
    return 0


if __name__ == "__main__":
    sys.exit(main())
