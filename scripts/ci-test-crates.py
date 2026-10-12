#!/usr/bin/env python3
"""Pick the cargo test set for a pull request: the crates its diff touches,
every workspace crate that depends on them (transitively), and the crates
whose tests read files outside their own folder. ci.yml runs it on
pull_request only; a push to main always runs the whole workspace.

  python3 scripts/ci-test-crates.py BASE         # diff BASE..HEAD
  python3 scripts/ci-test-crates.py --files A B  # explicit changed paths
  python3 scripts/ci-test-crates.py --self-check

Prints the decision and, under Actions, writes `mode` (full | some | none)
and `args` (the cargo test package arguments) to $GITHUB_OUTPUT.

Anything this script does not know is treated as a workspace-wide change:
an unmapped path runs the full suite, never fewer tests.
"""
import os
import subprocess
import sys
import tomllib

FULL_ARGS = "--workspace --exclude nebo"
# The desktop crate: no tests, and its build script wants the real frontend.
# The full run excludes it; so does every selection.
EXCLUDED = {"nebo"}

# Any change here can change every crate's build or this job: full suite.
WORKSPACE_WIDE = (
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain",
    ".cargo/", ".github/workflows/ci.yml", "scripts/ci-test-crates.py",
    "etc/",      # etc/nebo.yaml is compiled into nebo-config (include_bytes)
    "assets/",   # assets/cloud-bot/* is compiled into nebo-tools (include_str)
)

# Tests that scan the whole tree (crates/*/src, src-tauri/src, app/src) for
# drift: nebo-tls (no outbound client outside it), nebo-tools
# (architecture_drift, rename_map, registry scans agent/src), nebo-agent
# (nothing_reads_the_old_columns). Any change under crates/ or src-tauri/ runs them.
TREE_GUARDS = {"nebo-tls", "nebo-tools", "nebo-agent"}

# Files outside crates/ that tests read at run time -> the crates whose tests
# read them (plus their dependents, when the file is compiled in).
DATA_INPUTS = (
    ("proto/", {"nebo-proto"}, True),             # crates/proto/build.rs compiles them
    ("suites/", {"nebo-agent", "nebo-server", "nebo-cli"}, False),
    ("fixtures/", {"nebo-agent", "nebo-server", "nebo-cli"}, False),
    ("app/src/", {"nebo-tools"}, False),          # rename_map scans app/src
    ("app/static/", {"nebo-tools"}, False),       # bundled skill checks the SDK bundle
    (".github/workflows/harness-gate.yml", {"nebo-agent"}, False),
    ("scripts/gate-", {"nebo-agent"}, False),     # testing/isolation.rs runs gate-run.sh
)

# No Rust build or test reads these.
NO_RUST = (
    "app/", "docs/", "issues/", "bench/", "docker/", "vm/", ".github/", "scripts/",
    "Dockerfile", "Makefile", "LICENSE", "NOTICE", ".gitignore", ".dockerignore",
)


def workspace(root="."):
    """{package name: (dir, set of workspace package names it depends on)}."""
    with open(os.path.join(root, "Cargo.toml"), "rb") as f:
        root_ws = tomllib.load(f)["workspace"]
    members, shared = root_ws["members"], root_ws.get("dependencies", {})
    raw = {}
    for m in members:
        with open(os.path.join(root, m, "Cargo.toml"), "rb") as f:
            t = tomllib.load(f)
        tables = [t.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        for target in t.get("target", {}).values():
            tables += [target.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
        deps = set()
        for table in tables:
            for key, spec in table.items():
                if isinstance(spec, dict) and spec.get("workspace"):
                    spec = shared.get(key, {})  # `x = { workspace = true }`
                deps.add(spec.get("package", key) if isinstance(spec, dict) else key)
        raw[t["package"]["name"]] = (m.rstrip("/") + "/", deps)
    names = set(raw)
    return {n: (d, deps & names) for n, (d, deps) in raw.items()}


def dependents(ws, seeds):
    """seeds plus every package that depends on one of them, transitively."""
    out, todo = set(seeds), list(seeds)
    while todo:
        cur = todo.pop()
        for name, (_, deps) in ws.items():
            if cur in deps and name not in out:
                out.add(name)
                todo.append(name)
    return out


def select(ws, files):
    """('full' | 'some' | 'none', packages, reasons)."""
    pkgs, reasons = set(), []
    by_dir = sorted(((d, n) for n, (d, _) in ws.items()), key=lambda x: -len(x[0]))
    for f in files:
        if f.startswith(WORKSPACE_WIDE) or f in WORKSPACE_WIDE:
            return "full", set(), [f"{f}: workspace-wide input"]
        owner = next((n for d, n in by_dir if f.startswith(d)), None)
        if owner:
            pkgs |= dependents(ws, {owner}) | TREE_GUARDS
            continue
        if f.startswith(("crates/", "src-tauri/")):
            # In the tree the guards scan, but in no package (crates/a2ui/README...).
            # src-tauri is the nebo package, so it always has an owner.
            return "full", set(), [f"{f}: under crates/ but in no package"]
        data = next(((p, c, code) for p, c, code in DATA_INPUTS if f.startswith(p)), None)
        if data:
            pkgs |= dependents(ws, data[1]) if data[2] else set(data[1])
            continue
        if f.startswith(NO_RUST) or (f.endswith(".md") and "/" not in f):
            reasons.append(f"{f}: no Rust input")
            continue
        return "full", set(), [f"{f}: unmapped path, treated as workspace-wide"]
    pkgs -= EXCLUDED
    if not pkgs:
        return "none", set(), reasons
    if pkgs >= set(ws) - EXCLUDED:
        return "full", set(), ["every testable package is selected"]
    return "some", pkgs, reasons


def changed_files(base):
    # --no-renames: a file moved out of a crate shows its old path too.
    out = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", base, "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout
    return [l for l in out.splitlines() if l]


def self_check():
    ws = workspace()
    rd = dependents(ws, {"nebo-tools"})
    assert {"nebo-tools", "nebo-agent", "nebo-workflow", "nebo-server", "nebo-cli"} <= rd, rd
    assert "nebo-db" not in rd and "nebo-types" not in rd, rd
    mode, pkgs, _ = select(ws, ["crates/tools/src/lib.rs"])
    assert mode == "some" and {"nebo-tools", "nebo-server", "nebo-cli", "nebo-tls"} <= pkgs and "nebo" not in pkgs, (mode, pkgs)
    assert select(ws, ["crates/a2ui/a2ui-core/src/lib.rs"])[1] >= {"a2ui-core", "nebo-server"}
    assert select(ws, ["app/package.json", "app/e2e/x.ts", "docs/x.md", "README.md"])[0] == "none"
    assert select(ws, ["app/src/routes/+page.svelte"])[1] == {"nebo-tools"}
    for f in ("Cargo.lock", "Cargo.toml", ".cargo/config.toml", ".github/workflows/ci.yml", "proto2/x", "crates/x.txt"):
        assert select(ws, [f])[0] == "full", f
    assert select(ws, ["crates/types/src/lib.rs"])[1] >= {"nebo-types", "nebo-db", "nebo-server"}
    assert select(ws, [".github/workflows/release.yml"])[0] == "none"
    # The TOML graph must match cargo's own, where cargo is at hand.
    try:
        import json
        meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                                         check=True, capture_output=True, text=True).stdout)
    except (OSError, subprocess.CalledProcessError):
        print("self-check: cargo not available, graph cross-check skipped")
    else:
        names = {p["name"] for p in meta["packages"]}
        cargo = {p["name"]: {d["name"] for d in p["dependencies"]} & names for p in meta["packages"]}
        assert cargo == {n: deps for n, (_, deps) in ws.items()}, "dependency graph differs from cargo metadata"
    print("self-check OK")


def main(argv):
    if argv[:1] == ["--self-check"]:
        return self_check()
    files = argv[1:] if argv[:1] == ["--files"] else changed_files(argv[0])
    mode, pkgs, reasons = select(workspace(), files)
    args = FULL_ARGS if mode == "full" else " ".join(f"-p {p}" for p in sorted(pkgs))
    print(f"{len(files)} changed file(s)")
    for r in reasons:
        print("  " + r)
    if mode == "none":
        print("No Rust crate or Rust test input changed: cargo tests skipped.")
    elif mode == "full":
        print(f"Full suite: cargo test {args}")
    else:
        print(f"Selected {len(pkgs)} package(s): " + " ".join(sorted(pkgs)))
        print(f"cargo test {args}")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as f:
            f.write(f"mode={mode}\nargs={args}\n")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
