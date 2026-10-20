use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::{ApiKey, ProviderError};

/// Trait for embedding providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider identifier.
    fn id(&self) -> &str;

    /// Number of dimensions in the embedding vectors.
    fn dimensions(&self) -> usize;

    /// Embed one or more texts, returning one vector per text.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
}

/// Most texts one embeddings request carries. Every caller hands over all
/// its texts at once (a whole backfill batch of memories); the provider splits
/// them into requests of this size.
const MAX_INPUTS_PER_REQUEST: usize = 128;

/// Embeddings requests in flight at once, per provider. Saves, recall
/// queries, the consolidation check and the backfill all share these, so a
/// boot backfill landing on a busy minute cannot stack requests up.
const MAX_IN_FLIGHT: usize = 2;

/// Attempts per request, and the pause before each retry when the server
/// names no wait.
const MAX_ATTEMPTS: usize = 3;
const BACKOFF: [Duration; MAX_ATTEMPTS - 1] = [Duration::from_millis(500), Duration::from_secs(2)];

/// The longest `Retry-After` waited out in place. A longer one fails the call
/// at once and holds every caller off until it has passed.
const MAX_INLINE_WAIT: Duration = Duration::from_secs(30);

/// How long every caller is held off after a refusal (429, 408, 5xx) that
/// outlasted the retries and named no wait.
const DEFAULT_HOLD: Duration = Duration::from_secs(60);

/// The ONE transport every embedding provider sends through: bounded
/// concurrency, retries that honour `Retry-After`, and a shared hold-off so a
/// refusing server gets one request, not one per waiting caller.
struct EmbedGate {
    permits: Semaphore,
    hold_until: Mutex<Option<Instant>>,
}

impl EmbedGate {
    fn new() -> Self {
        Self {
            permits: Semaphore::new(MAX_IN_FLIGHT),
            hold_until: Mutex::new(None),
        }
    }

    /// Time left on the hold-off, if one is running.
    fn held_for(&self) -> Option<Duration> {
        let until = (*self.hold_until.lock().unwrap_or_else(|e| e.into_inner()))?;
        until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())
    }

    fn hold(&self, d: Duration) {
        let until = Instant::now() + d;
        let mut slot = self.hold_until.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none_or(|t| t < until) {
            *slot = Some(until);
        }
    }

    fn held_error(left: Duration) -> ProviderError {
        ProviderError::RateLimit {
            retry_after_secs: Some(left.as_secs_f64().ceil() as u64),
        }
    }

    /// Send the request `build` makes, retrying what can succeed on a retry.
    async fn send(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ProviderError> {
        if let Some(left) = self.held_for() {
            return Err(Self::held_error(left));
        }
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| ProviderError::Request("embedding gate closed".into()))?;
        for attempt in 0..MAX_ATTEMPTS {
            // Another caller may have been told to wait while this one queued.
            if let Some(left) = self.held_for() {
                return Err(Self::held_error(left));
            }
            let last = attempt + 1 == MAX_ATTEMPTS;
            let resp = match build().send().await {
                Ok(r) => r,
                Err(e) => {
                    if last {
                        return Err(ProviderError::Request(format!("embedding request: {e}")));
                    }
                    debug!(attempt, error = %e, "embedding request failed, retrying");
                    tokio::time::sleep(BACKOFF[attempt]).await;
                    continue;
                }
            };
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            if status == 401 || status == 403 {
                return Err(ProviderError::Auth(format!("embedding auth error: {status}")));
            }
            let retry_after = crate::http::retry_after_secs(resp.headers()).map(Duration::from_secs);
            let retryable = status == 429 || status == 408 || status.is_server_error();
            if !retryable {
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    code: status.as_u16().to_string(),
                    message: format!("embedding error: {status}: {}", body.chars().take(300).collect::<String>()),
                    retryable: false,
                });
            }
            let wait = retry_after.unwrap_or(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
            if last || wait > MAX_INLINE_WAIT {
                let hold = retry_after.unwrap_or(DEFAULT_HOLD);
                self.hold(hold);
                warn!(status = %status, hold_secs = hold.as_secs(), "embedding provider refused; holding off");
                return Err(if status == 429 {
                    ProviderError::RateLimit {
                        retry_after_secs: Some(hold.as_secs()),
                    }
                } else {
                    ProviderError::Api {
                        code: status.as_u16().to_string(),
                        message: format!("embedding error: {status}"),
                        retryable: true,
                    }
                });
            }
            debug!(attempt, status = %status, wait_ms = wait.as_millis() as u64, "embedding retry");
            tokio::time::sleep(wait).await;
        }
        unreachable!("the last attempt always returns")
    }
}

/// OpenAI-compatible embedding provider (text-embedding-3-small).
pub struct OpenAIEmbeddingProvider {
    api_key: ApiKey,
    model: String,
    base_url: String,
    dims: usize,
    http_client: reqwest::Client,
    extra_headers: Vec<(String, String)>,
    gate: EmbedGate,
}

impl OpenAIEmbeddingProvider {
    pub fn with_base_url(api_key: impl Into<ApiKey>, base_url: String) -> Self {
        Self {
            api_key: api_key.into(),
            model: "text-embedding-3-small".to_string(),
            base_url,
            dims: 1536,
            http_client: crate::http::request_client(),
            extra_headers: Vec::new(),
            gate: EmbedGate::new(),
        }
    }

    pub fn with_model(mut self, model: String, dims: usize) -> Self {
        self.model = model;
        self.dims = dims;
        self
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a [String],
    model: &'a str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    #[serde(default)]
    index: usize,
    embedding: Vec<f32>,
}

/// One vector per input, or an error naming the mismatch.
fn check_count(got: usize, want: usize) -> Result<(), ProviderError> {
    if got == want {
        Ok(())
    } else {
        Err(ProviderError::Request(format!(
            "embedding response has {got} vectors for {want} inputs"
        )))
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbeddingProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let url = format!("{}/embeddings", self.base_url);
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(MAX_INPUTS_PER_REQUEST) {
            let body = EmbeddingRequest {
                input: batch,
                model: &self.model,
            };
            let resp = self
                .gate
                .send(|| {
                    let mut req = self
                        .http_client
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", self.api_key.current()))
                        .headers(crate::RequestTrace::new("embedding").headers());
                    for (key, value) in &self.extra_headers {
                        req = req.header(key, value);
                    }
                    req.json(&body)
                })
                .await?;
            let mut data: EmbeddingResponse = resp
                .json()
                .await
                .map_err(|e| ProviderError::Request(format!("parse error: {}", e)))?;
            check_count(data.data.len(), batch.len())?;
            data.data.sort_by_key(|d| d.index);
            out.extend(data.data.into_iter().map(|d| d.embedding));
        }
        Ok(out)
    }
}

/// Ollama embedding provider.
pub struct OllamaEmbeddingProvider {
    base_url: String,
    model: String,
    dims: usize,
    http_client: reqwest::Client,
    gate: EmbedGate,
}

impl OllamaEmbeddingProvider {
    pub fn new(base_url: String, model: String, dims: usize) -> Self {
        Self {
            base_url,
            model,
            dims,
            http_client: crate::http::request_client(),
            gate: EmbedGate::new(),
        }
    }
}

#[derive(Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let url = format!("{}/api/embed", self.base_url);
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(MAX_INPUTS_PER_REQUEST) {
            let body = OllamaEmbedRequest {
                model: &self.model,
                input: batch,
            };
            let resp = self
                .gate
                .send(|| self.http_client.post(&url).json(&body))
                .await?;
            let data: OllamaEmbedResponse = resp
                .json()
                .await
                .map_err(|e| ProviderError::Request(format!("parse error: {}", e)))?;
            check_count(data.embeddings.len(), batch.len())?;
            out.extend(data.embeddings);
        }
        Ok(out)
    }
}

/// Cached wrapper around any embedding provider.
/// Uses SHA256 content hashing → embedding_cache table to avoid re-embedding.
pub struct CachedEmbeddingProvider {
    inner: Box<dyn EmbeddingProvider>,
    store: std::sync::Arc<db::Store>,
}

impl CachedEmbeddingProvider {
    pub fn new(inner: Box<dyn EmbeddingProvider>, store: std::sync::Arc<db::Store>) -> Self {
        Self { inner, store }
    }

    fn content_hash(text: &str) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(text.as_bytes());
        hex::encode(hash)
    }
}

#[async_trait]
impl EmbeddingProvider for CachedEmbeddingProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let model = self.inner.id().to_string();
        let mut results = vec![Vec::new(); texts.len()];
        let mut uncached_indices = Vec::new();
        let mut uncached_texts = Vec::new();

        // Check cache for each text
        for (i, text) in texts.iter().enumerate() {
            let hash = Self::content_hash(text);
            if let Ok(Some(cached)) = self.store.get_cached_embedding(&hash, &model) {
                results[i] = bytes_to_f32(&cached);
                debug!(hash = %hash, "embedding cache hit");
            } else {
                uncached_indices.push(i);
                uncached_texts.push(text.clone());
            }
        }

        // Embed uncached texts
        if !uncached_texts.is_empty() {
            let embeddings = self.inner.embed(&uncached_texts).await?;
            let dims = self.inner.dimensions() as i64;

            for (j, embedding) in embeddings.into_iter().enumerate() {
                let idx = uncached_indices[j];
                let hash = Self::content_hash(&texts[idx]);

                // Store in cache
                let blob = f32_to_bytes(&embedding);
                let _ = self
                    .store
                    .insert_cached_embedding(&hash, &blob, &model, dims);

                results[idx] = embedding;
            }
        }

        Ok(results)
    }
}

/// Convert f32 slice to bytes (little-endian).
pub fn f32_to_bytes(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert bytes to f32 vec (little-endian).
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_f32_roundtrip() {
        let original = vec![1.0f32, 2.5, -3.14, 0.0];
        let bytes = f32_to_bytes(&original);
        let recovered = bytes_to_f32(&bytes);
        assert_eq!(original.len(), recovered.len());
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_content_hash() {
        let h1 = CachedEmbeddingProvider::content_hash("hello");
        let h2 = CachedEmbeddingProvider::content_hash("hello");
        let h3 = CachedEmbeddingProvider::content_hash("world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64); // SHA256 hex
    }

    /// One scripted answer: a status, an optional `Retry-After`, and for a
    /// 200 one vector per input (sent in REVERSE order, each tagged with its
    /// index, as the API may).
    #[derive(Clone, Copy)]
    struct Reply {
        status: u16,
        retry_after: Option<u64>,
    }

    const OK: Reply = Reply { status: 200, retry_after: None };

    /// An embeddings stub answering requests with `script` in turn (the last
    /// entry repeats), keeping the input count of every request it read.
    async fn stub(script: Vec<Reply>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<usize>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 65536];
                let body = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break text[end + 4..].to_string();
                        }
                    }
                };
                let inputs = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["input"].as_array().map(|a| a.len()))
                    .unwrap_or(0);
                let n = {
                    let mut l = log.lock().unwrap();
                    l.push(inputs);
                    l.len()
                };
                let reply = script[(n - 1).min(script.len() - 1)];
                let payload = if reply.status == 200 {
                    let data: Vec<serde_json::Value> = (0..inputs)
                        .rev()
                        .map(|i| serde_json::json!({"index": i, "embedding": [i as f32, 1.0]}))
                        .collect();
                    serde_json::json!({ "data": data }).to_string()
                } else {
                    r#"{"error":{"message":"refused"}}"#.to_string()
                };
                let extra = reply.retry_after.map(|s| format!("retry-after: {s}\r\n")).unwrap_or_default();
                let out = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    reply.status,
                    payload.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("memory {i}")).collect()
    }

    #[tokio::test]
    async fn texts_go_in_batched_requests_and_come_back_in_input_order() {
        let (url, seen) = stub(vec![OK]).await;
        let p = OpenAIEmbeddingProvider::with_base_url("k".to_string(), url);
        let out = p.embed(&texts(150)).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![128, 22], "150 texts = two requests, not 150");
        assert_eq!(out.len(), 150);
        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[127][0], 127.0, "sorted by index, not response order");
        assert_eq!(out[128][0], 0.0, "the second request's first vector");
    }

    #[tokio::test]
    async fn a_short_retry_after_is_waited_out_then_the_request_succeeds() {
        let (url, seen) = stub(vec![Reply { status: 429, retry_after: Some(1) }, OK]).await;
        let p = OpenAIEmbeddingProvider::with_base_url("k".to_string(), url);
        let started = Instant::now();
        let out = p.embed(&texts(3)).await.unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(started.elapsed() >= Duration::from_secs(1), "waited the server's Retry-After");
    }

    #[tokio::test]
    async fn a_long_retry_after_fails_now_and_holds_every_caller_off() {
        let (url, seen) = stub(vec![Reply { status: 503, retry_after: Some(300) }]).await;
        let p = OpenAIEmbeddingProvider::with_base_url("k".to_string(), url);
        let started = Instant::now();
        let err = p.embed(&texts(3)).await.unwrap_err();
        assert!(err.is_retryable(), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "did not sit out five minutes");
        let again = p.embed(&texts(3)).await.unwrap_err();
        assert!(matches!(again, ProviderError::RateLimit { retry_after_secs: Some(s) } if s > 290), "{again:?}");
        assert_eq!(seen.lock().unwrap().len(), 1, "the held call never reached the server");
    }

    #[tokio::test]
    async fn refusals_without_a_wait_back_off_then_hold() {
        let (url, seen) = stub(vec![Reply { status: 429, retry_after: None }]).await;
        let p = OpenAIEmbeddingProvider::with_base_url("k".to_string(), url);
        let err = p.embed(&texts(1)).await.unwrap_err();
        assert!(matches!(err, ProviderError::RateLimit { .. }), "{err:?}");
        assert_eq!(seen.lock().unwrap().len(), MAX_ATTEMPTS);
        assert!(p.embed(&texts(1)).await.is_err());
        assert_eq!(seen.lock().unwrap().len(), MAX_ATTEMPTS, "held off after the retries ran out");
    }

    #[tokio::test]
    async fn a_rejected_request_is_not_retried() {
        let (url, seen) = stub(vec![Reply { status: 400, retry_after: None }]).await;
        let p = OpenAIEmbeddingProvider::with_base_url("k".to_string(), url);
        let err = p.embed(&texts(1)).await.unwrap_err();
        assert!(!err.is_retryable(), "{err:?}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
