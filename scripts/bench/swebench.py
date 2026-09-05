#!/usr/bin/env python3
"""SWE-bench Verified runner for the Nebo agent harness.

Selects instances, runs each one sequentially through `nebo-cli test run`
against the live dev server, collects the resulting diff as a prediction, then
scores the predictions with the official swebench harness. The agent edits a
host checkout that is also bind-mounted into the instance's SWE-bench container,
so it can run the project's own tests in the project's own environment.

    bench/.venv/bin/python scripts/bench/swebench.py --ids django__django-11099
    bench/.venv/bin/python scripts/bench/swebench.py --count 10 --seed 1 --model nebo-1
    bench/.venv/bin/python scripts/bench/swebench.py --eval-only bench/runs/<stamp>

Requires docker, the dev server (`make dev`), a built ./target/debug/nebo-cli,
and the `datasets` + `swebench` packages (bench/.venv, see `make bench-swe`).
See docs/sme/TESTING_SME.md section 3c.
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from datasets import load_dataset

DATASET_NAME = "SWE-bench/SWE-bench_Verified"
DATASET_SPLIT = "test"
DOCKER_PLATFORM = "linux/amd64"
CONDA_ACTIVATE = "/opt/miniconda3/bin/activate"
CONDA_ENV = "testbed"
CONTAINER_REPO = "/testbed"
WORK_ROOT = Path("/tmp/nebo-bench")
CONTAINER_PREFIX = "nebo-bench-"
COPY_CONTAINER_SUFFIX = "-copy"
IMAGE_HEAD_MARKER = "image-head"
REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_NEBO_CLI = REPO_ROOT / "target" / "debug" / "nebo-cli"
DEFAULT_RUNS_ROOT = REPO_ROOT / "bench" / "runs"
DEFAULT_SERVER = "localhost:27895"
DEFAULT_TIMEOUT_MIN = 30
DEFAULT_SEED = 1
MODEL_NAME_PREFIX = "nebo:"
DEFAULT_MODEL_LABEL = "default"
PREDICTIONS_FILE = "predictions.jsonl"
RESULTS_FILE = "results.json"
REPORT_JSON = "report.json"
REPORT_MD = "report.md"
HARNESS_LOG = "harness.log"
HARNESS_RUN_LOG_DIR = Path("logs") / "run_evaluation"
HARNESS_MAX_WORKERS = 2
HARNESS_TEST_TIMEOUT_S = 2400
HARNESS_SLACK_S = 600
IDLE_POLL_S = 30
HARNESS_PROCESS_PATTERN = "nebo-cli test run"
HEALTH_TIMEOUT_S = 3
DOCKER_PULL_TIMEOUT_S = 3600
DOCKER_COPY_TIMEOUT_S = 900
DOCKER_TIMEOUT_S = 120
GIT_TIMEOUT_S = 120
TERMINATE_GRACE_S = 15
PATCH_EXCLUDES = (":(exclude)**/__pycache__/**", ":(exclude)*.pyc")
TRACE_RUN_SUFFIX = "_run-1.json"
# nebo-cli exits 0 even when the run failed (2026-09-04: a WS reset mid-run);
# its stdout carries the reason on this line, so the report keeps it.
HARNESS_FAILED_MARKER = "FAILED:"
EXIT_TIMEOUT = "timeout"
VERDICT_RESOLVED = "yes"
VERDICT_UNRESOLVED = "no"
VERDICT_EMPTY = "empty"
VERDICT_ERROR = "error"


def log(message: str) -> None:
    stamp = datetime.now(timezone.utc).strftime("%H:%M:%S")
    print(f"[{stamp}] {message}", flush=True)


def run(
    cmd: list[str],
    timeout_s: int,
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout_s, check=check
    )


def git(workdir: Path, *args: str) -> str:
    return run(["git", "-C", str(workdir), *args], GIT_TIMEOUT_S).stdout


# ─── Selection ───────────────────────────────────────────────────────────────


def load_instances() -> dict[str, dict[str, Any]]:
    log(f"loading {DATASET_NAME} ({DATASET_SPLIT})")
    rows = load_dataset(DATASET_NAME, split=DATASET_SPLIT)
    return {row["instance_id"]: row for row in rows}


def select_instances(
    by_id: dict[str, dict[str, Any]], ids: list[str] | None, count: int | None, seed: int
) -> list[dict[str, Any]]:
    if ids:
        missing = [i for i in ids if i not in by_id]
        if missing:
            raise SystemExit(f"unknown instance ids: {', '.join(missing)}")
        chosen = ids
    else:
        chosen = random.Random(seed).sample(sorted(by_id), count or 0)
    selected = [by_id[i] for i in sorted(chosen)]
    log(f"selected {len(selected)} instance(s): {difficulty_summary(selected)}")
    return selected


def difficulty_counts(rows: list[dict[str, Any]]) -> dict[str, int]:
    return dict(sorted(Counter(row["difficulty"] for row in rows).items()))


def difficulty_summary(rows: list[dict[str, Any]]) -> str:
    return ", ".join(f"{name}: {n}" for name, n in difficulty_counts(rows).items())


# ─── Preconditions ───────────────────────────────────────────────────────────


def check_server(server: str) -> None:
    try:
        urllib.request.urlopen(f"http://{server}/health", timeout=HEALTH_TIMEOUT_S)
    except (urllib.error.URLError, OSError) as exc:
        raise SystemExit(f"no Nebo on {server} ({exc}); start one with 'make dev' first")


def wait_for_idle_harness() -> None:
    while True:
        probe = run(["pgrep", "-fl", HARNESS_PROCESS_PATTERN], DOCKER_TIMEOUT_S, check=False)
        if probe.returncode != 0:
            return
        log(f"another '{HARNESS_PROCESS_PATTERN}' is running, waiting {IDLE_POLL_S}s: {probe.stdout.strip()}")
        time.sleep(IDLE_POLL_S)


# ─── Docker + checkout ───────────────────────────────────────────────────────


def ensure_image(image: str) -> None:
    present = run(["docker", "image", "inspect", image], DOCKER_TIMEOUT_S, check=False)
    if present.returncode == 0:
        log(f"image present: {image}")
        return
    log(f"pulling {image} ({DOCKER_PLATFORM})")
    run(["docker", "pull", "--platform", DOCKER_PLATFORM, image], DOCKER_PULL_TIMEOUT_S)


def container_name(instance_id: str) -> str:
    return f"{CONTAINER_PREFIX}{instance_id}"


def workdir_for(instance_id: str) -> Path:
    return WORK_ROOT / instance_id / CONTAINER_REPO.strip("/")


def image_head(workdir: Path) -> str:
    return (workdir.parent / IMAGE_HEAD_MARKER).read_text().strip()


def workdir_is_pristine(workdir: Path) -> bool:
    marker = workdir.parent / IMAGE_HEAD_MARKER
    if not (workdir / ".git").is_dir() or not marker.is_file():
        return False
    head = git(workdir, "rev-parse", "HEAD").strip()
    dirty = git(workdir, "status", "--porcelain").strip()
    return head == marker.read_text().strip() and not dirty


def prepare_workdir(instance_id: str, image: str) -> Path:
    workdir = workdir_for(instance_id)
    if workdir_is_pristine(workdir):
        log(f"reusing pristine checkout at {workdir}")
        return workdir
    shutil.rmtree(workdir.parent, ignore_errors=True)
    workdir.parent.mkdir(parents=True)
    staging = container_name(instance_id) + COPY_CONTAINER_SUFFIX
    remove_container(staging)
    log(f"copying {CONTAINER_REPO} out of {image} to {workdir}")
    run(
        ["docker", "create", "--platform", DOCKER_PLATFORM, "--name", staging, image],
        DOCKER_TIMEOUT_S,
    )
    try:
        run(
            ["docker", "cp", f"{staging}:{CONTAINER_REPO}", str(workdir.parent)],
            DOCKER_COPY_TIMEOUT_S,
        )
    finally:
        remove_container(staging)
    head = git(workdir, "rev-parse", "HEAD").strip()
    (workdir.parent / IMAGE_HEAD_MARKER).write_text(head + "\n")
    log(f"checkout ready at {workdir} (image HEAD {head[:10]})")
    return workdir


def start_container(instance_id: str, image: str, workdir: Path) -> str:
    name = container_name(instance_id)
    remove_container(name)
    run(
        [
            "docker", "run", "-d", "--platform", DOCKER_PLATFORM, "--name", name,
            "-v", f"{workdir}:{CONTAINER_REPO}", image, "sleep", "infinity",
        ],
        DOCKER_TIMEOUT_S,
    )
    log(f"container {name} up with {workdir} mounted at {CONTAINER_REPO}")
    return name


def remove_container(name: str) -> None:
    run(["docker", "rm", "-f", name], DOCKER_TIMEOUT_S, check=False)


# ─── Fixture + agent run ─────────────────────────────────────────────────────


def fixture_text(row: dict[str, Any], workdir: Path, container: str) -> str:
    instance_id = row["instance_id"]
    exec_form = (
        f"docker exec {container} bash -lc "
        f"'cd {CONTAINER_REPO} && source {CONDA_ACTIVATE} {CONDA_ENV} && <command>'"
    )
    prompt = (
        f"Fix a bug in the {row['repo']} repository.\n\n"
        f"The checkout is at {workdir} (an absolute path on this machine). Edit the files there.\n\n"
        f"The project's Python environment lives in a Docker container named {container}, "
        f"with the same checkout mounted at {CONTAINER_REPO}. To run anything in that environment "
        f"(the test suite, python, pip), use exactly this form:\n\n"
        f"  {exec_form}\n\n"
        "Rules:\n"
        "- Do not commit, do not create branches, and do not run git checkout, reset, stash or clean. "
        "Leave the fix as uncommitted changes in the working tree.\n"
        "- Do not edit the project's existing test files. The fix is graded by running the project's "
        "own tests, including new ones the graders add, so changes to those files are discarded.\n"
        "- Keep the change focused on the issue below.\n"
        "- When you are done, finish with a short summary of what you changed and why.\n\n"
        "The issue, as reported:\n\n"
        f"<issue>\n{row['problem_statement']}\n</issue>"
    )
    description = f"SWE-bench Verified instance {instance_id} ({row['difficulty']}) from {row['repo']}."
    lines = [
        f"id: {json.dumps(instance_id)}",
        f"name: {json.dumps(f'SWE-bench Verified: {instance_id}')}",
        f"description: {json.dumps(description)}",
        "conversation:",
        "  - role: user",
        f"    content: {json.dumps(prompt, ensure_ascii=False)}",
        "",
    ]
    return "\n".join(lines)


def run_agent(
    nebo_cli: Path,
    fixture: Path,
    traces_dir: Path,
    server: str,
    model: str | None,
    timeout_s: int,
    log_path: Path,
) -> tuple[str, float]:
    cmd = [
        str(nebo_cli), "test", "run", "--fixture", str(fixture), "--no-judge",
        "--server", server, "--output", str(traces_dir),
    ]
    if model:
        cmd += ["--model", model]
    log(f"running agent: {' '.join(cmd)}")
    started = time.monotonic()
    with log_path.open("w") as out, subprocess.Popen(
        cmd, cwd=REPO_ROOT, stdout=out, stderr=subprocess.STDOUT
    ) as proc:
        try:
            exit_code = str(proc.wait(timeout=timeout_s))
        except subprocess.TimeoutExpired:
            log(f"agent exceeded {timeout_s}s wall clock, terminating")
            proc.terminate()
            try:
                proc.wait(timeout=TERMINATE_GRACE_S)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            exit_code = EXIT_TIMEOUT
    return exit_code, time.monotonic() - started


def collect_patch(workdir: Path) -> str:
    git(workdir, "add", "-A", "-N")
    return git(workdir, "diff", "--no-color", image_head(workdir), "--", ".", *PATCH_EXCLUDES)


def harness_note(log_path: Path) -> str:
    for line in log_path.read_text().splitlines():
        if HARNESS_FAILED_MARKER in line:
            return line.strip()
    return ""


def read_trace(traces_dir: Path, instance_id: str) -> dict[str, Any]:
    path = traces_dir / f"{instance_id}{TRACE_RUN_SUFFIX}"
    if not path.is_file():
        return {}
    return json.loads(path.read_text())


def run_instance(row: dict[str, Any], args: argparse.Namespace, runs_dir: Path) -> dict[str, Any]:
    instance_id = row["instance_id"]
    image = row["image"]
    log(f"=== {instance_id} ({row['difficulty']})")
    ensure_image(image)
    workdir = prepare_workdir(instance_id, image)
    container = start_container(instance_id, image, workdir)
    try:
        fixture = runs_dir / "fixtures" / f"{instance_id}.yaml"
        fixture.write_text(fixture_text(row, workdir, container))
        traces_dir = runs_dir / "traces"
        agent_log = runs_dir / "agent-logs" / f"{instance_id}.log"
        exit_code, duration_s = run_agent(
            args.nebo_cli, fixture, traces_dir, args.server, args.model,
            args.timeout_min * 60, agent_log,
        )
        patch = collect_patch(workdir)
    finally:
        remove_container(container)
    if args.clean:
        shutil.rmtree(workdir.parent, ignore_errors=True)
    metrics = read_trace(traces_dir, instance_id).get("metrics", {})
    note = harness_note(agent_log)
    log(
        f"agent exit {exit_code} after {duration_s:.0f}s, patch {len(patch)} bytes, "
        f"tool calls {metrics.get('total_tool_calls')}, tokens {metrics.get('total_tokens')}"
        + (f"; {note}" if note else "")
    )
    return {
        "instance_id": instance_id,
        "difficulty": row["difficulty"],
        "image": image,
        "model_name_or_path": model_name(args.model),
        "harness_exit": exit_code,
        "harness_note": note,
        "duration_s": round(duration_s, 1),
        "tool_calls": metrics.get("total_tool_calls"),
        "total_tokens": metrics.get("total_tokens"),
        "input_tokens": metrics.get("input_tokens"),
        "output_tokens": metrics.get("output_tokens"),
        "patch_bytes": len(patch),
        "model_patch": patch,
    }


def model_name(model: str | None) -> str:
    return MODEL_NAME_PREFIX + (model or DEFAULT_MODEL_LABEL)


def save_progress(runs_dir: Path, rows: list[dict[str, Any]]) -> None:
    with (runs_dir / PREDICTIONS_FILE).open("w") as out:
        for row in rows:
            prediction = {
                "instance_id": row["instance_id"],
                "model_name_or_path": row["model_name_or_path"],
                "model_patch": row["model_patch"],
            }
            out.write(json.dumps(prediction) + "\n")
    results = [{k: v for k, v in row.items() if k != "model_patch"} for row in rows]
    (runs_dir / RESULTS_FILE).write_text(json.dumps(results, indent=2) + "\n")


# ─── Evaluation + report ─────────────────────────────────────────────────────


def run_harness(runs_dir: Path, run_id: str, instance_count: int) -> Path:
    cmd = [
        sys.executable, "-m", "swebench.harness.run_evaluation",
        "--dataset_name", DATASET_NAME, "--split", DATASET_SPLIT,
        "--predictions_path", str(runs_dir / PREDICTIONS_FILE),
        "--run_id", run_id, "--max_workers", str(HARNESS_MAX_WORKERS),
        "--timeout", str(HARNESS_TEST_TIMEOUT_S), "--report_dir", str(runs_dir),
    ]
    log(f"running swebench harness: {' '.join(cmd)}")
    wall_s = instance_count * HARNESS_TEST_TIMEOUT_S + HARNESS_SLACK_S
    with (runs_dir / HARNESS_LOG).open("w") as out:
        subprocess.run(cmd, cwd=runs_dir, stdout=out, stderr=subprocess.STDOUT, timeout=wall_s, check=True)
    reports = sorted(runs_dir.glob(f"*.{run_id}.json"))
    if not reports:
        raise SystemExit(f"harness wrote no *.{run_id}.json report under {runs_dir}; see {HARNESS_LOG}")
    return reports[-1]


def verdict_for(instance_id: str, harness_report: dict[str, Any]) -> str:
    if instance_id in harness_report["resolved_ids"]:
        return VERDICT_RESOLVED
    if instance_id in harness_report["unresolved_ids"]:
        return VERDICT_UNRESOLVED
    if instance_id in harness_report["empty_patch_ids"]:
        return VERDICT_EMPTY
    return VERDICT_ERROR


def format_duration(seconds: float) -> str:
    minutes, secs = divmod(int(seconds), 60)
    return f"{minutes}m{secs:02d}s"


def format_count(value: int | None) -> str:
    return "-" if value is None else f"{value:,}"


def write_reports(runs_dir: Path, run_id: str, rows: list[dict[str, Any]], harness_report_path: Path) -> None:
    harness_report = json.loads(harness_report_path.read_text())
    for row in rows:
        row["resolved"] = verdict_for(row["instance_id"], harness_report)
    resolved = sum(row["resolved"] == VERDICT_RESOLVED for row in rows)
    total = len(rows)
    pct = 100.0 * resolved / total if total else 0.0
    totals = {
        "tool_calls": sum(row["tool_calls"] or 0 for row in rows),
        "total_tokens": sum(row["total_tokens"] or 0 for row in rows),
        "duration_s": round(sum(row["duration_s"] for row in rows), 1),
    }
    report = {
        "run_id": run_id,
        "dataset": DATASET_NAME,
        "model_name_or_path": rows[0]["model_name_or_path"] if rows else model_name(None),
        "instances": total,
        "resolved": resolved,
        "resolved_pct": round(pct, 1),
        "difficulty": difficulty_counts(rows),
        "harness_report": harness_report_path.name,
        "totals": totals,
        "rows": rows,
    }
    (runs_dir / REPORT_JSON).write_text(json.dumps(report, indent=2) + "\n")

    lines = [
        f"# SWE-bench Verified, Nebo run {run_id}",
        "",
        f"- Model: `{report['model_name_or_path']}`",
        f"- Dataset: {DATASET_NAME}, {total} of {harness_report['total_instances']} instances in this run",
        f"- Resolved: **{resolved} / {total} ({pct:.1f}%)**",
        f"- Sample difficulty: {difficulty_summary(rows) or 'none'}",
        f"- Harness report: `{harness_report_path.name}`",
        "",
        "| Instance | Difficulty | Resolved | Tool calls | Tokens | Duration | Harness exit | Note |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for row in rows:
        lines.append(
            f"| {row['instance_id']} | {row['difficulty']} | {row['resolved']} | "
            f"{format_count(row['tool_calls'])} | {format_count(row['total_tokens'])} | "
            f"{format_duration(row['duration_s'])} | {row['harness_exit']} | {row['harness_note']} |"
        )
    lines.append(
        f"| **Totals** | | {resolved}/{total} | {format_count(totals['tool_calls'])} | "
        f"{format_count(totals['total_tokens'])} | {format_duration(totals['duration_s'])} | | |"
    )
    lines.append("")
    (runs_dir / REPORT_MD).write_text("\n".join(lines))
    log(f"resolved {resolved}/{total} ({pct:.1f}%); reports at {runs_dir / REPORT_JSON} and {runs_dir / REPORT_MD}")


def evaluate(runs_dir: Path) -> None:
    results_path = runs_dir / RESULTS_FILE
    if not results_path.is_file():
        raise SystemExit(f"no {RESULTS_FILE} in {runs_dir}; run the agent phase first")
    rows: list[dict[str, Any]] = json.loads(results_path.read_text())
    if not rows:
        raise SystemExit(f"{results_path} holds no instances; nothing to evaluate")
    run_id = runs_dir.name
    for image in sorted({row["image"] for row in rows}):
        ensure_image(image)
    stale = runs_dir / HARNESS_RUN_LOG_DIR / run_id
    if stale.is_dir():
        log(f"clearing previous harness logs at {stale} so every instance is re-scored")
        shutil.rmtree(stale)
    harness_report_path = run_harness(runs_dir, run_id, len(rows))
    write_reports(runs_dir, run_id, rows, harness_report_path)


# ─── Entry point ─────────────────────────────────────────────────────────────


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    which = parser.add_mutually_exclusive_group()
    which.add_argument("--ids", nargs="+", metavar="ID", help="instance ids to run")
    which.add_argument("--count", type=int, metavar="N", help="size of a seeded sample over sorted instance ids")
    which.add_argument("--eval-only", metavar="RUNS_DIR", help="re-score an existing runs dir; no agent runs")
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED, help=f"sample seed (default {DEFAULT_SEED})")
    parser.add_argument("--model", help="model passed through to nebo-cli --model")
    parser.add_argument("--runs-dir", type=Path, help=f"output dir (default {DEFAULT_RUNS_ROOT}/<UTC stamp>)")
    parser.add_argument("--server", default=DEFAULT_SERVER, help=f"Nebo server (default {DEFAULT_SERVER})")
    parser.add_argument("--nebo-cli", type=Path, default=DEFAULT_NEBO_CLI, help=f"CLI binary (default {DEFAULT_NEBO_CLI})")
    parser.add_argument("--timeout-min", type=int, default=DEFAULT_TIMEOUT_MIN, help=f"wall clock per instance (default {DEFAULT_TIMEOUT_MIN})")
    parser.add_argument("--clean", action="store_true", help=f"delete {WORK_ROOT}/<id> after each instance")
    parser.add_argument("--skip-eval", action="store_true", help="collect predictions only; do not run the swebench harness")
    args = parser.parse_args()
    if not (args.ids or args.count or args.eval_only):
        parser.error("one of --ids, --count or --eval-only is required")
    return args


def main() -> None:
    args = parse_args()
    if args.eval_only:
        evaluate(Path(args.eval_only).resolve())
        return
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    runs_dir = (args.runs_dir or DEFAULT_RUNS_ROOT / stamp).resolve()
    for sub in ("fixtures", "traces", "agent-logs"):
        (runs_dir / sub).mkdir(parents=True, exist_ok=True)
    log(f"runs dir {runs_dir}")
    if not args.nebo_cli.is_file():
        raise SystemExit(f"{args.nebo_cli} not built (cargo build -p nebo-cli)")
    check_server(args.server)
    instances = select_instances(load_instances(), args.ids, args.count, args.seed)
    wait_for_idle_harness()
    rows: list[dict[str, Any]] = []
    for row in instances:
        rows.append(run_instance(row, args, runs_dir))
        save_progress(runs_dir, rows)
    if args.skip_eval:
        log(f"skipping evaluation; predictions at {runs_dir / PREDICTIONS_FILE}")
        return
    evaluate(runs_dir)


if __name__ == "__main__":
    main()
