#!/usr/bin/env python3
"""Correction rate over a traces directory.

Usage: scripts/correction-rate.py <traces-dir> <suite.yaml> [--min PCT]

With --min, exit 1 when the correction rate is below PCT percent, so a CI
job can gate on it.

A trace (nebo-cli test run --output DIR) records each graded assertion as
{id, passed}. The fixture says which category and severity that id has. The
correction rate is the pass rate of critical `recovery` assertions; the
first-call rate is shown beside it so the two can be compared.
"""
import glob
import json
import os
import sys

try:
    import yaml
except ImportError:  # ponytail: pyyaml is a dev dependency of the harness already
    sys.exit("pyyaml is required: pip install pyyaml")


def load_suite(path):
    suite = yaml.safe_load(open(path))
    base = os.path.dirname(os.path.abspath(path))
    fixtures = {}
    for rel in suite["fixtures"]:
        fx = yaml.safe_load(open(os.path.join(base, rel)))
        table = {}
        for category, items in (fx.get("prompt_assertions") or {}).items():
            for a in items or []:
                table[a["id"]] = (category, a.get("severity", "important"))
        # Errors the fixture induces on purpose (a missing file, a refused
        # command) are the test, not a failure; `induced_errors: N` in the
        # fixture says how many to forgive per run.
        fixtures[fx["id"]] = {"table": table, "induced": int(fx.get("induced_errors", 0) or 0)}
    return fixtures


def main():
    args = sys.argv[1:]
    minimum = None
    if "--min" in args:
        i = args.index("--min")
        try:
            minimum = float(args[i + 1])
        except (IndexError, ValueError):
            sys.exit(__doc__)
        del args[i:i + 2]
    if len(args) != 2:
        sys.exit(__doc__)
    traces_dir, suite_path = args
    fixtures = load_suite(suite_path)
    tally = {}  # fixture -> {"recovery": [passed, total], "first_call": [p, t]}
    for f in sorted(glob.glob(os.path.join(traces_dir, "*.json"))):
        d = json.load(open(f))
        entry = fixtures.get(d.get("fixture_id"))
        if not entry:
            continue
        table, induced = entry["table"], entry["induced"]
        t = tally.setdefault(d["fixture_id"], {"recovery": [0, 0], "first_call": [0, 0], "runs": 0, "calls": [0, 0]})
        t["runs"] += 1
        # Call-level first-attempt acceptance: every call the model made that
        # did not come back as an error. The strictest of the three numbers,
        # and the one a spiral shows up in first.
        calls = d.get("tool_calls") or []
        errors = sum(1 for c in calls if (c.get("response") or {}).get("is_error"))
        t["calls"][1] += len(calls)
        t["calls"][0] += len(calls) - max(0, errors - induced)
        for a in (d.get("grade") or {}).get("assertions") or []:
            cat, sev = table.get(a["id"], (None, None))
            if cat not in ("recovery", "first_call") or sev != "critical":
                continue
            t[cat][1] += 1
            t[cat][0] += 1 if a.get("passed") else 0
    if not tally:
        sys.exit(f"no graded traces for this suite in {traces_dir}")
    rec = [0, 0]
    fc = [0, 0]
    calls = [0, 0]
    print(f"{'fixture':40} {'runs':>4} {'correction':>11} {'first call':>11} {'calls ok':>10}")
    for fid, t in tally.items():
        r, f1, c = t["recovery"], t["first_call"], t["calls"]
        rec[0] += r[0]; rec[1] += r[1]; fc[0] += f1[0]; fc[1] += f1[1]; calls[0] += c[0]; calls[1] += c[1]
        rate = lambda p: f"{p[0]}/{p[1]}" if p[1] else "n/a"
        print(f"{fid:40} {t['runs']:>4} {rate(r):>11} {rate(f1):>11} {rate(c):>10}")
    pct = lambda p: f"{100 * p[0] / p[1]:.0f}%" if p[1] else "n/a"
    print(f"\ncorrection rate {pct(rec)} ({rec[0]}/{rec[1]} critical recovery assertions)")
    print(f"first-call rate {pct(fc)} ({fc[0]}/{fc[1]} critical first-call assertions)")
    print(f"call acceptance {pct(calls)} ({calls[0]}/{calls[1]} tool calls accepted on the first attempt, induced errors forgiven)")
    if minimum is not None and rec[1] and 100 * rec[0] / rec[1] < minimum:
        sys.exit(f"correction rate below the {minimum:.0f}% gate")


if __name__ == "__main__":
    main()
