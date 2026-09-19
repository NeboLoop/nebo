//! Compile-on-first-use Swift helpers — the ONE mechanism for every embedded
//! Swift source (PIM, accessibility). The source is compiled with `swiftc`
//! into the data dir's `bin/`, the path is cached per helper name, and a
//! source-hash file beside the binary triggers a recompile when the embedded
//! source changes (a Nebo update). No `swiftc` → `None` with a warning; the
//! caller falls back and says so.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;

static HELPERS: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

/// Path to the compiled helper `name`, compiling `source` if needed.
pub async fn ensure_compiled(name: &str, source: &str, frameworks: &[&str]) -> Option<PathBuf> {
    let mut cache = HELPERS.get_or_init(|| Mutex::new(HashMap::new())).lock().await;
    if let Some(path) = cache.get(name) {
        if path.exists() {
            return Some(path.clone());
        }
    }
    let bin_dir = config::data_dir().ok()?.join("bin");
    std::fs::create_dir_all(&bin_dir).ok()?;
    let binary_path = bin_dir.join(name);
    let hash_path = bin_dir.join(format!("{name}.hash"));
    let hash = fnv1a(source.as_bytes()).to_string();

    if binary_path.exists() {
        if std::fs::read_to_string(&hash_path).map(|s| s.trim() == hash).unwrap_or(false) {
            cache.insert(name.to_string(), binary_path.clone());
            return Some(binary_path);
        }
        tracing::info!(name, "helper source changed, recompiling…");
        let _ = std::fs::remove_file(&binary_path);
    }

    let source_path = bin_dir.join(format!("{name}.swift"));
    std::fs::write(&source_path, source).ok()?;
    tracing::info!(name, "compiling native Swift helper…");
    let mut cmd = tokio::process::Command::new("swiftc");
    cmd.arg("-O");
    for f in frameworks {
        cmd.args(["-framework", f]);
    }
    let output = cmd.arg("-o").arg(&binary_path).arg(&source_path).output().await;
    let _ = std::fs::remove_file(&source_path);
    match output {
        Ok(o) if o.status.success() => {
            let _ = std::fs::write(&hash_path, &hash);
            tracing::info!(name, path = %binary_path.display(), "Swift helper compiled");
            cache.insert(name.to_string(), binary_path.clone());
            Some(binary_path)
        }
        Ok(o) => {
            tracing::warn!(name, stderr = %String::from_utf8_lossy(&o.stderr), "Swift helper compilation failed");
            None
        }
        Err(e) => {
            tracing::warn!(name, %e, "swiftc not found");
            None
        }
    }
}
