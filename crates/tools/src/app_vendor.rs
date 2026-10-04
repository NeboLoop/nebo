//! npm packages saved into a Lane A app when it is written.
//!
//! `update_employee` compiles the page's modules; every bare import they make
//! (and the React the page runs on) is fetched here from esm.sh once, with
//! the modules it imports in turn, and written under the app's
//! `ui/vendor/<pkg>@<version>-<hash>.js`. The compiled imports then name
//! those files, so the app opens offline and never waits on esm.sh at run
//! time. `src/vendor.lock.json` records which version each specifier got
//! (an unpinned one is pinned to what esm.sh resolved the first time) and
//! which file holds each esm.sh module, so a later write reuses the files
//! instead of fetching again and every module shares one copy (one React).
//!
//! A package that cannot be fetched (offline, esm.sh down) is left on its
//! esm.sh address, as before, and the tool result says so: a write never
//! fails for being offline.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

/// The lockfile, relative to the app's folder.
pub const LOCK_FILE: &str = "src/vendor.lock.json";
/// Where saved packages go, relative to `ui/`.
const VENDOR_DIR: &str = "vendor";
/// One build target for every saved module, instead of esm.sh guessing it
/// from our client's User-Agent.
const TARGET: &str = "es2022";
/// A package graph past either limit is not saved (it stays on esm.sh).
const MAX_FILES: usize = 400;
const MAX_BYTES: usize = 40 * 1024 * 1024;
/// Modules fetched at once.
const PARALLEL: usize = 8;

/// Where packages are fetched from. Tests point it at a local server.
#[derive(Clone, Debug)]
pub struct Registry {
    /// esm.sh origin, no trailing slash.
    pub esm: String,
    /// The Tailwind play CDN, no trailing slash.
    pub tailwind: String,
}

impl Default for Registry {
    fn default() -> Self {
        Self { esm: "https://esm.sh".into(), tailwind: render::TAILWIND_CDN.into() }
    }
}

/// `src/vendor.lock.json`.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Lock {
    /// Specifier as written (`recharts`, `three@0.170.0`) → what it got.
    #[serde(default)]
    pub imports: BTreeMap<String, Locked>,
    /// esm.sh module (path and query) → its file under `ui/`.
    #[serde(default)]
    pub modules: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Locked {
    pub version: String,
    /// Under `ui/`, e.g. `vendor/recharts@3.10.1-k4p2x7m0q3.js`.
    pub file: String,
}

/// What [`vendor`] did.
#[derive(Default, Debug)]
pub struct Vendored {
    /// Specifier → its file under `ui/`, for the compiler.
    pub map: BTreeMap<String, String>,
    /// New files to write under `ui/`.
    pub files: Vec<(PathBuf, Vec<u8>)>,
    pub lock: Lock,
    /// `name@version` of each package saved by this write.
    pub saved: Vec<String>,
    /// Specifier and why it was not saved; it loads from esm.sh.
    pub missed: Vec<(String, String)>,
}

impl Vendored {
    /// Write the lockfile into the app's folder (nothing when no package
    /// was ever saved).
    pub fn save_lock(&self, agent_dir: &Path) -> std::io::Result<()> {
        if self.lock == Lock::default() {
            return Ok(());
        }
        let path = agent_dir.join(LOCK_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.lock).map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")
    }

    /// One or two sentences for the tool result; empty when nothing changed.
    pub fn note(&self) -> String {
        let mut note = String::new();
        if !self.saved.is_empty() {
            note.push_str(&format!(
                " Packages saved into the app (ui/{VENDOR_DIR}/, pinned in {LOCK_FILE}; they load offline): {}.",
                self.saved.join(", ")
            ));
        }
        if !self.missed.is_empty() {
            let list: Vec<String> = self.missed.iter().map(|(s, why)| format!("{s} ({why})")).collect();
            note.push_str(&format!(
                " Could not save {}: {} still load{} from esm.sh at run time and need network. Send the same files again when online to save them.",
                list.join(", "),
                if self.missed.len() == 1 { "it" } else { "they" },
                if self.missed.len() == 1 { "s" } else { "" },
            ));
        }
        note
    }
}

/// Save every specifier in `specs` into the app (see the module docs).
/// `agent_dir` is the app's folder when it already exists: its lockfile and
/// saved files are reused. Never fails; what could not be fetched is in
/// [`Vendored::missed`].
pub async fn vendor(specs: &BTreeSet<String>, agent_dir: Option<&Path>, registry: &Registry) -> Vendored {
    let mut out = Vendored { lock: agent_dir.map(read_lock).unwrap_or_default(), ..Default::default() };
    if specs.is_empty() {
        return out;
    }
    let ui = agent_dir.map(|d| d.join("ui"));
    let on_disk = |file: &str| ui.as_ref().is_some_and(|u| u.join(file).is_file());
    // A saved module whose file is gone is fetched again.
    out.lock.modules.retain(|_, file| on_disk(file));
    let client = match tls::http_client()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            out.missed = specs.iter().map(|s| (s.clone(), e.to_string())).collect();
            return out;
        }
    };
    // Files this write adds count as present for the next specifier.
    let mut fresh: HashSet<String> = HashSet::new();
    // After a network failure the rest is not tried: one timeout, not one per package.
    let mut offline: Option<String> = None;
    for spec in specs {
        if let Some(locked) = out.lock.imports.get(spec)
            && (on_disk(&locked.file) || fresh.contains(&locked.file))
        {
            out.map.insert(spec.clone(), locked.file.clone());
            continue;
        }
        if let Some(why) = &offline {
            out.missed.push((spec.clone(), why.clone()));
            continue;
        }
        let pinned = pin(spec, &out.lock);
        let fetched = if spec == render::TAILWIND_CDN {
            fetch_tailwind(&client, registry, out.lock.imports.get(spec).map(|l| l.version.as_str())).await
        } else {
            fetch_graph(&client, registry, spec, &pinned, &out.lock.modules).await
        };
        match fetched {
            Ok(got) => {
                for (key, file) in &got.modules {
                    out.lock.modules.insert(key.clone(), file.clone());
                }
                for (file, bytes) in got.files {
                    if fresh.insert(file.clone()) {
                        out.files.push((PathBuf::from(file), bytes));
                    }
                }
                let saved = format!("{}@{}", package_name(spec), got.version);
                if !out.saved.contains(&saved) {
                    out.saved.push(saved);
                }
                out.map.insert(spec.clone(), got.entry.clone());
                out.lock.imports.insert(spec.clone(), Locked { version: got.version, file: got.entry });
            }
            Err(e) => {
                if e.network {
                    offline = Some(e.reason.clone());
                }
                out.missed.push((spec.clone(), e.reason));
            }
        }
    }
    out
}

fn read_lock(agent_dir: &Path) -> Lock {
    std::fs::read_to_string(agent_dir.join(LOCK_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// `name`, `version` and `/subpath` of a bare specifier:
/// `@scope/pkg@1.2/sub` → (`@scope/pkg`, `1.2`, `/sub`).
fn split_spec(spec: &str) -> (&str, Option<&str>, &str) {
    let name_end = if spec.starts_with('@') {
        spec.match_indices('/').nth(1).map(|(i, _)| i).unwrap_or(spec.len())
    } else {
        spec.find('/').unwrap_or(spec.len())
    };
    let (head, sub) = spec.split_at(name_end);
    match head[1..].find('@') {
        Some(at) => (&head[..at + 1], Some(&head[at + 2..]), sub),
        None => (head, None, sub),
    }
}

fn package_name(spec: &str) -> &str {
    if spec == render::TAILWIND_CDN { "tailwindcss" } else { split_spec(spec).0 }
}

/// An unpinned specifier gets the version its package was locked to, so
/// `recharts` and later `recharts/x` share one copy. React is pinned by the
/// compiler already.
fn pin(spec: &str, lock: &Lock) -> String {
    let (name, version, sub) = split_spec(spec);
    if version.is_some() || name == "react" || name == "react-dom" || spec == render::TAILWIND_CDN {
        return spec.to_string();
    }
    lock.imports
        .iter()
        .find(|(k, _)| k.as_str() != render::TAILWIND_CDN && split_spec(k).0 == name)
        .map(|(_, l)| format!("{name}@{}{sub}", l.version))
        .unwrap_or_else(|| spec.to_string())
}

struct Fetched {
    version: String,
    /// The specifier's own file under `ui/`.
    entry: String,
    /// `(file under ui/, content)` for every module fetched.
    files: Vec<(String, Vec<u8>)>,
    /// esm.sh module key → file, for the lockfile.
    modules: Vec<(String, String)>,
}

#[derive(Debug)]
struct FetchError {
    reason: String,
    /// The network failed (offline, timeout), as opposed to a bad answer.
    network: bool,
}

impl FetchError {
    fn http(e: reqwest::Error) -> Self {
        Self { reason: format!("offline or unreachable: {e}"), network: true }
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Result<(String, Option<String>, String), FetchError> {
    let resp = client.get(url).send().await.map_err(FetchError::http)?;
    if !resp.status().is_success() {
        return Err(FetchError { reason: format!("{url} answered {}", resp.status()), network: false });
    }
    let esm_path = resp.headers().get("x-esm-path").and_then(|v| v.to_str().ok()).map(String::from);
    let final_url = resp.url().to_string();
    let body = resp.text().await.map_err(FetchError::http)?;
    Ok((body, esm_path, final_url))
}

/// The Tailwind play CDN is one classic script: no imports to follow. The
/// CDN redirects to the current version (`/3.4.17`), which is the pin.
async fn fetch_tailwind(client: &reqwest::Client, registry: &Registry, version: Option<&str>) -> Result<Fetched, FetchError> {
    let url = match version {
        Some(v) => format!("{}/{v}", registry.tailwind),
        None => registry.tailwind.clone(),
    };
    let (body, _, final_url) = get(client, &url).await?;
    let version = version
        .map(String::from)
        .or_else(|| {
            let last = final_url.trim_end_matches('/').rsplit('/').next()?;
            last.chars().next().is_some_and(|c| c.is_ascii_digit()).then(|| last.to_string())
        })
        .unwrap_or_else(|| "latest".into());
    let file = vendor_file(&format!("tailwindcss@{version}"), body.as_bytes());
    Ok(Fetched { version, entry: file.clone(), files: vec![(file, body.into_bytes())], modules: Vec::new() })
}

/// Fetch the specifier's esm.sh module and, breadth first, every esm.sh
/// module it imports that is not saved yet; name each file by its content;
/// rewrite every import between them to the saved file (all live flat in
/// `ui/vendor/`, so `./<file>`).
async fn fetch_graph(
    client: &reqwest::Client,
    registry: &Registry,
    spec: &str,
    pinned: &str,
    saved: &BTreeMap<String, String>,
) -> Result<Fetched, FetchError> {
    let mut entry_key = render::esm_url(pinned).trim_start_matches("https://esm.sh").to_string();
    entry_key.push_str(if entry_key.contains('?') { "&target=" } else { "?target=" });
    entry_key.push_str(TARGET);

    let mut bodies: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([entry_key.clone()]);
    let mut frontier = vec![entry_key.clone()];
    let mut version: Option<String> = None;
    let mut bytes = 0usize;
    while !frontier.is_empty() {
        let got: Vec<_> = stream::iter(frontier.drain(..).map(|key| {
            let url = format!("{}{key}", registry.esm);
            async move { (key, get(client, &url).await) }
        }))
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
        for (key, result) in got {
            let (body, esm_path, _) = result?;
            if key == entry_key {
                version = esm_path
                    .as_deref()
                    .and_then(|p| split_spec(p.trim_start_matches('/')).1.map(String::from))
                    .or_else(|| banner_version(&body));
            }
            for dep in imports_in(&body, &key, registry) {
                if !saved.contains_key(&dep) && seen.insert(dep.clone()) {
                    frontier.push(dep);
                }
            }
            bytes += body.len();
            bodies.push((key, body));
        }
        if bodies.len() > MAX_FILES || bytes > MAX_BYTES {
            return Err(FetchError {
                reason: format!("more than {MAX_FILES} modules or {} MB", MAX_BYTES / 1024 / 1024),
                network: false,
            });
        }
    }
    let version = version.ok_or_else(|| FetchError {
        reason: "esm.sh did not say which version it served".into(),
        network: false,
    })?;

    let (name, _, sub) = split_spec(spec);
    let mut names: HashMap<String, String> = saved.iter().map(|(k, f)| (k.clone(), f.clone())).collect();
    for (key, body) in &bodies {
        let label = if *key == entry_key { format!("{name}@{version}{sub}") } else { module_label(key) };
        names.insert(key.clone(), vendor_file(&label, body.as_bytes()));
    }
    let mut files = Vec::new();
    let mut modules = Vec::new();
    for (key, body) in &bodies {
        let rewritten = IMPORT_RE.replace_all(body, |caps: &regex::Captures| {
            let whole = &caps[0];
            let spec = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str()).unwrap_or_default();
            match module_key(spec, key, registry).and_then(|k| names.get(&k)) {
                Some(file) => whole.replacen(spec, &format!("./{}", file.trim_start_matches(&format!("{VENDOR_DIR}/"))), 1),
                None => whole.to_string(),
            }
        });
        let file = names[key].clone();
        modules.push((key.clone(), file.clone()));
        files.push((file, rewritten.into_owned().into_bytes()));
    }
    Ok(Fetched { version, entry: names[&entry_key].clone(), files, modules })
}

/// A static or dynamic import in esm.sh's (often minified) output:
/// `from"/x"`, `import "/x"`, `import("/x")`, `export * from "/x"`.
static IMPORT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"\b(?:from|import)\s*\(?\s*(?:"([^"\n]+)"|'([^'\n]+)')"#).expect("import regex")
});

/// The esm.sh module keys (path and query) `body` imports. Anything not on
/// esm.sh (a bare name, another host) is left alone.
fn imports_in(body: &str, from: &str, registry: &Registry) -> Vec<String> {
    IMPORT_RE
        .captures_iter(body)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .filter_map(|m| module_key(m.as_str(), from, registry))
        .collect()
}

/// `spec` imported from module `from`, as a module key on the registry, or
/// None when it is not an esm.sh module.
fn module_key(spec: &str, from: &str, registry: &Registry) -> Option<String> {
    let spec = spec.strip_prefix("https://esm.sh").unwrap_or(spec);
    let spec = spec.strip_prefix(registry.esm.as_str()).unwrap_or(spec);
    if !(spec.starts_with('/') || spec.starts_with("./") || spec.starts_with("../")) || spec.starts_with("//") {
        return None;
    }
    let base = url::Url::parse(&format!("{}{from}", registry.esm)).ok()?;
    let url = base.join(spec).ok()?;
    let origin = url::Url::parse(&registry.esm).ok()?;
    if url.origin() != origin.origin() {
        return None;
    }
    Some(match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    })
}

/// `/* esm.sh - recharts@3.10.1 */` → `3.10.1`.
fn banner_version(body: &str) -> Option<String> {
    let rest = body.split_once("esm.sh - ")?.1;
    let spec = rest.split(|c: char| c.is_whitespace() || c == '*').next()?;
    split_spec(spec).1.map(String::from)
}

/// A readable name for a dependency module from its key:
/// `/react-dom@18.3.1/X-ab/es2022/client.mjs` → `react-dom@18.3.1-client`.
fn module_label(key: &str) -> String {
    let path = key.split('?').next().unwrap_or(key).trim_start_matches('/');
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let mut pkg = parts.next().unwrap_or("module").to_string();
    if pkg.starts_with('@')
        && let Some(next) = parts.next()
    {
        pkg = format!("{pkg}/{next}");
    }
    let rest: Vec<&str> = parts.filter(|s| !s.starts_with("X-") && *s != TARGET).collect();
    let stem = rest
        .last()
        .map(|f| f.trim_end_matches(".mjs").trim_end_matches(".js").trim_end_matches(".bundle"))
        .unwrap_or("");
    let base = split_spec(&pkg).0.rsplit('/').next().unwrap_or("").to_string();
    if stem.is_empty() || stem == base { pkg } else { format!("{pkg}-{stem}") }
}

/// `vendor/<label>-<hash>.js`: the label made safe for a file name, the hash
/// from the content in the shape the app server caches for a year
/// (lowercase letters and digits, a digit between two letters).
fn vendor_file(label: &str, content: &[u8]) -> String {
    let mut clean = String::new();
    for c in label.trim_start_matches('@').trim_end_matches(".mjs").trim_end_matches(".js").chars().filter(|c| !matches!(c, '^' | '~')) {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_') { c } else { '-' };
        if !(c == '-' && clean.ends_with('-')) {
            clean.push(c);
        }
    }
    let clean: String = clean.trim_matches('-').chars().take(80).collect();
    let mut h = xxhash_rust::xxh3::xxh3_64(content);
    let hash: String = (0..10)
        .map(|i| {
            let c = if i % 2 == 0 { (b'a' + (h % 26) as u8) as char } else { (b'0' + (h % 10) as u8) as char };
            h /= 26;
            c
        })
        .collect();
    format!("{VENDOR_DIR}/{clean}-{hash}.js")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A local esm.sh: path (with query) → (X-ESM-Path, body). Every
    /// request is counted, so a test can tell what was fetched.
    pub(crate) struct Mock {
        pub(crate) origin: String,
        pub(crate) hits: Arc<Mutex<Vec<String>>>,
    }

    pub(crate) async fn mock(routes: Vec<(String, Option<&str>, String)>) -> Mock {
        let routes: HashMap<String, (Option<String>, String)> =
            routes.into_iter().map(|(p, h, b)| (p, (h.map(String::from), b))).collect();
        let routes = Arc::new(routes);
        let hits = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (r, h) = (routes.clone(), hits.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let (r, h) = (r.clone(), h.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    h.lock().unwrap().push(path.clone());
                    let resp = match r.get(&path) {
                        Some((esm, body)) => format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/javascript\r\n{}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            esm.as_ref().map(|p| format!("x-esm-path: {p}\r\n")).unwrap_or_default(),
                            body.len()
                        ),
                        None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string(),
                    };
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        Mock { origin, hits }
    }

    pub(crate) fn registry(origin: &str) -> Registry {
        Registry { esm: origin.to_string(), tailwind: format!("{origin}/tw") }
    }

    fn specs(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const DEPS: &str = "deps=react@18.3.1,react-dom@18.3.1&target=es2022";

    /// esm.sh's shape: a stub that re-exports the built module, which
    /// imports React by its esm.sh path, which a second package imports too.
    fn routes() -> Vec<(String, Option<&'static str>, String)> {
        vec![
            (
                format!("/chartlib?{DEPS}"),
                Some("/chartlib@2.1.0/X-ZHJl/es2022/chartlib.mjs"),
                "/* esm.sh - chartlib@2.1.0 */\nimport \"/react@18.3.1/es2022/react.mjs\";\nexport * from \"/chartlib@2.1.0/X-ZHJl/es2022/chartlib.mjs\";\n".into(),
            ),
            (
                "/chartlib@2.1.0/X-ZHJl/es2022/chartlib.mjs".into(),
                None,
                "import{a as r}from\"/react@18.3.1/es2022/react.mjs\";import\"/node/process.mjs\";const l=()=>import(\"./lazy.mjs\");export const Chart=r;\n".into(),
            ),
            ("/chartlib@2.1.0/X-ZHJl/es2022/lazy.mjs".into(), None, "export const lazy=1;\n".into()),
            ("/node/process.mjs".into(), None, "export default {};\n".into()),
            (
                "/react@18.3.1?target=es2022".into(),
                Some("/react@18.3.1/es2022/react.mjs"),
                "/* esm.sh - react@18.3.1 */\nexport * from \"/react@18.3.1/es2022/react.mjs\";\nexport { default } from \"/react@18.3.1/es2022/react.mjs\";\n".into(),
            ),
            ("/react@18.3.1/es2022/react.mjs".into(), None, "export const a=1;export default {a};\n".into()),
            (
                format!("/chartlib@2.1.0/extra?{DEPS}"),
                Some("/chartlib@2.1.0/X-ZHJl/es2022/extra.mjs"),
                "export * from \"/chartlib@2.1.0/X-ZHJl/es2022/extra.mjs\";\n".into(),
            ),
            ("/chartlib@2.1.0/X-ZHJl/es2022/extra.mjs".into(), None, "import\"/react@18.3.1/es2022/react.mjs\";export const x=1;\n".into()),
        ]
    }

    async fn serve() -> Mock {
        mock(routes()).await
    }

    fn file<'a>(v: &'a Vendored, name: &str) -> &'a str {
        let (_, bytes) = v.files.iter().find(|(p, _)| p.to_string_lossy() == name).unwrap_or_else(|| panic!("{name} not written: {:?}", v.files.iter().map(|f| &f.0).collect::<Vec<_>>()));
        std::str::from_utf8(bytes).unwrap()
    }

    /// An unpinned package is pinned to the version esm.sh served; it and
    /// every esm.sh module it imports, statically or dynamically, are saved
    /// under vendor/ with their imports rewritten to the saved files, and
    /// React is one file shared by both specifiers.
    #[tokio::test]
    async fn packages_and_their_esm_imports_are_saved_and_rewritten() {
        let m = serve().await;
        let v = vendor(&specs(&["chartlib", "react"]), None, &registry(&m.origin)).await;
        assert!(v.missed.is_empty(), "{:?}", v.missed);
        assert_eq!(v.saved, vec!["chartlib@2.1.0", "react@18.3.1"]);
        assert_eq!(v.lock.imports["chartlib"].version, "2.1.0");
        let entry = &v.map["chartlib"];
        assert!(entry.starts_with("vendor/chartlib@2.1.0-") && entry.ends_with(".js"), "{entry}");
        let react = &v.lock.modules["/react@18.3.1/es2022/react.mjs"];
        let react_name = react.trim_start_matches("vendor/");
        assert!(react_name.starts_with("react@18.3.1-"), "{react}");

        let stub = file(&v, entry);
        let built = &v.lock.modules["/chartlib@2.1.0/X-ZHJl/es2022/chartlib.mjs"];
        assert!(stub.contains(&format!("\"./{react_name}\"")), "{stub}");
        assert!(stub.contains(&format!("\"./{}\"", built.trim_start_matches("vendor/"))), "{stub}");
        let body = file(&v, built);
        assert!(body.contains(&format!("from\"./{react_name}\"")), "minified import rewritten: {body}");
        assert!(body.contains("import(\"./chartlib@2.1.0-lazy-"), "dynamic relative import saved: {body}");
        assert!(body.contains("import\"./node-process-"), "{body}");
        assert!(!body.contains("\"/"), "no esm.sh path left: {body}");
        // React's stub re-exports the same file the chart imports: one React.
        let react_stub = file(&v, &v.map["react"]);
        assert!(react_stub.contains(&format!("\"./{react_name}\"")), "{react_stub}");
        let reacts = v.files.iter().filter(|(p, _)| p.to_string_lossy() == *react).count();
        assert_eq!(reacts, 1);
        // Every name is one the app server caches for a year.
        for (p, _) in &v.files {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let hash = name.trim_end_matches(".js").rsplit_once('-').unwrap().1.to_string();
            assert_eq!(hash.len(), 10, "{name}");
            assert!(hash.as_bytes().windows(3).any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_digit() && w[2].is_ascii_lowercase()), "{name}");
        }
    }

    /// The lockfile is reused: a second write fetches nothing for what is
    /// saved, and a new subpath of a locked package is fetched at the locked
    /// version, sharing the already-saved React instead of a second copy.
    #[tokio::test]
    async fn the_lockfile_is_reused_and_pins_later_imports() {
        let m = serve().await;
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(&m.origin);
        let first = vendor(&specs(&["chartlib", "react"]), Some(dir.path()), &reg).await;
        for (p, bytes) in &first.files {
            let target = dir.path().join("ui").join(p);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, bytes).unwrap();
        }
        first.save_lock(dir.path()).unwrap();
        let lock: Lock = serde_json::from_str(&std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap()).unwrap();
        assert_eq!(lock, first.lock);

        m.hits.lock().unwrap().clear();
        let again = vendor(&specs(&["chartlib", "react"]), Some(dir.path()), &reg).await;
        assert!(m.hits.lock().unwrap().is_empty(), "fetched again: {:?}", m.hits.lock().unwrap());
        assert!(again.files.is_empty() && again.saved.is_empty());
        assert_eq!(again.map, first.map);

        let sub = vendor(&specs(&["chartlib/extra"]), Some(dir.path()), &reg).await;
        let hits = m.hits.lock().unwrap().clone();
        assert!(hits.contains(&format!("/chartlib@2.1.0/extra?{DEPS}")), "pinned to the lock: {hits:?}");
        assert!(!hits.iter().any(|h| h.starts_with("/react")), "React reused: {hits:?}");
        let react = first.lock.modules["/react@18.3.1/es2022/react.mjs"].trim_start_matches("vendor/").to_string();
        let extra = file(&sub, &sub.lock.modules["/chartlib@2.1.0/X-ZHJl/es2022/extra.mjs"]);
        assert!(extra.contains(&format!("import\"./{react}\"")), "{extra}");
        assert!(sub.lock.imports.contains_key("chartlib"), "earlier entries kept");
    }

    /// Offline: nothing is saved, every package is reported, and the next
    /// one is not tried after the first network failure.
    #[tokio::test]
    async fn offline_leaves_packages_on_esm_sh_and_says_so() {
        // A port nothing listens on: connection refused at once.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let v = vendor(&specs(&["chartlib", "react"]), None, &registry(&origin)).await;
        assert!(v.map.is_empty() && v.files.is_empty() && v.saved.is_empty());
        assert_eq!(v.missed.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), vec!["chartlib", "react"]);
        let note = v.note();
        assert!(note.contains("Could not save chartlib") && note.contains("esm.sh at run time"), "{note}");
    }

    /// A package esm.sh does not have is reported without stopping the rest.
    #[tokio::test]
    async fn a_missing_package_does_not_stop_the_others() {
        let m = serve().await;
        let v = vendor(&specs(&["nope", "react"]), None, &registry(&m.origin)).await;
        assert_eq!(v.missed.len(), 1);
        assert_eq!(v.missed[0].0, "nope");
        assert!(v.map.contains_key("react"));
    }

    /// The Tailwind CDN is saved as one script, pinned to the version the
    /// CDN redirected to.
    #[tokio::test]
    async fn tailwind_is_saved_at_its_version() {
        let m = mock(vec![("/tw/3.4.17".into(), None, "(()=>{tw})();".into())]).await;
        let reg = Registry { esm: m.origin.clone(), tailwind: format!("{}/tw/3.4.17", m.origin) };
        let v = vendor(&specs(&[render::TAILWIND_CDN]), None, &reg).await;
        assert!(v.missed.is_empty(), "{:?}", v.missed);
        assert_eq!(v.lock.imports[render::TAILWIND_CDN].version, "3.4.17");
        assert!(v.map[render::TAILWIND_CDN].starts_with("vendor/tailwindcss@3.4.17-"));
        assert_eq!(v.saved, vec!["tailwindcss@3.4.17"]);
    }

    #[test]
    fn specifiers_split_into_name_version_and_subpath() {
        assert_eq!(split_spec("recharts"), ("recharts", None, ""));
        assert_eq!(split_spec("three@0.170.0/examples/x.js"), ("three", Some("0.170.0"), "/examples/x.js"));
        assert_eq!(split_spec("@react-three/fiber@8/x"), ("@react-three/fiber", Some("8"), "/x"));
        assert_eq!(split_spec("@scope/pkg"), ("@scope/pkg", None, ""));
        assert_eq!(module_label("/react-dom@18.3.1/X-ab/es2022/client.mjs"), "react-dom@18.3.1-client");
        assert_eq!(module_label("/react@18.3.1/es2022/react.mjs"), "react@18.3.1");
        assert_eq!(banner_version("/* esm.sh - react@18.3.1/jsx-runtime */\n").as_deref(), Some("18.3.1"));
        assert!(vendor_file("clsx@^2.1.1", b"x").starts_with("vendor/clsx@2.1.1-"));
    }
}
