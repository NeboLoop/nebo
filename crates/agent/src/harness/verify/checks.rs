//! The project's own build and tests: the plain-code step of every round.
//!
//! Read from the project's manifests, never guessed by a model: the same
//! project always runs the same checks. Every project is compiled whole
//! (every package and target, so a dependent the change breaks fails the
//! round) and then tested where the change is. Each check runs in the copy
//! the work happens in, with the project's one build folder
//! ([`Check::with_build_dir`], [`build_cache`]): every verification of a
//! project shares it, so a round never starts cold and the disk holds one
//! build output per project, not one per round.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One check command for a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// What the owner reads: "Rust tests", "pnpm check".
    pub label: String,
    /// The command, run in `dir`.
    pub command: String,
    /// Where it runs: the project folder, or one of its packages.
    pub dir: PathBuf,
}

/// The checks for the project at `root`, scoped to the packages `changed`
/// (paths relative to `root`) touch where the project is a workspace.
/// Empty when the project declares none: the round then says no build or
/// test could run, and the work is not verified.
pub fn detect(root: &Path, changed: &[String]) -> Vec<Check> {
    let mut checks = Vec::new();
    if root.join("Cargo.toml").is_file() {
        checks.extend(cargo(root, changed));
    }
    if root.join("package.json").is_file() {
        checks.extend(node(root, root));
    } else {
        // A repository whose web app is a package of its own (`app/`).
        for dir in changed_package_dirs(root, changed, "package.json") {
            checks.extend(node(root, &dir));
        }
    }
    if root.join("go.mod").is_file() {
        checks.push(Check {
            label: "Go build and tests".into(),
            command: "go build ./... && go test ./...".into(),
            dir: root.to_path_buf(),
        });
    }
    let python = root.join("pyproject.toml").is_file()
        || root.join("setup.py").is_file()
        || root.join("pytest.ini").is_file();
    if python
        && (root.join("tests").is_dir()
            || root.join("pytest.ini").is_file()
            || has_pytest_files(root))
    {
        checks.push(Check {
            label: "Python tests".into(),
            command: "python3 -m pytest -q".into(),
            dir: root.to_path_buf(),
        });
    }
    if root.join("pubspec.yaml").is_file() {
        checks.push(Check {
            label: "Flutter analyze".into(),
            command: "flutter analyze".into(),
            dir: root.to_path_buf(),
        });
        if root.join("test").is_dir() {
            checks.push(Check {
                label: "Flutter tests".into(),
                command: "flutter test".into(),
                dir: root.to_path_buf(),
            });
        }
    }
    if checks.is_empty() && makefile_has(root, "test") {
        checks.push(Check {
            label: "make test".into(),
            command: "make test".into(),
            dir: root.to_path_buf(),
        });
    }
    checks
}

/// The build folder every verification of the project at `root` shares:
/// `<copies>/build/<hash of the project's path>` ([`builds_root`]).
pub fn build_cache(root: &Path) -> Result<PathBuf, String> {
    use sha2::Digest;
    let hash = hex::encode(&sha2::Sha256::digest(root.to_string_lossy().as_bytes())[..8]);
    builds_root().map(|b| b.join(hash)).ok_or_else(|| "no folder for the build: the working-copy folder could not be made".to_string())
}

/// Where the projects' build folders are: with the working copies
/// (`config::copies_dir`), where every command that builds may write.
pub fn builds_root() -> Option<PathBuf> {
    config::copies_dir().ok().map(|c| c.join("build"))
}

/// What one check did in a round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub label: String,
    pub command: String,
    pub passed: bool,
    /// The last lines of its output.
    pub tail: String,
    pub duration_ms: u64,
    /// Each problem a failed check names one by one (the live check's
    /// errors); empty for a build or tests, whose tail says what failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl CheckRun {
    /// The open problems this run counts toward the loop's progress: each
    /// error it names, else one for a failed check; none when it passed.
    pub fn problems(&self) -> usize {
        if self.passed { 0 } else { self.errors.len().max(1) }
    }
}

/// The most of a check's output a round keeps.
pub const TAIL_CHARS: usize = 4_000;

/// The end of `output`, at most [`TAIL_CHARS`], on a line boundary.
pub fn tail(output: &str) -> String {
    let output = output.trim_end();
    if output.chars().count() <= TAIL_CHARS {
        return output.to_string();
    }
    let start = output
        .char_indices()
        .rev()
        .nth(TAIL_CHARS - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let cut = &output[start..];
    let cut = cut.find('\n').map(|i| &cut[i + 1..]).unwrap_or(cut);
    format!("[… earlier output left out]\n{cut}")
}

/// The longest one check runs before it is stopped and counted failed.
pub const CHECK_TIMEOUT_MS: u64 = 600_000;

/// Run `checks` one after another through the one door every command
/// takes (`run_command` in the registry, under the run's permissions in
/// `ctx`), each in its folder with the project's build folder. A check the
/// permission check refuses, or that outlasts [`CHECK_TIMEOUT_MS`], failed.
pub async fn run(tools: &tools::Registry, ctx: &tools::ToolContext, checks: &[Check], build_dir: &Path) -> Vec<CheckRun> {
    let mut out = Vec::with_capacity(checks.len());
    for check in checks {
        let started = std::time::Instant::now();
        let mut ctx = ctx.clone();
        ctx.cwd = Some(check.dir.to_string_lossy().into_owned());
        // Nobody answers a question in the middle of a check.
        ctx.cannot_wait = true;
        let input = serde_json::json!({
            "command": check.with_build_dir(build_dir),
            "description": format!("Checking: {}", check.label),
            "cwd": check.dir.to_string_lossy(),
            "timeout": CHECK_TIMEOUT_MS,
        });
        let result = tools.execute(&ctx, "run_command", input).await;
        let mut passed = !result.is_error;
        let mut text = result.content.clone();
        if let Some(id) = backgrounded(&text) {
            let _ = tools.execute(&ctx, "stop_task", serde_json::json!({ "task_id": id })).await;
            passed = false;
            text = format!("It ran past {} minutes and was stopped.", CHECK_TIMEOUT_MS / 60_000);
        }
        out.push(CheckRun {
            label: check.label.clone(),
            command: check.command.clone(),
            passed,
            tail: tail(&text),
            duration_ms: started.elapsed().as_millis() as u64,
            errors: Vec::new(),
        });
    }
    out
}

/// The id of a command that outlasted its timeout and moved to the
/// background (`run_command` says "…moved to the background with ID: bg-…").
fn backgrounded(text: &str) -> Option<String> {
    let at = text.find("moved to the background with ID: ")? + "moved to the background with ID: ".len();
    let id: String = text[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (!id.is_empty()).then_some(id)
}

impl Check {
    /// The environment prefix that gives the check its own build folder,
    /// `build_dir`, where the tool has one.
    pub fn with_build_dir(&self, build_dir: &Path) -> String {
        if self.command.starts_with("cargo ") {
            format!(
                "CARGO_TARGET_DIR={} {}",
                shell_quote(&build_dir.to_string_lossy()),
                self.command
            )
        } else {
            self.command.clone()
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `cargo test` for the crates the change touches (all of them when it
/// touches the workspace's own files).
fn cargo(root: &Path, changed: &[String]) -> Vec<Check> {
    let mut packages: Vec<String> = Vec::new();
    let mut whole = false;
    for path in changed
        .iter()
        .filter(|p| p.ends_with(".rs") || p.ends_with("Cargo.toml") || p.ends_with(".sql"))
    {
        match package_of(root, path) {
            Some(name) => {
                if !packages.contains(&name) {
                    packages.push(name);
                }
            }
            None => whole = true,
        }
    }
    // The whole workspace compiles, every target: a crate that depends on
    // the change and no longer builds fails the round.
    let compile = Check {
        label: "Rust build".into(),
        command: "cargo check --workspace --all-targets".into(),
        dir: root.to_path_buf(),
    };
    if packages.is_empty() && !whole {
        return vec![compile];
    }
    let scope = if whole || packages.is_empty() {
        "--workspace".to_string()
    } else {
        packages
            .iter()
            .map(|p| format!("-p {p}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    vec![
        compile,
        Check {
            label: "Rust tests".into(),
            command: format!("cargo test {scope}"),
            dir: root.to_path_buf(),
        },
    ]
}

/// The package that owns `path`: the nearest `Cargo.toml` with a
/// `[package]` name, below `root`.
fn package_of(root: &Path, path: &str) -> Option<String> {
    let mut dir = root.join(path).parent()?.to_path_buf();
    while dir.starts_with(root) {
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && let Some(name) = package_name(&text)
        {
            return Some(name);
        }
        if dir == root {
            break;
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// The `name` under `[package]` in a Cargo manifest.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(rest) = line.strip_prefix("name") {
            let value = rest
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .trim_matches('"');
            return Some(value.to_string());
        }
    }
    None
}

/// The node package at `dir`: its `check`, `test` and (when it has neither)
/// `build` scripts, with the package manager its lockfile names.
fn node(root: &Path, dir: &Path) -> Vec<Check> {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let scripts = json
        .get("scripts")
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default();
    let manager = if dir.join("pnpm-lock.yaml").is_file() || root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if dir.join("yarn.lock").is_file() || root.join("yarn.lock").is_file() {
        "yarn"
    } else if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    };
    let has = |name: &str| {
        scripts
            .get(name)
            .and_then(|s| s.as_str())
            .is_some_and(|s| !s.contains("no test specified"))
    };
    let run = |script: &str| match (manager, script) {
        ("npm", "test") => "npm test".to_string(),
        ("npm", s) => format!("npm run {s}"),
        (m, s) => format!("{m} run {s}"),
    };
    let mut out = Vec::new();
    // The whole package compiles: its type check, else its build.
    if let Some(compile) = ["check", "build"].into_iter().find(|s| has(s)) {
        out.push(Check {
            label: format!("{manager} {compile}"),
            command: run(compile),
            dir: dir.to_path_buf(),
        });
    }
    if has("test") {
        out.push(Check {
            label: format!("{manager} test"),
            command: run("test"),
            dir: dir.to_path_buf(),
        });
    }
    out
}

/// The package folders (holding `manifest`) the changed paths sit in.
fn changed_package_dirs(root: &Path, changed: &[String], manifest: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in changed {
        let mut dir = root.join(path).parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if !d.starts_with(root) || d == root {
                break;
            }
            if d.join(manifest).is_file() {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    dirs
}

fn has_pytest_files(root: &Path) -> bool {
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("test_") && name.ends_with(".py")
            })
        })
        .unwrap_or(false)
}

fn makefile_has(root: &Path, target: &str) -> bool {
    std::fs::read_to_string(root.join("Makefile"))
        .map(|m| m.lines().any(|l| l.starts_with(&format!("{target}:"))))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn a_rust_workspace_tests_only_the_crates_the_change_touches() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(
            root,
            "crates/db/Cargo.toml",
            "[package]\nname = \"nebo-db\"\n",
        );
        write(
            root,
            "crates/agent/Cargo.toml",
            "[package]\nname = \"nebo-agent\"\nversion = \"1\"\n",
        );
        let checks = detect(
            root,
            &[
                "crates/db/src/a.rs".into(),
                "crates/agent/src/b.rs".into(),
                "crates/db/migrations/1.sql".into(),
            ],
        );
        let commands: Vec<&str> = checks.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(
            commands,
            [
                "cargo check --workspace --all-targets",
                "cargo test -p nebo-db -p nebo-agent"
            ],
            "the whole workspace compiles"
        );
        let build = tempfile::tempdir().unwrap();
        assert!(
            checks[1]
                .with_build_dir(build.path())
                .starts_with("CARGO_TARGET_DIR='"),
            "its own build folder"
        );
    }

    #[test]
    fn a_web_package_runs_its_own_scripts_with_its_own_package_manager() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"x\"\n");
        write(
            root,
            "app/package.json",
            r#"{"scripts":{"check":"svelte-check","test":"vitest --run","build":"vite build"}}"#,
        );
        write(root, "app/pnpm-lock.yaml", "");
        let checks = detect(root, &["app/src/routes/+page.svelte".into()]);
        let commands: Vec<&str> = checks.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(
            commands,
            [
                "cargo check --workspace --all-targets",
                "pnpm run check",
                "pnpm run test"
            ]
        );
        assert_eq!(checks[1].dir, root.join("app"));
    }

    #[test]
    fn a_check_that_outlasts_its_time_is_found_by_its_background_id() {
        assert_eq!(
            backgrounded("Command exceeded its timeout (600s) and was moved to the background with ID: bg-12ab. It is still running"),
            Some("bg-12ab".into())
        );
        assert_eq!(backgrounded("test result: ok"), None);
    }

    #[test]
    fn a_long_output_keeps_its_end() {
        let long: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let t = tail(&long);
        assert!(t.starts_with("[… earlier output left out]\n"));
        assert!(t.ends_with("line 1999"));
        assert!(t.chars().count() <= TAIL_CHARS + 40);
        assert_eq!(tail("ok\n"), "ok");
    }

    #[test]
    fn a_web_package_without_a_type_check_compiles_with_its_build() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"build":"vite build","test":"vitest --run"}}"#,
        );
        let commands: Vec<String> = detect(dir.path(), &["src/a.ts".into()])
            .into_iter()
            .map(|c| c.command)
            .collect();
        assert_eq!(commands, ["npm run build", "npm test"]);
    }

    #[test]
    fn every_verification_of_a_project_shares_one_build_folder() {
        crate::test_home::with_home(|home| {
            let a = build_cache(Path::new("/work/site")).unwrap();
            assert_eq!(a, build_cache(Path::new("/work/site")).unwrap());
            assert_ne!(a, build_cache(Path::new("/work/other")).unwrap());
            // With the working copies, where a command may write; never in
            // Nebo's own folder, which is closed to every command.
            assert!(a.starts_with(config::copies_dir().unwrap().join("build")), "{}", a.display());
            assert!(!a.starts_with(home));
            let _ = std::fs::remove_dir_all(config::copies_dir().unwrap());
        })
    }

    #[test]
    fn a_project_with_no_build_or_tests_has_no_checks() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        );
        assert!(detect(dir.path(), &["index.js".into()]).is_empty());
    }
}
