//! Native accessibility walk — one JSON-lines contract on every platform.
//!
//! A platform backend (`macos`, `linux`, `windows`) produces the lines; this
//! module parses them into [`AxTree`]. The vocabulary is macOS's on every
//! platform (roles `AXButton`, `AXTextField`, …; actions `AXPress`,
//! `AXSetValue`, …) so the rest of the desktop tool has one set of names.
//!
//! Wire format, one JSON object per line:
//!   header  {"app":"Finder","pid":412,"windows":2}
//!   node    {"path":"0.3.2","role":"AXButton","title":"Back","value":null,"desc":null,
//!            "frame":[x,y,w,h],"actions":["AXPress"],"enabled":true,"focused":false}
//!   footer  {"truncated":false,"elapsed_ms":210}
//! `frame` is in screen points. `path` is the child-index path from the
//! window, valid only for the walk that produced it.

use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct AxNode {
    pub path: String,
    pub role: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    pub frame: [i64; 4],
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxTree {
    pub app: String,
    pub pid: i32,
    pub windows: usize,
    pub nodes: Vec<AxNode>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct WalkOpts {
    pub window: usize,
    pub depth: usize,
    pub max: usize,
    pub timeout: Duration,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self { window: 1, depth: 30, max: 400, timeout: Duration::from_secs(2) }
    }
}

/// Parse the JSON-lines output of a backend. The header is required; a
/// missing footer means the walk was cut off, which is reported as truncated
/// rather than guessed at.
pub fn parse_tree(lines: &str) -> Result<AxTree, String> {
    let mut tree = AxTree::default();
    let mut saw_header = false;
    let mut saw_footer = false;
    for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("bad ax line {line:?}: {e}"))?;
        if !saw_header {
            tree.app = v["app"].as_str().unwrap_or("").to_string();
            tree.pid = v["pid"].as_i64().unwrap_or(0) as i32;
            tree.windows = v["windows"].as_u64().unwrap_or(0) as usize;
            saw_header = true;
        } else if v.get("elapsed_ms").is_some() || v.get("truncated").is_some() {
            tree.truncated = v["truncated"].as_bool().unwrap_or(false);
            tree.elapsed_ms = v["elapsed_ms"].as_u64().unwrap_or(0);
            saw_footer = true;
        } else {
            let node: AxNode =
                serde_json::from_value(v).map_err(|e| format!("bad ax node {line:?}: {e}"))?;
            tree.nodes.push(node);
        }
    }
    if !saw_header {
        return Err("ax helper produced no output".into());
    }
    if !saw_footer {
        tree.truncated = true;
    }
    Ok(tree)
}

/// Walk `app`'s window. `Err` means the native backend is unavailable or
/// failed; the caller decides what to fall back to and says which it used.
pub async fn tree(app: &str, opts: &WalkOpts) -> Result<AxTree, String> {
    #[cfg(target_os = "macos")]
    let raw = macos::tree_raw(app, opts).await?;
    #[cfg(target_os = "linux")]
    let raw = linux::tree_raw(app, opts).await?;
    #[cfg(target_os = "windows")]
    let raw = windows::tree_raw(app, opts).await?;
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let raw: String = {
        let _ = (app, opts);
        return Err("no native accessibility backend on this platform".into());
    };
    parse_tree(&raw)
}

/// Perform an accessibility action (`AXPress`, `AXShowMenu`, …) on the node at
/// `path` from the most recent walk of `app`'s window.
pub async fn act(app: &str, window: usize, path: &str, action: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::act_raw(app, window, path, action).await;
    #[cfg(target_os = "linux")]
    return linux::act_raw(app, window, path, action).await;
    #[cfg(target_os = "windows")]
    return windows::act_raw(app, window, path, action).await;
    #[allow(unreachable_code)]
    {
        let _ = (app, window, path, action);
        Err("no native accessibility backend on this platform".into())
    }
}

/// Set the value of an editable node (text fields, sliders) without typing.
pub async fn set_value(app: &str, window: usize, path: &str, value: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::set_raw(app, window, path, value).await;
    #[cfg(target_os = "linux")]
    return linux::set_raw(app, window, path, value).await;
    #[cfg(target_os = "windows")]
    return windows::set_raw(app, window, path, value).await;
    #[allow(unreachable_code)]
    {
        let _ = (app, window, path, value);
        Err("no native accessibility backend on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINDER: &str = r#"{"app":"Finder","pid":412,"windows":2}
{"path":"0","role":"AXToolbar","title":"","value":null,"desc":null,"frame":[120,80,1040,52],"actions":[],"enabled":true,"focused":false}
{"path":"0.0","role":"AXButton","title":"Back","value":null,"desc":"go back","frame":[144,94,40,28],"actions":["AXPress"],"enabled":true,"focused":false}
{"path":"1.2","role":"AXTextField","title":"Search","value":"","desc":null,"frame":[980,92,160,28],"actions":["AXSetValue","AXConfirm"],"enabled":true,"focused":true}
{"truncated":false,"elapsed_ms":210}"#;

    #[test]
    fn parses_header_nodes_and_footer() {
        let t = parse_tree(FINDER).unwrap();
        assert_eq!((t.app.as_str(), t.pid, t.windows), ("Finder", 412, 2));
        assert_eq!(t.nodes.len(), 3);
        assert_eq!(t.nodes[1].actions, vec!["AXPress"]);
        assert_eq!(t.nodes[1].frame, [144, 94, 40, 28]);
        assert!(t.nodes[2].focused);
        assert_eq!(t.nodes[2].value.as_deref(), Some(""));
        assert!(!t.truncated);
        assert_eq!(t.elapsed_ms, 210);
    }

    #[test]
    fn a_missing_footer_is_reported_as_truncated() {
        let cut = FINDER.lines().take(3).collect::<Vec<_>>().join("\n");
        let t = parse_tree(&cut).unwrap();
        assert_eq!(t.nodes.len(), 2);
        assert!(t.truncated, "a walk that died mid-way must not read as complete");
    }

    #[test]
    fn no_output_is_an_error_not_an_empty_tree() {
        assert!(parse_tree("").is_err());
        assert!(parse_tree("   \n").is_err());
    }

    #[test]
    fn a_malformed_line_fails_loudly() {
        let bad = format!("{}\n{{\"path\":1}}\n", FINDER.lines().next().unwrap());
        assert!(parse_tree(&bad).unwrap_err().contains("bad ax node"));
    }
}
