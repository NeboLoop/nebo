//! Replay export: a real run that ended in a guard or reviewer stop becomes a
//! fixture a person can finish (`setup:`) and re-run. `nebo-cli test export`
//! is the one caller; everything here is pure so it can be tested without a
//! store.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::fixture::{
    Assertion, Check, ConversationTurn, Fixture, IdealBehavior, PromptAssertions, Severity,
};
use db::models::ChatMessage;

/// Characters of the session-key hash carried in a replay fixture id.
const REPLAY_ID_HASH_CHARS: usize = 8;
/// The fixture name is the first user prompt cut to this many characters.
const REPLAY_NAME_MAX_CHARS: usize = 72;
/// Tool-call argument fields that may name a filesystem location.
const PATH_ARG_FIELDS: [&str; 4] = ["path", "paths", "command", "cwd"];
/// Assertion ids the exporter writes.
const FIRST_CALL_ASSERTION_ID: &str = "first-call";
const RECOVERY_ASSERTION_ID: &str = "recovery-no-repeat";

/// What `nebo-cli test export` writes: the fixture plus the paths the run
/// named, for the header comment a person turns into `setup:`.
#[derive(Debug, Clone)]
pub struct ReplayExport {
    pub fixture: Fixture,
    /// Absolute paths from the run's tool-call arguments, sorted.
    pub touched: Vec<String>,
}

/// Build the replay fixture for one session from its stored messages.
/// `Err` when the transcript has no real user turn to replay.
pub fn fixture_from_run(
    session_key: &str,
    exit_reason: &str,
    messages: &[ChatMessage],
) -> Result<ReplayExport, String> {
    let conversation: Vec<ConversationTurn> = messages
        .iter()
        .filter(|m| is_real_user_turn(m))
        .map(|m| ConversationTurn {
            role: "user".to_string(),
            content: m.content.clone(),
        })
        .collect();
    let first_prompt = conversation
        .first()
        .map(|t| t.content.as_str())
        .ok_or_else(|| format!("session {} has no user turn to replay", session_key))?;

    let calls: Vec<ai::ToolCall> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_deref())
        // Rows the runner did not write (or wrote before the current shape)
        // carry no replayable call; the transcript is still worth exporting.
        .filter_map(|json| serde_json::from_str::<Vec<ai::ToolCall>>(json).ok())
        .flatten()
        .collect();

    let (touched, cwd) = paths_from_calls(&calls);

    let mut first_call = Vec::new();
    if let Some(call) = calls.first() {
        let action = call.input.get("action").and_then(|a| a.as_str());
        let text = match action {
            Some(a) => format!("First tool call is {} {}", call.name, a),
            None => format!("First tool call is {}", call.name),
        };
        first_call.push(Assertion {
            id: FIRST_CALL_ASSERTION_ID.to_string(),
            text,
            severity: Severity::Important,
            metric: None,
            threshold: None,
            tests: None,
            check: Some(Check {
                first_call: true,
                tool: vec![call.name.clone()],
                arg: action.map(|_| "action".to_string()),
                equals: action.map(|a| serde_yaml::Value::String(a.to_string())),
                ..Check::default()
            }),
        });
    }

    let recovery = vec![Assertion {
        id: RECOVERY_ASSERTION_ID.to_string(),
        text: format!(
            "The run must not end in {} again. If it cannot finish the task, it states what blocked it.",
            exit_reason
        ),
        severity: Severity::Critical,
        metric: None,
        threshold: None,
        tests: None,
        check: None,
    }];

    let agent = types::keyparser::extract_agent_id(session_key);
    let fixture = Fixture {
        id: format!("replay-{}", session_hash(session_key)),
        name: truncate_chars(first_prompt, REPLAY_NAME_MAX_CHARS),
        description: format!(
            "Replay of session {}, which ended in {}.",
            session_key, exit_reason
        ),
        target_component: String::new(),
        setup: Vec::new(),
        teardown: Vec::new(),
        conversation,
        tool_config: Default::default(),
        prompt_assertions: PromptAssertions {
            first_call,
            recovery,
            cost: Vec::new(),
        },
        integrated_assertions: Vec::new(),
        ideal_behavior: IdealBehavior::default(),
        cwd: cwd.map(|p| p.to_string_lossy().into_owned()),
        agent: (!agent.is_empty()).then_some(agent),
    };

    Ok(ReplayExport { fixture, touched })
}

/// The fixture as YAML, headed by the touched-file list as comments.
pub fn render_yaml(export: &ReplayExport) -> Result<String, String> {
    let body = serde_yaml::to_string(&export.fixture)
        .map_err(|e| format!("serialize fixture: {}", e))?;
    let mut out = String::new();
    out.push_str("# Replay fixture written by `nebo-cli test export`.\n");
    out.push_str("# Files the run touched. Write `setup:` so they exist before the replay:\n");
    if export.touched.is_empty() {
        out.push_str("#   (none: the run named no absolute paths)\n");
    }
    for p in &export.touched {
        out.push_str("#   ");
        out.push_str(p);
        out.push('\n');
    }
    out.push_str(&body);
    Ok(out)
}

/// A user row the owner actually typed: not a synthetic continuation and not
/// an isMeta (platform-authored) prompt.
fn is_real_user_turn(m: &ChatMessage) -> bool {
    if m.role != "user" || m.content.trim().is_empty() {
        return false;
    }
    if crate::goals::is_continuation_prompt(&m.content) {
        return false;
    }
    let is_meta = m
        .metadata
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|v| v.get("isMeta").and_then(|b| b.as_bool()))
        .unwrap_or(false);
    !is_meta
}

/// Every absolute path the calls name (sorted, deduplicated) and the longest
/// common ancestor of their directories. A `path`/`paths`/`command` entry is
/// treated as a file, so its ancestor is its parent; a `cwd` entry is a
/// directory and counts as itself. No ancestor below the filesystem root is
/// reported as `None`.
fn paths_from_calls(calls: &[ai::ToolCall]) -> (Vec<String>, Option<PathBuf>) {
    let mut touched: BTreeSet<String> = BTreeSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for call in calls {
        for field in PATH_ARG_FIELDS {
            let Some(value) = call.input.get(field) else { continue };
            let strings: Vec<&str> = match value {
                serde_json::Value::String(s) => vec![s.as_str()],
                serde_json::Value::Array(items) => {
                    items.iter().filter_map(|i| i.as_str()).collect()
                }
                _ => Vec::new(),
            };
            for s in strings {
                let candidates: Vec<&str> = if field == "command" {
                    absolute_tokens(s)
                } else if s.starts_with('/') {
                    vec![s]
                } else {
                    Vec::new()
                };
                for p in candidates {
                    touched.insert(p.to_string());
                    let path = Path::new(p);
                    let dir = if field == "cwd" {
                        Some(path.to_path_buf())
                    } else {
                        path.parent().map(Path::to_path_buf)
                    };
                    if let Some(d) = dir {
                        dirs.push(d);
                    }
                }
            }
        }
    }
    (touched.into_iter().collect(), common_ancestor(&dirs))
}

/// Whitespace-separated tokens of a shell command that are absolute paths,
/// with surrounding quotes, a `--flag=` prefix, and trailing punctuation
/// stripped.
fn absolute_tokens(command: &str) -> Vec<&str> {
    command
        .split_whitespace()
        .map(|tok| {
            let tok = tok.trim_matches(['"', '\'']);
            let tok = tok.rsplit_once('=').map(|(_, v)| v).unwrap_or(tok);
            tok.trim_end_matches([';', ',', ')', ':'])
        })
        .filter(|tok| tok.starts_with('/') && tok.len() > 1)
        .collect()
}

/// Component-wise common prefix of absolute directories; `None` when they
/// share nothing below the root or there are none.
fn common_ancestor(dirs: &[PathBuf]) -> Option<PathBuf> {
    let first = dirs.first()?;
    let mut prefix: Vec<Component<'_>> = first.components().collect();
    for dir in &dirs[1..] {
        let comps: Vec<Component<'_>> = dir.components().collect();
        let shared = prefix
            .iter()
            .zip(comps.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(shared);
    }
    // The root alone (`/`) is one component and not a working directory.
    if prefix.len() < 2 {
        return None;
    }
    Some(prefix.iter().collect())
}

fn session_hash(session_key: &str) -> String {
    let digest = Sha256::digest(session_key.as_bytes());
    hex::encode(digest)[..REPLAY_ID_HASH_CHARS].to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}...", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, tool_calls: Option<&str>, metadata: Option<&str>) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "c1".into(),
            role: role.into(),
            content: content.into(),
            metadata: metadata.map(str::to_string),
            created_at: 0,
            day_marker: None,
            tool_calls: tool_calls.map(str::to_string),
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    fn calls(json: &str) -> Option<&str> {
        Some(json)
    }

    const SESSION: &str = "agent:emp1:web";

    /// Only the owner's own turns replay: assistant rows, the synthetic
    /// continuation prompt, and isMeta prompts are all left out.
    #[test]
    fn conversation_is_the_real_user_turns_only() {
        let messages = vec![
            msg("user", "Fix the failing test", None, None),
            msg("assistant", "Looking.", None, None),
            msg("user", &crate::goals::continuation_prompt("unfinished"), None, None),
            msg("user", "platform intro", None, Some(r#"{"isMeta":true}"#)),
            msg("user", "Also update the docs", None, Some(r#"{"isMeta":false}"#)),
        ];
        let export = fixture_from_run(SESSION, "reviewer_stop", &messages).unwrap();
        let turns: Vec<&str> = export.fixture.conversation.iter().map(|t| t.content.as_str()).collect();
        assert_eq!(turns, vec!["Fix the failing test", "Also update the docs"]);
        assert!(export.fixture.conversation.iter().all(|t| t.role == "user"));
    }

    #[test]
    fn a_session_with_no_user_turn_is_an_error() {
        let messages = vec![msg("assistant", "hello", None, None)];
        let err = fixture_from_run(SESSION, "reviewer_stop", &messages).unwrap_err();
        assert!(err.contains("no user turn"), "{err}");
    }

    /// The cwd is the deepest directory every named path lives under, and the
    /// header list carries each path once.
    #[test]
    fn cwd_is_the_common_ancestor_of_touched_paths() {
        let messages = vec![
            msg("user", "go", None, None),
            msg(
                "assistant",
                "",
                calls(
                    r#"[
                      {"id":"1","name":"os","input":{"resource":"file","action":"read","path":"/proj/src/a.rs"}},
                      {"id":"2","name":"os","input":{"resource":"shell","action":"run","command":"cat /proj/README.md; ls '/proj/src'"}},
                      {"id":"3","name":"os","input":{"resource":"file","action":"checkpoint","paths":["/proj/src/b.rs","/proj/src/a.rs"]}}
                    ]"#,
                ),
                None,
            ),
        ];
        let export = fixture_from_run(SESSION, "same_error_loop", &messages).unwrap();
        assert_eq!(export.fixture.cwd.as_deref(), Some("/proj"));
        assert_eq!(
            export.touched,
            vec!["/proj/README.md", "/proj/src", "/proj/src/a.rs", "/proj/src/b.rs"]
        );
    }

    /// A `cwd` argument is a directory and counts as itself, not its parent.
    #[test]
    fn a_cwd_argument_counts_as_a_directory() {
        let messages = vec![
            msg("user", "go", None, None),
            msg(
                "assistant",
                "",
                calls(r#"[{"id":"1","name":"os","input":{"resource":"shell","action":"run","command":"make","cwd":"/work/app"}}]"#),
                None,
            ),
        ];
        let export = fixture_from_run(SESSION, "stalled", &messages).unwrap();
        assert_eq!(export.fixture.cwd.as_deref(), Some("/work/app"));
    }

    /// Paths that share only the root, relative paths, and runs with no
    /// calls at all yield no cwd rather than `/`.
    #[test]
    fn cwd_is_omitted_without_a_shared_directory() {
        let split = vec![
            msg("user", "go", None, None),
            msg(
                "assistant",
                "",
                calls(r#"[{"id":"1","name":"os","input":{"path":"/a/x.rs"}},{"id":"2","name":"os","input":{"path":"/b/y.rs"}}]"#),
                None,
            ),
        ];
        assert_eq!(fixture_from_run(SESSION, "r", &split).unwrap().fixture.cwd, None);

        let relative = vec![
            msg("user", "go", None, None),
            msg("assistant", "", calls(r#"[{"id":"1","name":"os","input":{"path":"src/x.rs"}}]"#), None),
        ];
        let export = fixture_from_run(SESSION, "r", &relative).unwrap();
        assert_eq!(export.fixture.cwd, None);
        assert!(export.touched.is_empty());

        let none = vec![msg("user", "go", None, None)];
        assert_eq!(fixture_from_run(SESSION, "r", &none).unwrap().fixture.cwd, None);
    }

    /// The first-call check pins the tool and its action; without an action
    /// argument it pins the tool alone.
    #[test]
    fn first_call_check_pins_tool_and_action() {
        let messages = vec![
            msg("user", "go", None, None),
            msg(
                "assistant",
                "",
                calls(r#"[{"id":"1","name":"os","input":{"resource":"file","action":"read","path":"/p/f"}},{"id":"2","name":"web","input":{"action":"search"}}]"#),
                None,
            ),
        ];
        let export = fixture_from_run(SESSION, "r", &messages).unwrap();
        let first = &export.fixture.prompt_assertions.first_call;
        assert_eq!(first.len(), 1);
        let check = first[0].check.as_ref().expect("program check");
        assert!(check.first_call);
        assert_eq!(check.tool, vec!["os".to_string()]);
        assert_eq!(check.arg.as_deref(), Some("action"));
        assert_eq!(check.equals, Some(serde_yaml::Value::String("read".into())));
        assert_eq!(first[0].text, "First tool call is os read");

        let no_action = vec![
            msg("user", "go", None, None),
            msg("assistant", "", calls(r#"[{"id":"1","name":"agent","input":{"resource":"memory"}}]"#), None),
        ];
        let export = fixture_from_run(SESSION, "r", &no_action).unwrap();
        let check = export.fixture.prompt_assertions.first_call[0].check.as_ref().unwrap();
        assert_eq!(check.tool, vec!["agent".to_string()]);
        assert_eq!(check.arg, None);
        assert_eq!(check.equals, None);

        let no_calls = vec![msg("user", "go", None, None)];
        let export = fixture_from_run(SESSION, "r", &no_calls).unwrap();
        assert!(export.fixture.prompt_assertions.first_call.is_empty());
    }

    /// Identity and the recovery contract: a stable hashed id, the name from
    /// the first prompt, the agent from the key, and a critical recovery
    /// assertion that names the exit reason.
    #[test]
    fn identity_and_recovery_name_the_run_and_its_exit() {
        let long_prompt = "word ".repeat(40);
        let messages = vec![msg("user", &long_prompt, None, None)];
        let export = fixture_from_run(SESSION, "repeated_tool_calls", &messages).unwrap();
        let f = &export.fixture;
        assert_eq!(f.id.len(), "replay-".len() + REPLAY_ID_HASH_CHARS);
        assert!(f.id.starts_with("replay-"));
        assert_eq!(
            f.id,
            fixture_from_run(SESSION, "other", &messages).unwrap().fixture.id,
            "the id depends on the session key alone"
        );
        assert_ne!(f.id, fixture_from_run("agent:emp2:web", "r", &messages).unwrap().fixture.id);
        assert!(f.name.ends_with("..."));
        assert!(f.name.chars().count() <= REPLAY_NAME_MAX_CHARS + "...".len());
        assert_eq!(f.agent.as_deref(), Some("emp1"));
        assert!(f.description.contains("repeated_tool_calls"));

        let recovery = &f.prompt_assertions.recovery;
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].severity, Severity::Critical);
        assert!(recovery[0].text.contains("must not end in repeated_tool_calls again"));
        assert!(recovery[0].text.contains("blocked"));

        let plain = fixture_from_run("eval:x:run-1:1", "r", &messages).unwrap();
        assert_eq!(plain.fixture.agent, None);
    }

    /// What the exporter writes is a fixture the harness loads back: the
    /// header comments survive as comments and every field round-trips.
    #[test]
    fn rendered_yaml_loads_as_a_fixture() {
        let messages = vec![
            msg("user", "Read the file", None, None),
            msg(
                "assistant",
                "",
                calls(r#"[{"id":"1","name":"os","input":{"resource":"file","action":"read","path":"/proj/a.txt"}}]"#),
                None,
            ),
        ];
        let export = fixture_from_run(SESSION, "reviewer_stop", &messages).unwrap();
        let yaml = render_yaml(&export).unwrap();
        assert!(yaml.starts_with("# Replay fixture"));
        assert!(yaml.contains("#   /proj/a.txt\n"));

        let loaded: Fixture = serde_yaml::from_str(&yaml).expect("exported YAML loads as a fixture");
        assert_eq!(loaded.id, export.fixture.id);
        assert_eq!(loaded.cwd.as_deref(), Some("/proj"));
        assert_eq!(loaded.agent.as_deref(), Some("emp1"));
        assert_eq!(loaded.conversation.len(), 1);
        let check = loaded.prompt_assertions.first_call[0].check.as_ref().unwrap();
        assert!(check.first_call);
        assert_eq!(check.tool, vec!["os".to_string()]);
        assert_eq!(loaded.prompt_assertions.recovery[0].severity, Severity::Critical);
        // Empty sections are not written (the header comment mentions
        // `setup:` in prose; the key itself would start a line).
        assert!(!yaml.contains("\nsetup:"));
        assert!(!yaml.contains("\nideal_behavior:"));
        assert!(!yaml.contains("\ntool_config:"));
    }
}
