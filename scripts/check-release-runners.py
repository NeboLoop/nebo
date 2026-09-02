#!/usr/bin/env python3
"""Release builds run on the house Mac mini ("stadium"), never on GitHub's
hosted runners. This check exists because it has been undone before (Aug 15-16
2026: CI and the Mac release builds were moved to macos-latest/ubuntu-latest
and releases took 70+ minutes on 4-core rented machines). Windows is the only
build allowed on a hosted runner, and only until a Windows box is registered.

Run locally: python3 scripts/check-release-runners.py
"""
import sys
import yaml

WF = ".github/workflows/release.yml"
MAC_JOBS = ("build-macos", "notarize-macos", "publish-macos")


def labels(v):
    if isinstance(v, list):
        return [str(x) for x in v]
    return [str(v)]


def main():
    with open(WF) as f:
        jobs = yaml.safe_load(f)["jobs"]
    errors = []
    for name in MAC_JOBS:
        runs_on = labels(jobs[name].get("runs-on"))
        if "stadium-mac" not in runs_on:
            errors.append(f"{name}: runs-on {runs_on} — must be the house Mac mini ([self-hosted, macOS, stadium-mac])")
    for leg in jobs["build-linux"]["strategy"]["matrix"]["include"]:
        runs_on = labels(leg.get("runner"))
        if "self-hosted" not in runs_on:
            errors.append(f"build-linux {leg.get('arch')}: runner {runs_on} — must be the Mac mini's Lima VM ([self-hosted, Linux, ARM64, neboloop])")
    for name, job in jobs.items():
        runs_on = " ".join(labels(job.get("runs-on")))
        if "macos-" in runs_on:
            errors.append(f"{name}: uses a GitHub-hosted macOS runner ({runs_on})")
    if errors:
        print("Release builds must run on the house Mac mini (see CLAUDE.md, 'Release builds'):")
        for e in errors:
            print("  -", e)
        return 1
    print("release runners OK: Mac + Linux on the house Mac mini, Windows hosted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
