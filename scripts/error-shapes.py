#!/usr/bin/env python3
"""Every distinct error a tool handed the model, across a traces directory.

Usage:
  scripts/error-shapes.py <traces-dir>                    list shapes, most frequent first
  scripts/error-shapes.py <traces-dir> --baseline FILE    exit 1 on a shape not in FILE
  scripts/error-shapes.py <traces-dir> --baseline FILE --update   rewrite FILE from this run

A shape is an error result with the run-specific parts replaced: ids, paths,
numbers, quoted names. Two runs that hit "Missing required parameter 'x'
for y action" produce one shape. The baseline is the set of shapes we have
already read and either fixed or accepted; a new one means a tool said
something to a model that no one has looked at yet. That is how the
correction suite stops hardening around errors it never notices.
"""
import glob
import json
import os
import re
import sys

PLACEHOLDERS = [
    (re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"), "<uuid>"),
    (re.compile(r"call_[0-9a-f]{16,}"), "<call-id>"),
    (re.compile(r"(?:/[\w .@+-]+){2,}"), "<path>"),
    (re.compile(r"'[^']*'"), "'<x>'"),
    (re.compile(r'"[^"]*"'), '"<x>"'),
    (re.compile(r"\b\d+(?:\.\d+)?\b"), "<n>"),
]
SHAPE_CHARS = 160


def shape(text: str) -> str:
    first = text.strip().splitlines()[0] if text.strip() else ""
    for pattern, token in PLACEHOLDERS:
        first = pattern.sub(token, first)
    return first[:SHAPE_CHARS]


def collect(traces_dir: str) -> dict:
    shapes = {}
    for path in sorted(glob.glob(os.path.join(traces_dir, "*.json"))):
        with open(path) as f:
            trace = json.load(f)
        fixture = trace.get("fixture_id", os.path.basename(path))
        for call in trace.get("tool_calls") or []:
            resp = call.get("response") or {}
            if not resp.get("is_error"):
                continue
            key = f"{call.get('tool')}: {shape(resp.get('content') or '')}"
            entry = shapes.setdefault(key, {"count": 0, "fixtures": set(), "example": resp.get("content", "")[:300]})
            entry["count"] += 1
            entry["fixtures"].add(fixture)
    return shapes


def main() -> None:
    args = sys.argv[1:]
    baseline = None
    update = False
    if "--update" in args:
        update = True
        args.remove("--update")
    if "--baseline" in args:
        i = args.index("--baseline")
        try:
            baseline = args[i + 1]
        except IndexError:
            sys.exit(__doc__)
        del args[i:i + 2]
    if len(args) != 1:
        sys.exit(__doc__)
    shapes = collect(args[0])
    if not shapes:
        print("no error results in", args[0])
    ranked = sorted(shapes.items(), key=lambda kv: -kv[1]["count"])
    print(f"{'count':>5}  {'fixtures':>8}  shape")
    for key, entry in ranked:
        print(f"{entry['count']:>5}  {len(entry['fixtures']):>8}  {key}")
    if baseline is None:
        return
    known = set()
    if os.path.exists(baseline):
        with open(baseline) as f:
            known = {line.rstrip("\n") for line in f if line.strip() and not line.startswith("#")}
    new = [key for key, _ in ranked if key not in known]
    if update:
        with open(baseline, "w") as f:
            f.write("# Error shapes the suite has produced and someone has read. One per line.\n")
            f.write("# A new shape fails the nightly until it is fixed or added here on purpose.\n")
            for key in sorted(known | set(shapes)):
                f.write(key + "\n")
        print(f"\nbaseline written: {len(known | set(shapes))} shapes in {baseline}")
        return
    if new:
        print(f"\n{len(new)} error shape(s) not in {baseline}:")
        for key in new:
            print("  NEW ", key)
            print("       e.g.", shapes[key]["example"].replace("\n", " | ")[:200])
        sys.exit(1)
    print(f"\nno new error shapes ({len(shapes)} seen, all in {baseline})")


if __name__ == "__main__":
    main()
