//! Minimal LSP client — opportunistic enrichment, never a dependency
//! (PRD_CODING_HARNESS Pillar 3).
//!
//! Detects language servers already on PATH (rust-analyzer,
//! typescript-language-server, pyright-langserver, gopls, clangd) and speaks
//! just enough JSON-RPC over stdio to get diagnostics, definition, references
//! and hover. NEVER installs, downloads, or suggests installing a server —
//! when none is present every caller degrades to its tree-sitter answer.
//!
//! Lifecycle: one server per (workspace root, language), lazily spawned on
//! first use, shut down after 5 idle minutes. A crashed server is marked
//! unavailable for the rest of the session and is NEVER respawned — no
//! restart loops. Every child is killed on drop (same orphaned-process scar
//! as execute_tool's kill_on_drop: a child that outlives the call reparents
//! to launchd/init and never exits).
//!
//! Every call is bounded by a 2-second budget; a server that hasn't answered
//! by then yields `Unavailable::Timeout` and the caller states or omits that
//! factually. The handshake keeps progressing across calls, so a slow first
//! index turns into working diagnostics on a later call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Per-call wall-clock budget: spawn + handshake + request must fit or the
/// call returns `Unavailable::Timeout` (the chain then appends nothing).
const CALL_BUDGET: Duration = Duration::from_secs(2);

/// A server idle longer than this is shut down (shutdown/exit, then killed).
const IDLE_SHUTDOWN: Duration = Duration::from_secs(5 * 60);

// ── Public API ──────────────────────────────────────────────────────

/// One diagnostic as published by a server. Lines and columns are 1-based.
#[derive(Debug, Clone)]
pub struct Diag {
    pub line: u32,
    pub col: u32,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn from_lsp(n: Option<u64>) -> Self {
        match n {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            Some(4) => Severity::Hint,
            // LSP: absent severity is interpreted as an error by convention.
            _ => Severity::Error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        }
    }
}

/// The diagnostics a server published for one file, with the server named so
/// callers can attribute the verdict factually ("lsp (rust-analyzer): …").
#[derive(Debug, Clone)]
pub struct DiagReport {
    pub server: String,
    pub diagnostics: Vec<Diag>,
}

/// One location (definition/reference target). 1-based line and column.
#[derive(Debug, Clone)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub col: u32,
}

/// Why an LSP answer is not available. Distinguishes "nothing installed"
/// (the permanent, factual state of this machine) from "crashed this
/// session" (was up, died, will not be respawned) from "no answer within
/// the budget" (may succeed on a later call once the server has indexed).
#[derive(Debug, Clone)]
pub enum Unavailable {
    NoServer { lang: String },
    Crashed { server: String },
    Timeout { server: String },
}

/// The client surface both the file tool's verification chain and the `code`
/// tool call — trait-fronted so tests mock it and nothing here needs a real
/// server. `text` is the current buffer content (the caller already has it in
/// hand post-write); `line`/`col` are 1-based.
pub trait LspProvider: Send + Sync {
    fn diagnostics(&self, path: &Path, text: &str) -> Result<DiagReport, Unavailable>;
    fn definition(&self, path: &Path, text: &str, line: u32, col: u32)
    -> Result<Vec<Location>, Unavailable>;
    fn references(&self, path: &Path, text: &str, line: u32, col: u32)
    -> Result<Vec<Location>, Unavailable>;
    fn hover(&self, path: &Path, text: &str, line: u32, col: u32)
    -> Result<Option<String>, Unavailable>;
}

/// Provider that reports "no server" for every language. The unit-test
/// default (see [`default_provider`]) so file/code tests never spawn a real
/// server that happens to be installed on the dev machine.
pub struct NoServers;

impl LspProvider for NoServers {
    fn diagnostics(&self, path: &Path, _text: &str) -> Result<DiagReport, Unavailable> {
        Err(Unavailable::NoServer { lang: lang_label(path) })
    }
    fn definition(&self, path: &Path, _text: &str, _line: u32, _col: u32)
    -> Result<Vec<Location>, Unavailable> {
        Err(Unavailable::NoServer { lang: lang_label(path) })
    }
    fn references(&self, path: &Path, _text: &str, _line: u32, _col: u32)
    -> Result<Vec<Location>, Unavailable> {
        Err(Unavailable::NoServer { lang: lang_label(path) })
    }
    fn hover(&self, path: &Path, _text: &str, _line: u32, _col: u32)
    -> Result<Option<String>, Unavailable> {
        Err(Unavailable::NoServer { lang: lang_label(path) })
    }
}

/// The process-global manager — ONE client for every call site (the file
/// tool's chain and the `code` tool share servers; never a parallel client).
pub fn global() -> Arc<LspManager> {
    static GLOBAL: OnceLock<Arc<LspManager>> = OnceLock::new();
    GLOBAL.get_or_init(|| Arc::new(LspManager::new())).clone()
}

/// The provider production code wires in: the global manager normally,
/// [`NoServers`] under `cfg(test)` so unit tests are hermetic and serverless
/// (tests that need diagnostics inject a mock explicitly).
pub fn default_provider() -> Arc<dyn LspProvider> {
    #[cfg(test)]
    {
        Arc::new(NoServers)
    }
    #[cfg(not(test))]
    {
        global()
    }
}

/// Human label for the LSP-relevant language of `path`, for degradation
/// messages ("no language server for rust detected"). Falls back to the
/// tree-sitter language name, then the extension.
pub fn lang_label(path: &Path) -> String {
    if let Some((_, _, label)) = lang_for_path(&default_table(), path) {
        return label;
    }
    if let Some(lang) = syntax::Lang::from_path(path) {
        return lang.name().to_string();
    }
    path.extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "this file".to_string())
}

/// Detection rows for `nebo doctor`: (binary name, Some(version/found line)
/// when on PATH, None when not found). Detection only — never a spawn of the
/// actual server protocol.
pub fn detect_servers() -> Vec<(String, Option<String>)> {
    default_table()
        .iter()
        .map(|spec| {
            let found = find_server(&spec.bin, None);
            let detail = found.map(|path| {
                std::process::Command::new(&path)
                    .args(&spec.version_args)
                    .output()
                    .ok()
                    .and_then(|o| {
                        let line = String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if line.is_empty() { None } else { Some(line) }
                    })
                    .unwrap_or_else(|| format!("found ({})", path.display()))
            });
            (spec.bin.clone(), detail)
        })
        .collect()
}

/// Render one factual diagnostics line: `lsp (rust-analyzer): 0 diagnostics`
/// or `lsp (rust-analyzer): 1 error, 2 warnings — line 40: unused variable
/// `x`; …; 3 more omitted`. Errors sort first; at most `cap` items are shown
/// and the omission is stated — never a silent cut (CODE_AUDITOR §11).
pub fn render_diagnostics(report: &DiagReport, cap: usize) -> String {
    if report.diagnostics.is_empty() {
        return format!("lsp ({}): 0 diagnostics", report.server);
    }
    let mut diags = report.diagnostics.clone();
    diags.sort_by_key(|d| (d.severity, d.line, d.col));
    let mut counts: [usize; 4] = [0; 4];
    for d in &diags {
        counts[d.severity as usize] += 1;
    }
    let summary = [Severity::Error, Severity::Warning, Severity::Info, Severity::Hint]
        .iter()
        .filter(|s| counts[**s as usize] > 0)
        .map(|s| {
            let n = counts[*s as usize];
            format!("{n} {}{}", s.label(), if n == 1 { "" } else { "s" })
        })
        .collect::<Vec<_>>()
        .join(", ");
    let items = diags
        .iter()
        .take(cap)
        .map(|d| {
            let first_line = d.message.lines().next().unwrap_or("");
            format!("line {}: {}", d.line, crate::truncate_str(first_line, 200))
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut out = format!("lsp ({}): {summary} — {items}", report.server);
    if diags.len() > cap {
        out.push_str(&format!("; {} more omitted", diags.len() - cap));
    }
    out
}

// ── Detection table ─────────────────────────────────────────────────

/// One detectable server: the binary, its stdio args, how to ask its version
/// (doctor), and the (extension, LSP languageId, human label) rows it serves.
#[derive(Clone)]
pub struct ServerSpec {
    pub bin: String,
    pub args: Vec<String>,
    pub version_args: Vec<String>,
    pub exts: Vec<(String, String, String)>,
}

fn spec(
    bin: &str,
    args: &[&str],
    version_args: &[&str],
    exts: &[(&str, &str, &str)],
) -> ServerSpec {
    ServerSpec {
        bin: bin.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        version_args: version_args.iter().map(|s| s.to_string()).collect(),
        exts: exts
            .iter()
            .map(|(e, id, l)| (e.to_string(), id.to_string(), l.to_string()))
            .collect(),
    }
}

/// The ONE detection table — the manager, the doctor and the degradation
/// labels all read from here.
pub fn default_table() -> Vec<ServerSpec> {
    vec![
        spec("rust-analyzer", &[], &["--version"], &[("rs", "rust", "rust")]),
        // typescript-language-server speaks stdio only when asked to.
        spec(
            "typescript-language-server",
            &["--stdio"],
            &["--version"],
            &[
                ("ts", "typescript", "typescript"),
                ("tsx", "typescriptreact", "typescript"),
                ("js", "javascript", "javascript"),
            ],
        ),
        spec(
            "pyright-langserver",
            &["--stdio"],
            &["--version"],
            &[("py", "python", "python")],
        ),
        spec("gopls", &[], &["version"], &[("go", "go", "go")]),
        spec(
            "clangd",
            &[],
            &["--version"],
            &[
                ("c", "c", "c"),
                ("h", "c", "c"),
                ("cpp", "cpp", "c++"),
                ("cc", "cpp", "c++"),
                ("cxx", "cpp", "c++"),
                ("hpp", "cpp", "c++"),
            ],
        ),
    ]
}

/// (spec index, languageId, label) for `path`'s extension, or None when no
/// table row serves it.
fn lang_for_path(table: &[ServerSpec], path: &Path) -> Option<(usize, String, String)> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    for (i, s) in table.iter().enumerate() {
        for (e, id, label) in &s.exts {
            if *e == ext {
                return Some((i, id.clone(), label.clone()));
            }
        }
    }
    None
}

/// PATH-only lookup (`which`-style). `paths` overrides the PATH variable for
/// tests; production passes None (the process environment). Detection NEVER
/// installs or downloads anything.
fn find_server(bin: &str, paths: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let env_path = std::env::var_os("PATH");
    let search = paths.map(|p| p.to_os_string()).or(env_path)?;
    which::which_in(bin, Some(search), "/").ok()
}

/// Nearest ancestor of `path` holding a workspace marker, else the file's
/// parent directory. This keys the one-server-per-(root, language) map and
/// becomes the initialize rootUri.
fn find_root(path: &Path) -> PathBuf {
    const MARKERS: &[&str] = &[
        ".git",
        "Cargo.toml",
        "go.mod",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "compile_commands.json",
    ];
    let start = path.parent().unwrap_or(Path::new("/"));
    let mut dir = start;
    loop {
        if MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return start.to_path_buf(),
        }
    }
}

// ── JSON-RPC 2.0 framing (Content-Length over stdio) ────────────────

/// Write one framed message: `Content-Length: N\r\n\r\n<body>`.
fn write_message<W: Write>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one framed message. `Ok(None)` = clean EOF (the server exited).
fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok();
        }
    }
    let len = len.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// `file://` URI for a local path, percent-encoding everything outside the
/// unreserved set (plus `/`).
fn to_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                s.push(b as char)
            }
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// Local path for a `file://` URI (percent-decoded). Non-file URIs come back
/// as-is — a factual rendering beats dropping the location.
fn from_uri(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&raw[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── The one live server ─────────────────────────────────────────────

/// A write/read failure or reader-channel disconnect: the server is gone.
struct Crash;

/// Outcome of pumping one incoming message.
enum Pumped {
    /// A response to some request — the caller matches ids.
    Response(Value),
    /// A notification or server request, handled internally.
    Handled,
    /// Nothing arrived before the deadline.
    Deadline,
}

struct Server {
    bin: String,
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: Receiver<Value>,
    next_id: i64,
    /// initialize request id; `initialized` flips once its response arrives
    /// and the `initialized` notification is sent. The handshake progresses
    /// across calls, so a slow first index doesn't wedge the session.
    init_id: i64,
    initialized: bool,
    /// uri-path → didOpen version, for didOpen-vs-didChange bookkeeping.
    open_docs: HashMap<String, i64>,
    /// Latest publishDiagnostics per path, plus which paths published since
    /// the caller last cleared them (freshness gate for diagnostics()).
    diags: HashMap<String, Vec<Diag>>,
    fresh: std::collections::HashSet<String>,
    last_used: Instant,
}

impl Drop for Server {
    fn drop(&mut self) {
        // kill on drop — without this the server outlives the session forever
        // (reparents to launchd/init; see execute_tool's kill_on_drop scar).
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn spawn(spec: &ServerSpec, bin_path: &Path, root: &Path) -> std::io::Result<Self> {
        let mut cmd = std::process::Command::new(bin_path);
        cmd.args(&spec.args)
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("lsp-{}", spec.bin))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                while let Ok(Some(msg)) = read_message(&mut reader) {
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
                // EOF/parse error: dropping tx disconnects the channel, which
                // the pump reads as a crash.
            })?;

        let root_uri = to_uri(root);
        let mut server = Server {
            bin: spec.bin.clone(),
            child,
            stdin,
            rx,
            next_id: 2,
            init_id: 1,
            initialized: false,
            open_docs: HashMap::new(),
            diags: HashMap::new(),
            fresh: std::collections::HashSet::new(),
            last_used: Instant::now(),
        };
        server
            .send(&json!({
                "jsonrpc": "2.0",
                "id": server.init_id,
                "method": "initialize",
                "params": {
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {
                        "textDocument": {
                            "publishDiagnostics": {},
                            "synchronization": {}
                        }
                    },
                    "workspaceFolders": [{
                        "uri": root_uri,
                        "name": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "workspace".to_string())
                    }]
                }
            }))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "initialize write failed"))?;
        Ok(server)
    }

    fn send(&mut self, msg: &Value) -> Result<(), Crash> {
        write_message(&mut self.stdin, msg).map_err(|_| Crash)
    }

    /// Pump exactly ONE incoming message, so callers regain control the
    /// moment the state they wait on has changed — a diagnostics wait must
    /// return when the publish lands, not when the budget expires.
    /// Server→client requests are answered minimally (null / empty
    /// configuration), publishDiagnostics is folded into `diags`/`fresh`.
    fn pump_one(&mut self, deadline: Instant) -> Result<Pumped, Crash> {
        let now = Instant::now();
        if now >= deadline {
            return Ok(Pumped::Deadline);
        }
        let msg = match self.rx.recv_timeout(deadline - now) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => return Ok(Pumped::Deadline),
            Err(RecvTimeoutError::Disconnected) => return Err(Crash),
        };
        let method = msg.get("method").and_then(|m| m.as_str());
        match method {
            Some("textDocument/publishDiagnostics") => {
                let params = &msg["params"];
                let path = from_uri(params["uri"].as_str().unwrap_or(""));
                let list: Vec<Diag> = params["diagnostics"]
                    .as_array()
                    .map(|a| a.iter().filter_map(parse_diag).collect())
                    .unwrap_or_default();
                self.diags.insert(path.clone(), list);
                self.fresh.insert(path);
                Ok(Pumped::Handled)
            }
            Some(m) if msg.get("id").is_some() => {
                // Server→client request: the minimal truthful answer keeps
                // these five servers moving without capability negotiation.
                let result = if m == "workspace/configuration" {
                    let n = msg["params"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
                    json!(vec![Value::Null; n])
                } else {
                    Value::Null
                };
                self.send(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}))?;
                Ok(Pumped::Handled)
            }
            Some(_) => Ok(Pumped::Handled), // notification we don't track
            None => Ok(Pumped::Response(msg)),
        }
    }

    /// Wait for the response to request `id`. `Ok(None)` = deadline.
    fn wait_response(&mut self, id: i64, deadline: Instant) -> Result<Option<Value>, Crash> {
        loop {
            match self.pump_one(deadline)? {
                Pumped::Response(msg)
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(id) =>
                {
                    return Ok(Some(msg));
                }
                Pumped::Response(_) | Pumped::Handled => continue,
                Pumped::Deadline => return Ok(None),
            }
        }
    }

    /// Finish the initialize handshake if it hasn't completed yet.
    /// `Ok(false)` = still waiting at the deadline (Timeout for this call;
    /// the response will be picked up by a later one).
    fn ensure_ready(&mut self, deadline: Instant) -> Result<bool, Crash> {
        if self.initialized {
            return Ok(true);
        }
        if self.wait_response(self.init_id, deadline)?.is_none() {
            return Ok(false);
        }
        self.send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}))?;
        self.initialized = true;
        Ok(true)
    }

    /// didOpen the document, or didChange (full sync) if it's already open.
    fn open_or_update(&mut self, path: &str, lang_id: &str, text: &str) -> Result<(), Crash> {
        let uri = to_uri(Path::new(path));
        match self.open_docs.get_mut(path) {
            Some(version) => {
                *version += 1;
                let v = *version;
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {"uri": uri, "version": v},
                        "contentChanges": [{"text": text}]
                    }
                }))
            }
            None => {
                self.open_docs.insert(path.to_string(), 1);
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": lang_id,
                            "version": 1,
                            "text": text
                        }
                    }
                }))
            }
        }
    }

    /// Best-effort graceful stop before the killing Drop: shutdown request +
    /// exit notification. Never waited on — Drop reaps.
    fn stop(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "shutdown", "params": null}));
        let _ = self.send(&json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
    }
}

fn parse_diag(v: &Value) -> Option<Diag> {
    let start = &v["range"]["start"];
    Some(Diag {
        line: start["line"].as_u64().unwrap_or(0) as u32 + 1,
        col: start["character"].as_u64().unwrap_or(0) as u32 + 1,
        severity: Severity::from_lsp(v["severity"].as_u64()),
        message: v["message"].as_str()?.to_string(),
    })
}

/// Locations from a definition/references result: null, a single Location,
/// an array of Locations, or an array of LocationLinks.
fn parse_locations(v: &Value) -> Vec<Location> {
    fn one(v: &Value) -> Option<Location> {
        let (uri, range) = if let Some(u) = v.get("uri") {
            (u, &v["range"])
        } else if let Some(u) = v.get("targetUri") {
            (u, v.get("targetSelectionRange").unwrap_or(&v["targetRange"]))
        } else {
            return None;
        };
        let start = &range["start"];
        Some(Location {
            path: from_uri(uri.as_str()?),
            line: start["line"].as_u64().unwrap_or(0) as u32 + 1,
            col: start["character"].as_u64().unwrap_or(0) as u32 + 1,
        })
    }
    match v {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Hover contents in all their LSP shapes, flattened to plain text.
fn parse_hover(v: &Value) -> Option<String> {
    fn contents(v: &Value) -> Vec<String> {
        match v {
            Value::String(s) => vec![s.clone()],
            Value::Object(o) => o
                .get("value")
                .and_then(|s| s.as_str())
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            Value::Array(items) => items.iter().flat_map(contents).collect(),
            _ => Vec::new(),
        }
    }
    let parts = contents(&v["contents"]);
    let text = parts.join("\n").trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

// ── The manager ─────────────────────────────────────────────────────

enum Entry {
    Live(Server),
    /// Crashed this session — stays crashed; never respawned.
    Crashed { bin: String },
}

struct Inner {
    servers: HashMap<(PathBuf, String), Entry>,
    /// Spawn attempts, so tests can prove "crashed = never respawned".
    spawns: usize,
}

/// One server per (workspace root, language), lazily spawned, idle-shutdown,
/// crash-latched. Implements [`LspProvider`]; the process-global instance is
/// [`global`]. Calls serialize on one mutex — bounded by the 2s budget.
pub struct LspManager {
    table: Vec<ServerSpec>,
    budget: Duration,
    idle: Duration,
    inner: Mutex<Inner>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::with_table(default_table(), CALL_BUDGET, IDLE_SHUTDOWN)
    }

    fn with_table(table: Vec<ServerSpec>, budget: Duration, idle: Duration) -> Self {
        Self {
            table,
            budget,
            idle,
            inner: Mutex::new(Inner { servers: HashMap::new(), spawns: 0 }),
        }
    }

    #[cfg(test)]
    fn spawn_count(&self) -> usize {
        self.inner.lock().expect("lsp lock").spawns
    }

    /// Shut down servers idle past the threshold. Crash markers stay.
    fn sweep_idle(&self, inner: &mut Inner) {
        let idle = self.idle;
        inner.servers.retain(|_, entry| match entry {
            Entry::Live(s) if s.last_used.elapsed() > idle => {
                s.stop();
                false // Drop kills
            }
            _ => true,
        });
    }

    /// Spawn-or-fetch the server for `path`, run `f` against it inside the
    /// call budget, and latch a crash. The ONE lifecycle path every provider
    /// method goes through.
    fn with_server<T>(
        &self,
        path: &Path,
        f: impl FnOnce(&mut Server, &str, &str, Instant) -> Result<Option<T>, Crash>,
    ) -> Result<T, Unavailable> {
        let (idx, lang_id, label) = match lang_for_path(&self.table, path) {
            Some(v) => v,
            None => return Err(Unavailable::NoServer { lang: lang_label(path) }),
        };
        let spec = self.table[idx].clone();
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
        };
        let root = find_root(&abs);
        let key = (root.clone(), spec.bin.clone());
        let deadline = Instant::now() + self.budget;

        let mut inner = self.inner.lock().expect("lsp lock");
        self.sweep_idle(&mut inner);

        if !inner.servers.contains_key(&key) {
            let Some(bin_path) = find_server(&spec.bin, None) else {
                return Err(Unavailable::NoServer { lang: label });
            };
            inner.spawns += 1;
            match Server::spawn(&spec, &bin_path, &root) {
                Ok(s) => {
                    inner.servers.insert(key.clone(), Entry::Live(s));
                }
                Err(_) => {
                    inner.servers.insert(key.clone(), Entry::Crashed { bin: spec.bin.clone() });
                    return Err(Unavailable::Crashed { server: spec.bin });
                }
            }
        }

        let entry = inner.servers.get_mut(&key).expect("just inserted");
        let server = match entry {
            Entry::Crashed { bin } => {
                return Err(Unavailable::Crashed { server: bin.clone() });
            }
            Entry::Live(s) => s,
        };
        server.last_used = Instant::now();

        let outcome = (|| -> Result<Option<T>, Crash> {
            if !server.ensure_ready(deadline)? {
                return Ok(None); // handshake still pending: Timeout
            }
            f(server, &abs.to_string_lossy(), &lang_id, deadline)
        })();

        match outcome {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err(Unavailable::Timeout { server: spec.bin }),
            Err(Crash) => {
                inner.servers.insert(key, Entry::Crashed { bin: spec.bin.clone() });
                Err(Unavailable::Crashed { server: spec.bin })
            }
        }
    }

    /// Position-request plumbing shared by definition/references/hover.
    fn position_request(
        &self,
        method: &'static str,
        extra_params: Value,
        path: &Path,
        text: &str,
        line: u32,
        col: u32,
    ) -> Result<Value, Unavailable> {
        self.with_server(path, |server, doc_path, lang_id, deadline| {
            server.open_or_update(doc_path, lang_id, text)?;
            let id = server.next_id;
            server.next_id += 1;
            let mut params = json!({
                "textDocument": {"uri": to_uri(Path::new(doc_path))},
                "position": {"line": line.saturating_sub(1), "character": col.saturating_sub(1)}
            });
            if let (Value::Object(p), Value::Object(extra)) = (&mut params, extra_params) {
                p.extend(extra);
            }
            server.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
            match server.wait_response(id, deadline)? {
                Some(resp) => Ok(Some(resp.get("result").cloned().unwrap_or(Value::Null))),
                None => Ok(None),
            }
        })
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LspProvider for LspManager {
    fn diagnostics(&self, path: &Path, text: &str) -> Result<DiagReport, Unavailable> {
        let mut server_name = String::new();
        let diags = self.with_server(path, |server, doc_path, lang_id, deadline| {
            server_name = server.bin.clone();
            server.fresh.remove(doc_path);
            server.open_or_update(doc_path, lang_id, text)?;
            // Pump until the server publishes for THIS document or the
            // budget runs out. What it publishes is what we report.
            while !server.fresh.contains(doc_path) {
                if matches!(server.pump_one(deadline)?, Pumped::Deadline) {
                    return Ok(None);
                }
            }
            Ok(Some(server.diags.get(doc_path).cloned().unwrap_or_default()))
        })?;
        Ok(DiagReport { server: server_name, diagnostics: diags })
    }

    fn definition(
        &self,
        path: &Path,
        text: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<Location>, Unavailable> {
        let result = self.position_request("textDocument/definition", json!({}), path, text, line, col)?;
        Ok(parse_locations(&result))
    }

    fn references(
        &self,
        path: &Path,
        text: &str,
        line: u32,
        col: u32,
    ) -> Result<Vec<Location>, Unavailable> {
        let result = self.position_request(
            "textDocument/references",
            json!({"context": {"includeDeclaration": true}}),
            path,
            text,
            line,
            col,
        )?;
        Ok(parse_locations(&result))
    }

    fn hover(
        &self,
        path: &Path,
        text: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<String>, Unavailable> {
        let result = self.position_request("textDocument/hover", json!({}), path, text, line, col)?;
        Ok(parse_hover(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── JSON-RPC framing ────────────────────────────────────────────

    /// A message written with Content-Length framing reads back identical,
    /// and a stream of two messages yields both in order with clean EOF.
    #[test]
    fn framing_round_trips_over_in_memory_buffer() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"x": "héllo"}});
        let b = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        assert!(buf.starts_with(b"Content-Length: "), "header framing");

        let mut r = Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), a);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), b);
        assert!(read_message(&mut r).unwrap().is_none(), "clean EOF is None, not an error");
    }

    /// A frame with no Content-Length header is a protocol error, never a
    /// silent empty message.
    #[test]
    fn framing_missing_content_length_is_an_error() {
        let mut r = Cursor::new(b"X-Other: 1\r\n\r\n{}".to_vec());
        assert!(read_message(&mut r).is_err());
    }

    #[test]
    fn uri_round_trips_spaces_and_unicode() {
        let p = Path::new("/tmp/dir with space/héllo.rs");
        let uri = to_uri(p);
        assert!(!uri.contains(' '), "{uri}");
        assert_eq!(from_uri(&uri), p.to_string_lossy());
    }

    // ── Detection table ─────────────────────────────────────────────

    /// find_server sees a binary on the given PATH and misses one that
    /// isn't there — PATH-only lookup, no fallbacks.
    #[test]
    fn detection_path_hit_and_miss() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("rust-analyzer");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let paths = dir.path().as_os_str();
        let hit = find_server("rust-analyzer", Some(paths));
        assert!(
            hit.as_ref().is_some_and(|p| p.ends_with("rust-analyzer")),
            "{hit:?}"
        );
        assert_eq!(find_server("gopls", Some(paths)), None);
    }

    /// Every advertised server is in the table, with the extensions the PRD
    /// names, and the stdio-only servers carry their required args.
    #[test]
    fn detection_table_matches_prd() {
        let table = default_table();
        let bins: Vec<&str> = table.iter().map(|s| s.bin.as_str()).collect();
        assert_eq!(
            bins,
            [
                "rust-analyzer",
                "typescript-language-server",
                "pyright-langserver",
                "gopls",
                "clangd"
            ]
        );
        let by_bin = |b: &str| table.iter().find(|s| s.bin == b).unwrap();
        assert!(by_bin("pyright-langserver").args == ["--stdio"]);
        assert!(by_bin("typescript-language-server").args == ["--stdio"]);
        for (ext, lang) in [("rs", "rust"), ("tsx", "typescript"), ("py", "python"), ("go", "go"), ("cpp", "c++")] {
            let (_, _, label) = lang_for_path(&table, Path::new(&format!("f.{ext}"))).unwrap();
            assert_eq!(label, lang);
        }
        assert!(lang_for_path(&table, Path::new("f.json")).is_none());
    }

    // ── Lifecycle ───────────────────────────────────────────────────

    /// A server that dies is marked crashed for the session and is NEVER
    /// respawned: the second call fails fast with Crashed and the spawn
    /// count stays at one.
    #[test]
    fn crashed_server_is_latched_and_never_respawned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        // `false` exits immediately: spawn succeeds, the protocol dies.
        let table = vec![spec("false", &[], &["--version"], &[("rs", "rust", "rust")])];
        let mgr = LspManager::with_table(table, Duration::from_secs(2), IDLE_SHUTDOWN);

        let first = mgr.diagnostics(&file, "fn main() {}\n");
        assert!(
            matches!(first, Err(Unavailable::Crashed { ref server }) if server == "false"),
            "{first:?}"
        );
        assert_eq!(mgr.spawn_count(), 1);

        let second = mgr.diagnostics(&file, "fn main() {}\n");
        assert!(matches!(second, Err(Unavailable::Crashed { .. })), "{second:?}");
        assert_eq!(mgr.spawn_count(), 1, "a crashed server must never be respawned");
    }

    /// A language with no server on PATH is NoServer — named as "not
    /// installed", distinct from Crashed.
    #[test]
    fn missing_binary_is_no_server_not_crash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let table = vec![spec(
            "nebo-test-no-such-language-server",
            &[],
            &["--version"],
            &[("rs", "rust", "rust")],
        )];
        let mgr = LspManager::with_table(table, Duration::from_secs(2), IDLE_SHUTDOWN);
        let r = mgr.diagnostics(&file, "fn main() {}\n");
        assert!(matches!(r, Err(Unavailable::NoServer { ref lang }) if lang == "rust"), "{r:?}");
        assert_eq!(mgr.spawn_count(), 0);
    }

    /// A file whose extension no table row serves is NoServer with the
    /// tree-sitter language as its label.
    #[test]
    fn unserved_extension_is_no_server_with_label() {
        let mgr = LspManager::new();
        let r = mgr.diagnostics(Path::new("/tmp/x.json"), "{}");
        assert!(matches!(r, Err(Unavailable::NoServer { ref lang }) if lang == "json"), "{r:?}");
    }

    // ── Rendering ───────────────────────────────────────────────────

    fn d(line: u32, sev: Severity, msg: &str) -> Diag {
        Diag { line, col: 1, severity: sev, message: msg.to_string() }
    }

    /// Zero diagnostics render as the factual zero line; a non-empty list
    /// leads with severity counts, sorts errors first, and states omissions.
    #[test]
    fn render_diagnostics_zero_counts_and_cap() {
        let report = DiagReport { server: "rust-analyzer".into(), diagnostics: vec![] };
        assert_eq!(render_diagnostics(&report, 10), "lsp (rust-analyzer): 0 diagnostics");

        let report = DiagReport {
            server: "rust-analyzer".into(),
            diagnostics: vec![
                d(50, Severity::Warning, "unused variable `x`"),
                d(40, Severity::Error, "mismatched types"),
                d(60, Severity::Warning, "unused import"),
            ],
        };
        let line = render_diagnostics(&report, 2);
        assert!(line.starts_with("lsp (rust-analyzer): 1 error, 2 warnings — "), "{line}");
        assert!(line.contains("line 40: mismatched types"), "errors sort first: {line}");
        assert!(line.contains("; 1 more omitted"), "cap must be stated: {line}");
        assert!(!line.contains("unused import"), "{line}");
    }

    // ── Real server (env-gated) ─────────────────────────────────────

    /// The ONE integration test against a real server from PATH — clangd
    /// preferred (standalone, no project indexing), rust-analyzer as the
    /// fallback fixture. Run with:
    /// `cargo test -p nebo-tools --lib lsp -- --ignored`
    #[test]
    #[ignore = "requires a real language server on PATH (clangd or rust-analyzer)"]
    fn real_server_publishes_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let mgr =
            LspManager::with_table(default_table(), Duration::from_secs(60), IDLE_SHUTDOWN);
        if find_server("clangd", None).is_some() {
            let file = dir.path().join("probe.c");
            let text = "int add(int a, int b) { return a + b; }\nint bad(void) { return \"not an int\"; }\nint user(void) { return add(1, 2); }\n";
            std::fs::write(&file, text).unwrap();

            let report = mgr.diagnostics(&file, text).expect("diagnostics from clangd");
            assert_eq!(report.server, "clangd");
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|d| d.line == 2
                        && matches!(d.severity, Severity::Error | Severity::Warning)),
                "the bad return must be flagged: {:?}",
                report.diagnostics
            );

            // Position path: definition of `add` from its call site (line 3,
            // the `add` in `add(1, 2)` starts at col 26).
            let defs = mgr.definition(&file, text, 3, 26).expect("definition from clangd");
            assert!(
                defs.iter().any(|l| l.line == 1),
                "definition of add is on line 1: {defs:?}"
            );
            let hover = mgr.hover(&file, text, 3, 26).expect("hover from clangd");
            assert!(
                hover.as_deref().is_some_and(|h| h.contains("int")),
                "hover names the signature: {hover:?}"
            );
        } else if find_server("rust-analyzer", None).is_some() {
            std::fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
            std::fs::create_dir_all(dir.path().join("src")).unwrap();
            let file = dir.path().join("src/lib.rs");
            let text = "pub fn f() -> i32 { \"not an i32\" }\n";
            std::fs::write(&file, text).unwrap();

            let report = mgr.diagnostics(&file, text).expect("diagnostics from rust-analyzer");
            assert_eq!(report.server, "rust-analyzer");
            assert!(
                report.diagnostics.iter().any(|d| d.severity == Severity::Error),
                "type error expected: {:?}",
                report.diagnostics
            );
        } else {
            eprintln!("no language server on PATH — nothing to exercise");
        }
    }
}
