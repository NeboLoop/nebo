use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::walk_bounds::{self, skip_paths};

/// The whole call's budget, not one command's: when plocate is absent and the
/// find fallback runs, both share the one deadline every filesystem walk in
/// this tree gets. See `walk_bounds` for why twenty seconds.
const SEARCH_DEADLINE: Duration = walk_bounds::WALK_DEADLINE;

/// The engine budget this tool asks for: the search's own deadline plus room
/// to format the answer. The tool always stops itself first, so the model
/// reads the search's own sentence about narrowing the query instead of the
/// runner's generic timeout text.
const SEARCH_EXECUTION_TIMEOUT: Duration = walk_bounds::WALK_EXECUTION_TIMEOUT;

/// How deep a name search walks. Matches the depth the Linux fallback has
/// always used; a file further down than this wants a `dir`.
const MAX_DEPTH: &str = "5";

/// Spotlight tool: search files using platform-native search (mdfind on macOS, plocate/find on Linux).
pub struct SpotlightTool;

impl SpotlightTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for SpotlightTool {
    fn name(&self) -> &str {
        "spotlight"
    }

    fn description(&self) -> String {
        "Search for files using the OS search index (Spotlight on macOS, plocate on Linux, PowerShell on Windows).\n\n\
         Actions:\n\
         - search: Find files matching a query\n\n\
         The search is bounded: it starts from `dir`, or from the bot's own working area when no `dir` is given, \
         and gives up after 20 seconds. Pass `dir` whenever you know roughly where the file is.\n\n\
         Examples:\n  \
         os(resource: \"search\", action: \"search\", query: \"budget 2024\")\n  \
         os(resource: \"search\", action: \"search\", query: \"*.pdf\", dir: \"~/Documents\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["search"]
                },
                "query": {
                    "type": "string",
                    "description": "Search query or filename pattern"
                },
                "dir": {
                    "type": "string",
                    "description": "Directory to search within (defaults to the bot's working area)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results (default 50)"
                }
            },
            "required": ["action", "query"]
        })
    }


    fn execution_timeout(&self, _input: &serde_json::Value) -> Option<std::time::Duration> {
        Some(SEARCH_EXECUTION_TIMEOUT)
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            match action {
                "search" => handle_search(ctx, &input).await,
                _ => ToolResult::error(format!("Unknown action '{}'. Use: search", action)),
            }
        })
    }
}

/// What a bounded search command came back with. `lines` is everything it had
/// printed when it stopped, whether it finished or ran out of time.
struct Search {
    lines: Vec<String>,
    /// The deadline passed and the child was killed before it finished.
    timed_out: bool,
    /// The command exited successfully (meaningless when it timed out).
    ok: bool,
    stderr: String,
    /// The child's pid. Carried for the test that proves nothing outlives the
    /// deadline; the running server reads it from the log line instead.
    #[cfg_attr(not(test), allow(dead_code))]
    pid: Option<u32>,
    /// The command could not be started at all (plocate absent, for instance).
    spawn_error: Option<String>,
}

impl Search {
    fn failed_to_start(e: std::io::Error) -> Self {
        Self {
            lines: Vec::new(),
            timed_out: false,
            ok: false,
            stderr: String::new(),
            pid: None,
            spawn_error: Some(e.to_string()),
        }
    }
}

/// Run one search command under a deadline it cannot outlive.
///
/// Output is read line by line as it arrives, so a search that is stopped
/// still hands back what it had found. The child goes through
/// `process::GroupChild`, the one spawn-with-a-deadline door in this crate:
/// it leads its own process group and the group is what gets killed, so the
/// gate's `find` processes — still walking the filesystem three and seven
/// minutes after their runs were cancelled — cannot happen here either.
async fn run_search(
    mut cmd: tokio::process::Command,
    limit: usize,
    deadline: Instant,
) -> Search {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match crate::process::GroupChild::spawn(cmd) {
        Ok(c) => c,
        Err(e) => return Search::failed_to_start(e),
    };
    let pid = child.id();
    let mut out = tokio::io::BufReader::new(child.stdout().expect("stdout piped")).lines();
    let mut err = tokio::io::BufReader::new(child.stderr().expect("stderr piped")).lines();

    let mut lines: Vec<String> = Vec::new();
    let mut stderr = String::new();
    let mut out_done = false;
    let mut err_done = false;
    let mut timed_out = false;
    let mut enough = false;

    let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(sleep);

    while !(out_done && err_done) {
        tokio::select! {
            _ = &mut sleep => { timed_out = true; break; }
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => {
                    if !l.trim().is_empty() {
                        lines.push(l);
                    }
                    if lines.len() >= limit {
                        enough = true;
                        break;
                    }
                }
                _ => out_done = true,
            },
            line = err.next_line(), if !err_done => match line {
                Ok(Some(l)) => {
                    // Broad searches print a "Permission denied" per unreadable
                    // directory; keep enough to explain a real failure, not the noise.
                    if stderr.len() < 2000 {
                        stderr.push_str(l.trim());
                        stderr.push('\n');
                    }
                }
                _ => err_done = true,
            },
        }
    }

    let ok = if timed_out || enough {
        // Kill and reap: the child dies with this call, never after it.
        if timed_out {
            tracing::debug!(pid = ?pid, "file search hit its deadline; killing the child");
        }
        child.kill_and_reap().await;
        enough
    } else {
        child.wait().await.map(|s| s.success()).unwrap_or(false)
    };

    Search {
        lines,
        timed_out,
        ok,
        stderr: stderr.trim().to_string(),
        pid,
        spawn_error: None,
    }
}

/// Where a search starts. A search with no `dir` searches the bot's own
/// working area — never `/`. On a Linux bot (every cloud bot is one) `find /`
/// walks `/proc`, `/sys` and every build tree on the box, and there is no
/// index to fall back on. An explicit `/` from the caller is honoured, and
/// bounded in time like any other root.
fn search_root(dir: &str, ctx: &ToolContext) -> PathBuf {
    let dir = dir.trim();
    if !dir.is_empty() {
        return PathBuf::from(crate::file_tool::expand_path(dir));
    }
    if let Some(cwd) = ctx.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        return PathBuf::from(cwd);
    }
    if let Some(home) = dirs::home_dir() {
        return home;
    }
    config::data_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// A bare word matches anywhere in the name; a pattern the caller already
/// wrote as a glob is used as written. One rule on every platform.
fn name_pattern(query: &str) -> String {
    if query.contains('*') || query.contains('?') || query.contains('[') {
        query.to_string()
    } else {
        format!("*{query}*")
    }
}

/// The find command for this root: depth-bounded, kept on the root's own
/// filesystem with `-xdev`, and with every path in `skip` pruned by name
/// before find ever asks the kernel about it.
fn find_command(root: &Path, query: &str, skip: &[PathBuf]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("find");
    // `-xdev` keeps the walk on one filesystem: a virtiofs, NFS or SMB mount
    // under the root is never entered, so a directory read can never block
    // forever in the kernel. The named prunes below cover what `-xdev` cannot
    // — an autofs trigger that would mount the remote filesystem the moment
    // find looked at it.
    cmd.arg(root).arg("-xdev").args(["-maxdepth", MAX_DEPTH]);

    if !skip.is_empty() {
        cmd.arg("(");
        for (i, p) in skip.iter().enumerate() {
            if i > 0 {
                cmd.arg("-o");
            }
            cmd.arg("-path").arg(p);
        }
        cmd.arg(")").arg("-prune").arg("-o");
    }

    cmd.arg("-iname").arg(name_pattern(query)).arg("-print");
    cmd
}

/// Walk `root` for a name, bounded by `deadline`. The ONE fallback for every
/// platform whose index came back empty or is not installed.
async fn run_find(
    root: &Path,
    query: &str,
    limit: usize,
    deadline: Instant,
    skip: &[PathBuf],
) -> Search {
    run_search(find_command(root, query, skip), limit, deadline).await
}

/// The plain sentence a search that ran out of time gives back. The wording
/// is the one every bounded walk in this tree uses; a search narrows with
/// `dir`.
fn took_too_long(query: &str, root: &Path, budget: Duration, partial: &[String]) -> String {
    walk_bounds::took_too_long(
        &format!("search for \"{query}\""),
        root,
        walk_bounds::CutShort::Deadline(budget),
        "dir",
        partial,
    )
}

/// Nothing matched — the same advice on every platform.
const NOTHING_FOUND: &str = "No files found. To find files by name or extension pattern, use find through run_command: find . -name \"*.ext\"";

fn render(found: &[String], limit: usize) -> ToolResult {
    if found.is_empty() {
        ToolResult::ok(NOTHING_FOUND)
    } else {
        ToolResult::ok(format!(
            "{}:\n{}",
            found_header(found.len(), limit),
            found.join("\n")
        ))
    }
}

async fn handle_search(ctx: &ToolContext, input: &serde_json::Value) -> ToolResult {
    let query = input["query"].as_str().unwrap_or("");
    if query.is_empty() {
        return ToolResult::error(crate::errors::missing_param(
            "search",
            "query",
            "os(resource: \"search\", action: \"search\", query: \"budget report 2024\")",
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let limit = input["limit"].as_i64().unwrap_or(50).clamp(1, 1000) as usize;
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let dir = input["dir"].as_str().unwrap_or("");
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let root = search_root(dir, ctx);
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let deadline = Instant::now() + SEARCH_DEADLINE;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let skip = skip_paths(&root);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _ = ctx;

    #[cfg(target_os = "macos")]
    {
        let mut cmd = tokio::process::Command::new("mdfind");
        cmd.arg("-onlyin").arg(&root).arg(query);
        let indexed = run_search(cmd, limit, deadline).await;

        if let Some(e) = &indexed.spawn_error {
            return ToolResult::error(format!("Failed to run mdfind: {e}"));
        }
        if indexed.timed_out {
            return ToolResult::error(took_too_long(query, &root, SEARCH_DEADLINE, &indexed.lines));
        }
        if !indexed.ok && indexed.lines.is_empty() && !indexed.stderr.is_empty() {
            return ToolResult::error(format!("mdfind error: {}", indexed.stderr));
        }
        if !indexed.lines.is_empty() {
            return render(&indexed.lines, limit);
        }

        // Spotlight does not index ~/Library (app data lives there), so fall
        // back to a bounded find over the same root, inside the same deadline.
        let walked = run_find(&root, query, limit, deadline, &skip).await;
        if walked.timed_out {
            return ToolResult::error(took_too_long(query, &root, SEARCH_DEADLINE, &walked.lines));
        }
        if walked.lines.is_empty() {
            return ToolResult::ok(format!(
                "No files found under {}. Note: Spotlight does not index ~/Library — for app data pass dir: \"~/Library\". For name/extension patterns use find through run_command: find . -name \"*.ext\"",
                root.display()
            ));
        }
        ToolResult::ok(format!(
            "{} (mdfind returned nothing; results are from a bounded find in {}):\n{}",
            found_header(walked.lines.len(), limit),
            root.display(),
            walked.lines.join("\n")
        ))
    }

    #[cfg(target_os = "linux")]
    {
        // The index first, when the box has one.
        let mut cmd = tokio::process::Command::new("plocate");
        cmd.arg("-l").arg(limit.to_string()).arg(query);
        let indexed = run_search(cmd, limit, deadline).await;

        if indexed.timed_out {
            return ToolResult::error(took_too_long(query, &root, SEARCH_DEADLINE, &indexed.lines));
        }
        if indexed.spawn_error.is_none() && indexed.ok && !indexed.lines.is_empty() {
            return render(&indexed.lines, limit);
        }
        if indexed.spawn_error.is_none() && indexed.ok {
            return ToolResult::ok(NOTHING_FOUND);
        }

        // No plocate on this box — every cloud bot is in this branch. Walk,
        // but only the bot's own working area, and only until the deadline.
        let walked = run_find(&root, query, limit, deadline, &skip).await;
        if let Some(e) = &walked.spawn_error {
            return ToolResult::error(format!("Search failed: {e}"));
        }
        if walked.timed_out {
            return ToolResult::error(took_too_long(query, &root, SEARCH_DEADLINE, &walked.lines));
        }
        render(&walked.lines, limit)
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_root = root.to_string_lossy().replace('\'', "''");
        let script = format!(
            "Get-ChildItem -Path '{}' -Recurse -Depth {} -Filter '{}' -ErrorAction SilentlyContinue | Select-Object -First {} -ExpandProperty FullName",
            escaped_root,
            MAX_DEPTH,
            name_pattern(query).replace('\'', "''"),
            limit
        );
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        let walked = run_search(cmd, limit, deadline).await;

        if let Some(e) = &walked.spawn_error {
            return ToolResult::error(format!("Failed to run search: {e}"));
        }
        if walked.timed_out {
            return ToolResult::error(took_too_long(query, &root, SEARCH_DEADLINE, &walked.lines));
        }
        if !walked.ok && walked.lines.is_empty() && !walked.stderr.is_empty() {
            return ToolResult::error(format!("Search error: {}", walked.stderr));
        }
        render(&walked.lines, limit)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        ToolResult::error("File search is not supported on this platform")
    }
}

/// "Found N results", and when the list was cut at `limit`, says so and
/// names the parameter that raises it.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux", target_os = "windows")), allow(dead_code))]
fn found_header(n: usize, limit: usize) -> String {
    if n >= limit {
        format!("Found {n} results (limit {limit}; pass limit: N for more)")
    } else {
        format!("Found {n} results")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk_bounds::parse_foreign_mounts;

    /// A pid is gone (or a reaped-any-moment zombie) — not still walking the disk.
    fn still_running(pid: u32) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
        !state.is_empty() && !state.starts_with('Z')
    }

    #[test]
    fn found_header_names_the_limit_only_when_it_was_hit() {
        assert_eq!(found_header(3, 50), "Found 3 results");
        assert_eq!(found_header(50, 50), "Found 50 results (limit 50; pass limit: N for more)");
    }

    #[test]
    fn test_tool_metadata() {
        let tool = SpotlightTool::new();
        assert_eq!(tool.name(), "spotlight");
        assert!(tool.description().contains("search"));
        let schema = tool.schema();
        assert!(schema["properties"]["query"].is_object());
    }

    #[tokio::test]
    async fn test_missing_query() {
        let tool = SpotlightTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "search"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("query"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = SpotlightTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"action": "delete", "query": "test"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown action"));
    }

    /// The engine's budget must be the tool's, so the search's own sentence
    /// about narrowing the query is what the model reads — never the runner's
    /// timeout text, which the harness would cut off before it ever arrived.
    #[test]
    fn the_engine_budget_is_the_tools_own() {
        let tool = SpotlightTool::new();
        let budget = tool
            .execution_timeout(&serde_json::json!({"action": "search", "query": "x"}))
            .expect("search declares its own budget");
        assert!(budget > SEARCH_DEADLINE, "the tool must stop itself first");
        assert!(
            budget < Duration::from_secs(180),
            "must land inside the harness's silence cap"
        );
    }

    /// The defect: a search with no `dir` used to become `find /`. The root
    /// is the bot's own working area instead, and an explicit root is kept.
    #[test]
    fn a_search_with_no_dir_never_starts_at_the_filesystem_root() {
        let ctx = ToolContext::default();
        assert_ne!(search_root("", &ctx), PathBuf::from("/"));

        let mut scoped = ToolContext::default();
        scoped.cwd = Some("/tmp/work-area".to_string());
        assert_eq!(search_root("", &scoped), PathBuf::from("/tmp/work-area"));

        // A caller who asks for `/` gets `/` — bounded in time, not refused.
        assert_eq!(search_root("/", &scoped), PathBuf::from("/"));
    }

    /// A broad root prunes the kernel and device trees and stays on one
    /// filesystem; a scoped one pays nothing for them.
    #[test]
    fn a_broad_root_prunes_the_trees_that_are_not_files() {
        let broad = find_command(Path::new("/"), "*.md", &skip_paths(Path::new("/")));
        let args: Vec<String> = broad
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"-xdev".to_string()), "the walk must stay on one filesystem");
        assert!(args.contains(&"-prune".to_string()));
        assert!(args.contains(&"/proc".to_string()));
        assert!(args.contains(&"/sys".to_string()));
        assert!(args.contains(&"/mnt".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "-maxdepth" && w[1] == MAX_DEPTH));

        let scoped = find_command(Path::new("/tmp/fixture"), "*.md", &skip_paths(Path::new("/tmp/fixture")));
        let args: Vec<String> = scoped
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!args.contains(&"-prune".to_string()));
        assert!(args.contains(&"-xdev".to_string()));
    }

    /// The incident's shape, run for real: a directory standing in for the
    /// virtiofs mount that hung the gate's `find`. The search must not enter
    /// it, and must still find what is beside it.
    #[tokio::test]
    async fn a_search_never_enters_a_foreign_mount() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path().join("lima-rosetta");
        std::fs::create_dir_all(&mount).expect("fixture mount");
        std::fs::write(mount.join("notes.md"), "x").expect("fixture file");
        std::fs::write(tmp.path().join("notes.md"), "x").expect("fixture file");

        // The mounts table this box would have reported, stubbed.
        let table = format!(
            "/dev/vda1 / ext4 rw 0 0\nmount0 {} fuse.virtiofs rw 0 0\n",
            mount.display()
        );
        let skip = parse_foreign_mounts(&table);
        assert_eq!(skip, vec![mount.clone()]);

        let walked = run_find(tmp.path(), "notes", 50, Instant::now() + SEARCH_DEADLINE, &skip).await;
        assert!(!walked.timed_out);
        assert_eq!(walked.lines.len(), 1, "found {:?}", walked.lines);
        assert!(
            !walked.lines[0].contains("lima-rosetta"),
            "the walk stepped into the mount it was told to skip: {:?}",
            walked.lines
        );
    }

    /// The incident, reproduced: a walk of a root far too big to finish in the
    /// budget it was given. It must come back inside the deadline, say plainly
    /// that it was stopped and how to narrow it, and leave nothing behind
    /// still walking the disk. The budget is a fraction of a second on purpose
    /// — what is under test is the deadline, not the size of the disk.
    #[tokio::test]
    async fn a_walk_of_a_huge_root_stops_at_the_deadline_and_kills_its_child() {
        let budget = Duration::from_millis(100);
        let root = Path::new("/");
        let started = Instant::now();
        let walked = run_find(root, "*.md", 50, started + budget, &skip_paths(root)).await;
        let elapsed = started.elapsed();

        assert!(walked.timed_out, "a walk of / cannot finish in {budget:?}");
        assert!(
            elapsed < Duration::from_secs(5),
            "the search must return at its deadline, took {elapsed:?}"
        );

        let pid = walked.pid.expect("the walk spawned a child");
        assert!(
            !still_running(pid),
            "find (pid {pid}) outlived the search that started it"
        );

        let msg = took_too_long("*.md", root, budget, &walked.lines);
        assert!(msg.contains("took longer than"));
        assert!(msg.contains("Narrow it"));
        assert!(msg.contains("dir"));
        assert!(!msg.contains('!'));
    }

    /// A scoped search still does its job: it finds the file and comes back.
    #[tokio::test]
    async fn a_scoped_search_finds_its_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("reports/2024");
        std::fs::create_dir_all(&nested).expect("fixture tree");
        std::fs::write(nested.join("budget-2024.md"), "x").expect("fixture file");
        std::fs::write(tmp.path().join("unrelated.txt"), "x").expect("fixture file");

        let walked = run_find(tmp.path(), "budget", 50, Instant::now() + SEARCH_DEADLINE, &[]).await;
        assert!(!walked.timed_out, "a three-file tree finishes");
        assert_eq!(walked.lines.len(), 1, "found {:?}", walked.lines);
        assert!(walked.lines[0].ends_with("budget-2024.md"));

        let rendered = render(&walked.lines, 50);
        assert!(!rendered.is_error);
        assert!(rendered.content.contains("budget-2024.md"));
    }

    /// The whole tool, through its public door: a search scoped to a fixture
    /// tree comes back with the file and no timeout.
    #[tokio::test]
    async fn the_tool_searches_the_directory_it_was_given() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("invoice-441.pdf"), "x").expect("fixture file");

        let tool = SpotlightTool::new();
        let ctx = ToolContext::default();
        let result = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({
                    "action": "search",
                    "query": "invoice-441",
                    "dir": tmp.path().to_string_lossy(),
                }),
            )
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(
            result.content.contains("invoice-441.pdf"),
            "{}",
            result.content
        );
    }

    /// A command that would run for half a minute is killed at the deadline,
    /// and its partial output survives.
    #[tokio::test]
    async fn a_slow_command_is_killed_at_the_deadline_with_its_partial_output() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "echo first; echo second; sleep 30"]);
        let started = Instant::now();
        let ran = run_search(cmd, 50, started + Duration::from_millis(600)).await;

        assert!(ran.timed_out);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(ran.lines, vec!["first".to_string(), "second".to_string()]);
        let pid = ran.pid.expect("spawned");
        assert!(!still_running(pid), "sleep (pid {pid}) outlived its deadline");
    }

    /// The search door spawns through the same process-group helper as the
    /// shell door, so a grandchild dies with the command that started it.
    /// The defect (gate fixture `run-command-retry-spiral`): the wrapper was
    /// killed at its deadline and the `find` it had started kept walking,
    /// reparented to init — sixteen of them on the CI VM at a load average
    /// near 22.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_search_that_hits_its_deadline_takes_its_grandchildren_with_it() {
        let file = std::env::temp_dir().join(format!(
            "nebo-search-group-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&file);
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", file.display()));

        let ran = run_search(cmd, 50, Instant::now() + Duration::from_millis(500)).await;
        assert!(ran.timed_out, "expected the deadline to stop the command");

        let mut grandchild = None;
        for _ in 0..50 {
            if let Ok(text) = std::fs::read_to_string(&file)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                grandchild = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let grandchild = grandchild.expect("the shell reported its grandchild's pid");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !still_running(grandchild),
            "the sleeping grandchild (pid {grandchild}) outlived its grandparent's deadline"
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_bare_word_matches_anywhere_a_glob_is_used_as_written() {
        assert_eq!(name_pattern("budget"), "*budget*");
        assert_eq!(name_pattern("*.md"), "*.md");
    }
}
