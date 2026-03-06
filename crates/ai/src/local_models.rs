use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Download progress report for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_name: String,
    pub downloaded: i64,
    pub total: i64,
    pub percent: i32,
    pub bytes_per_sec: i64,
}

/// Progress callback type.
pub type ProgressFn = Box<dyn Fn(DownloadProgress) + Send + Sync>;

/// Describes a model available for download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelSpec {
    pub name: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub size: i64,
    pub priority: i32,
}

/// Info about a locally discovered model file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Default Qwen 3.5 Small Model Series managed by Janus.
/// Priority 0 = downloaded first (small, immediate fallback)
/// Priority 1 = downloaded next (capable agent base)
/// Priority 2 = downloaded last (full capability)
pub fn default_local_models() -> Vec<LocalModelSpec> {
    vec![
        LocalModelSpec {
            name: "qwen3.5-0.8b".into(),
            filename: "Qwen3.5-0.8B-Q4_K_M.gguf".into(),
            url: "https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/resolve/main/Qwen3.5-0.8B-Q4_K_M.gguf".into(),
            sha256: String::new(),
            size: 533_000_000,
            priority: 0,
        },
        LocalModelSpec {
            name: "qwen3.5-2b".into(),
            filename: "Qwen3.5-2B-Q4_K_M.gguf".into(),
            url: "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf".into(),
            sha256: String::new(),
            size: 1_600_000_000,
            priority: 1,
        },
        LocalModelSpec {
            name: "qwen3.5-4b".into(),
            filename: "Qwen3.5-4B-Q4_K_M.gguf".into(),
            url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q4_K_M.gguf".into(),
            sha256: String::new(),
            size: 2_700_000_000,
            priority: 1,
        },
        LocalModelSpec {
            name: "qwen3.5-9b".into(),
            filename: "Qwen3.5-9B-Q4_K_M.gguf".into(),
            url: "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf".into(),
            sha256: String::new(),
            size: 5_680_000_000,
            priority: 2,
        },
    ]
}

/// Scan a directory for `.gguf` model files and return info about each.
pub fn find_local_models(dir: &Path) -> Vec<LocalModelInfo> {
    let mut models = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return models,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gguf") {
            if let Ok(meta) = entry.metadata() {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                models.push(LocalModelInfo {
                    name,
                    path,
                    size: meta.len(),
                });
            }
        }
    }

    // Sort by size descending (largest first for auto-select)
    models.sort_by(|a, b| b.size.cmp(&a.size));
    models
}

/// Manages downloading and verifying local GGUF models.
pub struct ModelDownloader {
    models_dir: PathBuf,
    progress_tx: Option<mpsc::UnboundedSender<DownloadProgress>>,
    active: Mutex<HashMap<String, CancellationToken>>,
}

impl ModelDownloader {
    /// Create a downloader that stores models in the given directory.
    /// `progress_tx` receives download progress updates.
    pub fn new(models_dir: PathBuf, progress_tx: Option<mpsc::UnboundedSender<DownloadProgress>>) -> Self {
        Self {
            models_dir,
            progress_tx,
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the directory where models are stored.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Check if a model file exists and is at least 50% of expected size.
    pub fn is_model_available(&self, spec: &LocalModelSpec) -> bool {
        let path = self.models_dir.join(&spec.filename);
        match std::fs::metadata(&path) {
            Ok(meta) => {
                if spec.size > 0 && (meta.len() as i64) < spec.size / 2 {
                    return false;
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Full path for a model file.
    pub fn model_path(&self, spec: &LocalModelSpec) -> PathBuf {
        self.models_dir.join(&spec.filename)
    }

    /// Download all default models that aren't already present.
    /// Priority 0 sequential, 1+ parallel within same level.
    pub async fn download_all(&self, cancel: CancellationToken) -> Result<(), String> {
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| format!("cannot create models directory: {}", e))?;

        let specs = default_local_models();
        let max_pri = specs.iter().map(|s| s.priority).max().unwrap_or(0);

        for pri in 0..=max_pri {
            if cancel.is_cancelled() {
                return Ok(());
            }

            let level_specs: Vec<_> = specs
                .iter()
                .filter(|s| s.priority == pri && !self.is_model_available(s))
                .cloned()
                .collect();

            if pri == 0 {
                // Sequential for priority 0
                for spec in &level_specs {
                    if let Err(e) = self.download(cancel.clone(), spec).await {
                        warn!(name = %spec.name, error = %e, "failed to download model");
                    }
                }
            } else {
                // Parallel for priority 1+
                let mut handles = Vec::new();
                for spec in level_specs {
                    let token = cancel.clone();
                    let dir = self.models_dir.clone();
                    let tx = self.progress_tx.clone();
                    handles.push(tokio::spawn(async move {
                        let dl = ModelDownloader::new(dir, tx);
                        if let Err(e) = dl.download(token, &spec).await {
                            warn!(name = %spec.name, error = %e, "failed to download model");
                        }
                    }));
                }
                for h in handles {
                    let _ = h.await;
                }
            }
        }

        Ok(())
    }

    /// Download a single model with resume support and progress tracking.
    pub async fn download(&self, cancel: CancellationToken, spec: &LocalModelSpec) -> Result<(), String> {
        // Check if already downloading
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(&spec.name) {
                return Err(format!("download already in progress for {}", spec.name));
            }
            active.insert(spec.name.clone(), cancel.clone());
        }

        let result = self.do_download(cancel, spec).await;

        // Remove from active
        {
            let mut active = self.active.lock().unwrap();
            active.remove(&spec.name);
        }

        result
    }

    async fn do_download(&self, cancel: CancellationToken, spec: &LocalModelSpec) -> Result<(), String> {
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| format!("cannot create models directory: {}", e))?;

        let dest_path = self.models_dir.join(&spec.filename);
        let tmp_path = dest_path.with_extension("gguf.download");

        // Check for partial download (resume)
        let existing_size = std::fs::metadata(&tmp_path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        info!(name = %spec.name, url = %spec.url, resume_from = existing_size, "downloading model");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(0)) // No timeout for large files
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let mut request = client.get(&spec.url);
        if existing_size > 0 {
            request = request.header("Range", format!("bytes={}-", existing_size));
        }

        let resp = request
            .send()
            .await
            .map_err(|e| format!("download failed: {}", e))?;

        let status = resp.status().as_u16();
        if status != 200 && status != 206 {
            return Err(format!("download failed: HTTP {}", status));
        }

        // If server doesn't support range, start fresh
        let resume = existing_size > 0 && status == 206;
        let content_length = resp.content_length().unwrap_or(0) as i64;
        let total_size = if resume {
            content_length + existing_size
        } else {
            content_length
        };

        // Open file
        let file = if resume {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&tmp_path)
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
        }
        .map_err(|e| format!("cannot create file: {}", e))?;

        let mut writer = std::io::BufWriter::new(file);
        let mut downloaded = if resume { existing_size } else { 0 };
        let mut last_report = std::time::Instant::now();
        let mut last_bytes = downloaded;

        use futures::StreamExt;
        use std::io::Write;
        let mut stream = resp.bytes_stream();

        while let Some(chunk_result) = stream.next().await {
            if cancel.is_cancelled() {
                return Err("download cancelled".into());
            }

            let chunk = chunk_result.map_err(|e| format!("read error at {} bytes: {}", downloaded, e))?;
            writer
                .write_all(&chunk)
                .map_err(|e| format!("write error: {}", e))?;
            downloaded += chunk.len() as i64;

            // Report progress every 500ms
            let now = std::time::Instant::now();
            if now.duration_since(last_report) >= std::time::Duration::from_millis(500) {
                let elapsed = now.duration_since(last_report).as_secs_f64();
                let bytes_per_sec = if elapsed > 0.0 {
                    ((downloaded - last_bytes) as f64 / elapsed) as i64
                } else {
                    0
                };
                let percent = if total_size > 0 {
                    (downloaded * 100 / total_size) as i32
                } else {
                    0
                };

                if let Some(ref tx) = self.progress_tx {
                    let _ = tx.send(DownloadProgress {
                        model_name: spec.name.clone(),
                        downloaded,
                        total: total_size,
                        percent,
                        bytes_per_sec,
                    });
                }

                last_report = now;
                last_bytes = downloaded;
            }
        }

        writer.flush().map_err(|e| format!("flush error: {}", e))?;
        drop(writer);

        // Verify checksum if provided
        if !spec.sha256.is_empty() {
            info!(name = %spec.name, "verifying checksum");
            verify_checksum(&tmp_path, &spec.sha256)?;
        }

        // Atomic rename
        std::fs::rename(&tmp_path, &dest_path)
            .map_err(|e| format!("failed to finalize download: {}", e))?;

        info!(name = %spec.name, size = downloaded, "model download complete");

        // Final progress report
        if let Some(ref tx) = self.progress_tx {
            let _ = tx.send(DownloadProgress {
                model_name: spec.name.clone(),
                downloaded,
                total: downloaded,
                percent: 100,
                bytes_per_sec: 0,
            });
        }

        Ok(())
    }

    /// Cancel an active download by model name.
    pub fn cancel_download(&self, model_name: &str) {
        let active = self.active.lock().unwrap();
        if let Some(token) = active.get(model_name) {
            token.cancel();
        }
    }
}

/// Verify SHA256 checksum of a file.
fn verify_checksum(path: &Path, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot open file for checksum: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 256 * 1024];

    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read error during checksum: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let actual = hex::encode(hasher.finalize());
    if actual != expected {
        return Err(format!("expected {}, got {}", expected, actual));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_find_local_models() {
        let dir = std::env::temp_dir().join("nebo_test_local_models");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Create fake GGUF files
        fs::write(dir.join("model-a.gguf"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("model-b.gguf"), vec![0u8; 200]).unwrap();
        fs::write(dir.join("not-a-model.txt"), "hello").unwrap();

        let models = find_local_models(&dir);
        assert_eq!(models.len(), 2);
        // Sorted by size descending
        assert_eq!(models[0].name, "model-b");
        assert_eq!(models[0].size, 200);
        assert_eq!(models[1].name, "model-a");
        assert_eq!(models[1].size, 100);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_model_available() {
        let dir = std::env::temp_dir().join("nebo_test_model_avail");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let spec = LocalModelSpec {
            name: "test".into(),
            filename: "test.gguf".into(),
            url: String::new(),
            sha256: String::new(),
            size: 1000,
            priority: 0,
        };

        let dl = ModelDownloader::new(dir.clone(), None);

        // Not available — doesn't exist
        assert!(!dl.is_model_available(&spec));

        // Too small (< 50% of expected)
        fs::write(dir.join("test.gguf"), vec![0u8; 100]).unwrap();
        assert!(!dl.is_model_available(&spec));

        // Big enough
        fs::write(dir.join("test.gguf"), vec![0u8; 600]).unwrap();
        assert!(dl.is_model_available(&spec));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_models_catalog() {
        let models = default_local_models();
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].priority, 0);
        assert!(models[0].size < models[1].size);
    }

    #[test]
    fn test_verify_checksum() {
        let dir = std::env::temp_dir().join("nebo_test_checksum");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("test.bin");
        fs::write(&path, b"hello world").unwrap();

        // Correct SHA256 of "hello world"
        let correct = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_checksum(&path, correct).is_ok());

        // Wrong checksum
        assert!(verify_checksum(&path, "0000000000000000").is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
