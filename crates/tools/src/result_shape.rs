//! What a tool result looks like to the model: the one spill path, the
//! saved-output preview, and the error shapes. Every door that runs a
//! tool goes through [`crate::Registry::execute`], which applies these.

use std::path::{Path, PathBuf};

use crate::registry::{PERSIST_THRESHOLD_CHARS, ToolResult};

/// How a result saved to a file begins: a preview of a large result, or an
/// old result cleared under context pressure.
pub const SAVED_OUTPUT: &str = "<saved-output>";

/// Characters of a persisted result shown inline.
pub const PREVIEW_CHARS: usize = 2_000;

/// Total result characters one assistant message's calls may bring back
/// inline; past it the largest results are persisted first.
pub const MESSAGE_RESULT_BUDGET: usize = 200_000;

/// An error longer than this keeps its head and tail.
const ERROR_CAP: usize = 10_000;
const ERROR_HALF: usize = 5_000;

/// The inline threshold for a tool's declared `max_result_chars`: its own
/// value, never above [`PERSIST_THRESHOLD_CHARS`]. `None` never persists.
pub fn threshold(max_result_chars: Option<usize>) -> Option<usize> {
    max_result_chars.map(|m| m.min(PERSIST_THRESHOLD_CHARS))
}

/// Where a session's persisted results live:
/// `<data_dir>/sessions/<session>/tool-results/`.
pub fn results_dir(session_id: &str) -> PathBuf {
    crate::checkpoint::session_dir(session_id).join("tool-results")
}

/// Shape a finished call's result for the model: an empty success names
/// the tool, a long error keeps head and tail, and a success over the
/// threshold is saved under `dir` (a session's [`results_dir`]) and previewed.
pub fn shape(tool: &str, dir: &Path, threshold: Option<usize>, result: &mut ToolResult) {
    if result.is_error {
        cap_error(&mut result.content);
        return;
    }
    if result.content.is_empty() {
        result.content = format!("({tool} returned no output)");
        return;
    }
    // A result carrying an image is never persisted: the image is the point.
    if result.image_url.is_some() {
        return;
    }
    if let Some(limit) = threshold
        && result.content.chars().count() > limit
    {
        result.content = persist(dir, &result.content);
    }
}

/// Save `content` in `dir` (a session's [`results_dir`]): the one place a
/// result that leaves the conversation is kept.
fn save(dir: &Path, content: &str) -> std::io::Result<PathBuf> {
    let path = dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir)?;
    crate::checkpoint::restrict_private(dir, true);
    std::fs::write(&path, content)?;
    crate::checkpoint::restrict_private(&path, false);
    Ok(path)
}

/// Save an old result cleared under context pressure and return what the
/// model sees in its place: where it was saved and
/// how to read it. `None` when it couldn't be saved.
pub fn persist_cleared(dir: &Path, content: &str) -> Option<String> {
    match save(dir, content) {
        Ok(path) => Some(format!(
            "{SAVED_OUTPUT}\nThis result was saved at: {}\nRead it with read_file if you need it.\n</saved-output>",
            path.display()
        )),
        Err(e) => {
            tracing::warn!(error = %e, "failed to save a cleared tool result");
            None
        }
    }
}

/// Save `content` in `dir` (a session's [`results_dir`]) and return what
/// the model sees in its place: the size, the path and a preview cut at a
/// newline. A failed write keeps the preview and says the rest is gone.
pub fn persist(dir: &Path, content: &str) -> String {
    let saved = save(dir, content);
    let size = human_size(content.len());
    let preview = preview(content);
    match saved {
        Ok(path) => format!(
            "{SAVED_OUTPUT}\nThe output is {size}, too long to show here. Saved in full at: {}\n\n\
             First 2KB:\n{preview}\n</saved-output>\n\
             Read the file a line range at a time with read_file (offset and limit), or search it with \
             run_command(command: \"grep -n '<text>' {0}\"); don't read it whole.",
            path.display()
        ),
        Err(e) => {
            tracing::warn!(error = %e, "failed to persist a large tool result");
            format!(
                "{SAVED_OUTPUT}\nThe output is {size}, too long to show here, and saving it failed ({e}); \
                 only the first 2KB is available.\n\nFirst 2KB:\n{preview}\n</saved-output>"
            )
        }
    }
}

/// The first [`PREVIEW_CHARS`] of `content`, cut back to a newline when one
/// falls in the second half.
fn preview(content: &str) -> &str {
    let end = content
        .char_indices()
        .nth(PREVIEW_CHARS)
        .map_or(content.len(), |(i, _)| i);
    let head = &content[..end];
    match head.rfind('\n') {
        Some(nl) if nl > end / 2 => &head[..nl],
        _ => head,
    }
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    }
}

/// An error over [`ERROR_CAP`] keeps its first and last [`ERROR_HALF`].
fn cap_error(content: &mut String) {
    if content.len() <= ERROR_CAP {
        return;
    }
    let head_end = types::strutil::floor_char_boundary(content, ERROR_HALF);
    let mut tail_start = content.len() - ERROR_HALF;
    while !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let cut = tail_start - head_end;
    *content = format!(
        "{}\n\n[{cut} characters truncated]\n\n{}",
        &content[..head_end],
        &content[tail_start..]
    );
}

/// Inside `<call_error>`: a call that never reached its tool.
pub fn call_error(message: &str) -> String {
    format!("<call_error>{message}</call_error>")
}

/// The call names no registered tool.
pub fn unknown_tool(name: &str) -> String {
    call_error(&format!("There is no tool named {name}."))
}

/// The call's input failed its tool's schema.
pub fn input_validation(tool: &str, issues: &[String]) -> String {
    call_error(&format!(
        "Invalid input for {tool}:\n{}",
        issues.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_result_is_saved_once_and_previewed() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/sess-1/tool-results");
        {
            let body: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
            let mut r = ToolResult::ok(body.clone());
            shape("run_command", &dir, threshold(Some(30_000)), &mut r);
            assert!(r.content.starts_with("<saved-output>\nThe output is "));
            assert!(r.content.contains("First 2KB:\nline 0\n"));
            let path = r
                .content
                .split("Saved in full at: ")
                .nth(1)
                .and_then(|s| s.lines().next())
                .unwrap();
            assert!(path.contains("/sessions/sess-1/tool-results/"), "{path}");
            assert_eq!(std::fs::read_to_string(path).unwrap(), body, "nothing is lost");
            assert!(r.content.len() < 3_000, "the preview is about 2KB");
        }
    }

    #[test]
    fn thresholds_cap_at_fifty_thousand_and_none_never_persists() {
        assert_eq!(threshold(Some(30_000)), Some(30_000));
        assert_eq!(threshold(Some(100_000)), Some(PERSIST_THRESHOLD_CHARS));
        assert_eq!(threshold(None), None);
        let mut r = ToolResult::ok("x".repeat(200_000));
        shape("read_file", Path::new("/nonexistent"), threshold(None), &mut r);
        assert_eq!(r.content.len(), 200_000, "a self-paging tool's result is untouched");
    }

    #[test]
    fn empty_success_names_the_tool_and_errors_keep_head_and_tail() {
        let mut r = ToolResult::ok("");
        shape("read_file", Path::new("/nonexistent"), Some(10), &mut r);
        assert_eq!(r.content, "(read_file returned no output)");
        let mut e = ToolResult::error(format!("{}{}", "a".repeat(8_000), "z".repeat(8_000)));
        shape("run_command", Path::new("/nonexistent"), Some(10), &mut e);
        assert!(e.content.starts_with("aaaa") && e.content.ends_with("zzzz"));
        assert!(e.content.contains("[6000 characters truncated]"), "{}", &e.content[4_990..5_040]);
        assert!(e.is_error);
    }

    #[test]
    fn error_shapes_are_wrapped() {
        assert_eq!(
            unknown_tool("bash"),
            "<call_error>There is no tool named bash.</call_error>"
        );
        assert_eq!(
            input_validation("read_file", &["The required parameter `path` is missing".into()]),
            "<call_error>Invalid input for read_file:\nThe required parameter `path` is missing</call_error>"
        );
    }
}
