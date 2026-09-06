#!/usr/bin/env bash
# Dependency vulnerability scan for Nebo: `make audit`.
#
# Runs on house machines only (a dev box or the stadium runner) before every
# release. Nothing here runs on GitHub-hosted runners.
#
#   cargo audit                       Rust workspace (Cargo.lock vs the RustSec DB)
#   pnpm audit --audit-level=high     app/ (npm advisories)
#
# The target fails on any high or critical finding. A RustSec vulnerability
# with no CVSS score is counted as failing too: "unscored" is not "low", and
# several real bugs (e.g. rustls-webpki name-constraint bypasses) ship without
# a score. RustSec warnings (unmaintained / unsound / yanked) are printed but
# never gate.
#
# Exit codes: 0 clean, 1 findings, 2 a scanner could not run.
set -u
cd "$(dirname "$0")/.."

need() {
	command -v "$1" >/dev/null 2>&1 && return
	echo "audit: $1 not found. Install: $2" >&2
	exit 2
}
need cargo-audit "cargo install cargo-audit --locked"
need pnpm "corepack enable (pnpm is the only package manager we use)"
need node "install Node.js (pnpm needs it; the summary is parsed with it)"

fail=0

# ── cargo audit ──────────────────────────────────────────────────────────────
echo "== cargo audit (Rust workspace) =="
cargo_out=$(cargo audit 2>&1)
cargo_rc=$?
printf '%s\n' "$cargo_out"
if ! grep -q 'Scanning Cargo.lock' <<<"$cargo_out"; then
	echo "audit: cargo audit did not complete (exit $cargo_rc)" >&2
	exit 2
fi

# One text block per (crate, version). Vulnerabilities carry an ID and, when
# the advisory has a CVSS score, "Severity: N.N (level)". Warning blocks carry
# "Warning:" and are skipped. Same advisory across several crate versions
# counts once.
cargo_vulns=$(awk '
	/^Crate:/    { id = ""; sev = "unscored"; warn = 0 }
	/^Warning:/  { warn = 1 }
	/^ID:/       { id = $2 }
	/^Severity:/ { sev = $3; gsub(/[()]/, "", sev) }
	/^$/         { if (id != "" && !warn) print sev, id; id = "" }
	END          { if (id != "" && !warn) print sev, id }
' <<<"$cargo_out" | sort -u)
cargo_warnings=$(grep -c '^Warning:' <<<"$cargo_out")

count() { grep -c "^$1 " <<<"$2"; }
cargo_total=$( [ -n "$cargo_vulns" ] && wc -l <<<"$cargo_vulns" | tr -d ' ' || echo 0)
cargo_critical=$(count critical "$cargo_vulns")
cargo_high=$(count high "$cargo_vulns")
cargo_medium=$(count medium "$cargo_vulns")
cargo_low=$(count low "$cargo_vulns")
cargo_unscored=$(count unscored "$cargo_vulns")
cargo_gate=$(grep -E '^(critical|high|unscored) ' <<<"$cargo_vulns" | awk '{print $2}' | sort -u)
[ -n "$cargo_gate" ] && fail=1

# ── pnpm audit ───────────────────────────────────────────────────────────────
echo
echo "== pnpm audit --audit-level=high (app/) =="
pnpm_err=$(mktemp)
pnpm_out=$(cd app && pnpm audit --audit-level=high --json 2>"$pnpm_err")
pnpm_rc=$?
pnpm_summary=$(node -e '
	let j;
	try { j = JSON.parse(require("fs").readFileSync(0, "utf8")); } catch (e) { console.log("ERROR pnpm audit produced no JSON"); process.exit(2); }
	if (j.error) { console.log("ERROR " + j.error.code + ": " + j.error.message); process.exit(2); }
	const v = j.metadata.vulnerabilities;
	const gate = Object.values(j.advisories || {}).filter(a => a.severity === "high" || a.severity === "critical");
	console.log(`critical ${v.critical}  high ${v.high}  moderate ${v.moderate}  low ${v.low}  (${j.metadata.totalDependencies} dependencies)`);
	for (const a of gate) {
		const versions = [...new Set(a.findings.map(f => f.version))].join(",");
		const via = a.findings.flatMap(f => f.paths)[0] || "";
		console.log(`  ${a.severity.padEnd(8)} ${a.github_advisory_id}  ${a.module_name}@${versions}  ${(a.cves || []).join(",") || "no CVE"}  ${a.title}  [${via}]`);
	}
	process.exit(gate.length ? 1 : 0);
' <<<"$pnpm_out")
pnpm_gate_rc=$?
printf '%s\n' "$pnpm_summary"
if [ "$pnpm_gate_rc" -eq 2 ]; then
	cat "$pnpm_err" >&2
	rm -f "$pnpm_err"
	echo "audit: pnpm audit did not complete (exit $pnpm_rc)" >&2
	exit 2
fi
rm -f "$pnpm_err"
[ "$pnpm_gate_rc" -eq 1 ] && fail=1

# ── summary ──────────────────────────────────────────────────────────────────
echo
echo "== audit summary =="
echo "cargo audit (Cargo.lock): $cargo_total vulnerable advisories: critical $cargo_critical, high $cargo_high, medium $cargo_medium, low $cargo_low, unscored $cargo_unscored; $cargo_warnings warnings (unmaintained/unsound/yanked, not gated)"
[ -n "$cargo_gate" ] && echo "  gating: $(tr '\n' ' ' <<<"$cargo_gate")"
echo "pnpm audit (app/): $(head -1 <<<"$pnpm_summary")"
if [ "$pnpm_gate_rc" -eq 1 ]; then
	echo "  gating: $(tail -n +2 <<<"$pnpm_summary" | awk '{print $2}' | tr '\n' ' ')"
fi
if [ "$fail" -ne 0 ]; then
	echo "RESULT: FAIL (high/critical findings)"
	exit 1
fi
echo "RESULT: PASS"
