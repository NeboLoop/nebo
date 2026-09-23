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
    /// A text field's placeholder: the name it keeps while its value changes.
    #[serde(default)]
    pub placeholder: Option<String>,
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
/// What an act expects to find at its path: the element's role and stable
/// label. The backend re-identifies before acting and refuses a stale or
/// ambiguous target; `None` acts on the path as recorded.
pub type Expect<'a> = Option<(&'a str, &'a str)>;

pub async fn act(app: &str, window: usize, path: &str, action: &str, expect: Expect<'_>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::act_raw(app, window, path, action, expect).await;
    #[cfg(target_os = "linux")]
    return linux::act_raw(app, window, path, action).await;
    #[cfg(target_os = "windows")]
    return windows::act_raw(app, window, path, action).await;
    #[allow(unreachable_code)]
    {
        let _ = (app, window, path, action, expect);
        Err("no native accessibility backend on this platform".into())
    }
}

/// Set the value of an editable node (text fields, sliders) without typing.
/// Bring the element's window to the front before physical input. Only
/// macOS knows windows by id; elsewhere the app is raised by other means.
pub async fn raise(app: &str, window: usize) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::raise_raw(app, window).await;
    #[allow(unreachable_code)]
    {
        let _ = (app, window);
        Ok(())
    }
}

pub async fn set_value(app: &str, window: usize, path: &str, value: &str, expect: Expect<'_>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::set_raw(app, window, path, value, expect).await;
    #[cfg(target_os = "linux")]
    return linux::set_raw(app, window, path, value).await;
    #[cfg(target_os = "windows")]
    return windows::set_raw(app, window, path, value).await;
    #[allow(unreachable_code)]
    {
        let _ = (app, window, path, value, expect);
        Err("no native accessibility backend on this platform".into())
    }
}

/// One line of text read from an image. `frame` is in the IMAGE's pixels
/// (top-left origin), never screen points: the caller maps it onto the
/// capture the line came from.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct TextLine {
    pub text: String,
    pub frame: [i64; 4],
    #[serde(default = "one")]
    pub confidence: f64,
}

fn one() -> f64 {
    1.0
}

/// Parse a backend's text output: one JSON object per line, `{"text","frame","confidence"}`.
pub fn parse_text_lines(lines: &str) -> Result<Vec<TextLine>, String> {
    let mut out = Vec::new();
    for line in lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let t: TextLine =
            serde_json::from_str(line).map_err(|e| format!("bad text line {line:?}: {e}"))?;
        if !t.text.trim().is_empty() && t.frame[2] > 0 && t.frame[3] > 0 {
            out.push(t);
        }
    }
    Ok(out)
}

/// Tesseract's `tsv` output (level 5 rows are words) grouped into lines by
/// (block, paragraph, line): the words joined by spaces, the box their union,
/// the confidence their mean. Words tesseract could not read (conf -1) are
/// skipped. Pure, so the Linux backend's only parsing is tested everywhere.
pub fn tesseract_tsv_to_lines(tsv: &str) -> Vec<TextLine> {
    let mut lines: Vec<((u32, u32, u32), Vec<String>, [i64; 4], Vec<f64>)> = Vec::new();
    for row in tsv.lines().skip(1) {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let word = c[11].trim();
        let conf: f64 = c[10].parse().unwrap_or(-1.0);
        if word.is_empty() || conf < 0.0 {
            continue;
        }
        let key = (c[2].parse().unwrap_or(0), c[3].parse().unwrap_or(0), c[4].parse().unwrap_or(0));
        let (x, y, w, h): (i64, i64, i64, i64) = (
            c[6].parse().unwrap_or(0),
            c[7].parse().unwrap_or(0),
            c[8].parse().unwrap_or(0),
            c[9].parse().unwrap_or(0),
        );
        match lines.last_mut() {
            Some((k, words, f, confs)) if *k == key => {
                words.push(word.to_string());
                let x2 = (f[0] + f[2]).max(x + w);
                let y2 = (f[1] + f[3]).max(y + h);
                f[0] = f[0].min(x);
                f[1] = f[1].min(y);
                f[2] = x2 - f[0];
                f[3] = y2 - f[1];
                confs.push(conf);
            }
            _ => lines.push((key, vec![word.to_string()], [x, y, w, h], vec![conf])),
        }
    }
    lines
        .into_iter()
        .map(|(_, words, frame, confs)| TextLine {
            text: words.join(" "),
            frame,
            confidence: confs.iter().sum::<f64>() / confs.len() as f64 / 100.0,
        })
        .collect()
}

/// A window as the platform sees it: frame in screen points, the id the
/// screen capturer takes (when the platform has one), and whether its app is
/// in front.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct WindowInfo {
    pub frame: [i64; 4],
    #[serde(default)]
    pub window_id: Option<u64>,
    #[serde(default)]
    pub windows: usize,
    #[serde(default)]
    pub frontmost: bool,
}

pub fn parse_window(line: &str) -> Result<WindowInfo, String> {
    let line = line.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let w: WindowInfo = serde_json::from_str(line).map_err(|e| format!("bad window line {line:?}: {e}"))?;
    if w.frame[2] <= 0 || w.frame[3] <= 0 {
        return Err(format!("window has no size: {:?}", w.frame));
    }
    Ok(w)
}

/// `app`'s window `index` (1-based). `Err` when the platform cannot say.
pub async fn window(app: &str, index: usize) -> Result<WindowInfo, String> {
    #[cfg(target_os = "macos")]
    let raw = macos::window_raw(app, index).await?;
    #[cfg(not(target_os = "macos"))]
    let raw: String = {
        let _ = (app, index);
        return Err("window ids are not read on this platform".into());
    };
    parse_window(&raw)
}

/// Read the text in an image file. `Err` means the platform's recognizer is
/// unavailable or failed; the caller says so and carries on without it.
pub async fn text(image: &std::path::Path) -> Result<Vec<TextLine>, String> {
    #[cfg(target_os = "macos")]
    let raw = macos::text_raw(image).await?;
    #[cfg(target_os = "linux")]
    let raw = linux::text_raw(image).await?;
    #[cfg(target_os = "windows")]
    let raw = windows::text_raw(image).await?;
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let raw: String = {
        let _ = image;
        return Err("no text recognizer on this platform".into());
    };
    parse_text_lines(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_line_carries_frame_and_optional_id() {
        let w = parse_window("{\"frame\":[10,20,300,200],\"windows\":2,\"frontmost\":false,\"window_id\":4711}\n").unwrap();
        assert_eq!((w.frame, w.window_id, w.windows, w.frontmost), ([10, 20, 300, 200], Some(4711), 2, false));
        let w = parse_window("{\"frame\":[0,0,1,1]}").unwrap();
        assert_eq!(w.window_id, None, "no id is a window that is captured by region");
        assert!(parse_window("{\"frame\":[0,0,0,5]}").is_err(), "a zero-size frame is not a window");
    }

    #[test]
    fn text_lines_parse_and_drop_empty_or_zero_sized() {
        let raw = r#"{"text":"Save","frame":[10,20,40,14],"confidence":0.98}
{"text":"  ","frame":[1,1,5,5],"confidence":0.5}
{"text":"zero","frame":[1,1,0,5]}
{"text":"Cancel","frame":[60,20,52,14]}"#;
        let t = parse_text_lines(raw).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].text, "Save");
        assert_eq!(t[1].confidence, 1.0, "confidence defaults to 1");
        assert!(parse_text_lines("{\"nope\":1}").is_err());
    }

    #[test]
    fn tesseract_words_group_into_lines_with_a_union_box() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t10\t20\t30\t12\t95\tHello\n\
5\t1\t1\t1\t1\t2\t45\t21\t40\t11\t85\tworld\n\
5\t1\t1\t1\t1\t3\t90\t20\t5\t12\t-1\t\n\
5\t1\t1\t1\t2\t1\t10\t40\t50\t12\t70\tSecond\n\
4\t1\t1\t1\t3\t0\t0\t0\t0\t0\t-1\t\n";
        let t = tesseract_tsv_to_lines(tsv);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].text, "Hello world");
        assert_eq!(t[0].frame, [10, 20, 75, 12]);
        assert!((t[0].confidence - 0.90).abs() < 1e-9);
        assert_eq!(t[1].text, "Second");
    }

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
