use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::ProviderError;

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

/// OpenAI-compatible embedding provider (text-embedding-3-small).
pub struct OpenAIEmbeddingProvider {
    api_key: String,
    model: String,
    base_url: String,
    dims: usize,
    http_client: reqwest::Client,
}

impl OpenAIEmbeddingProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: "text-embedding-3-small".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            dims: 1536,
            http_client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            api_key,
            model: "text-embedding-3-small".to_string(),
            base_url,
            dims: 1536,
            http_client: reqwest::Client::new(),
        }
    }

    pub fn with_model(mut self, model: String, dims: usize) -> Self {
        self.model = model;
        self.dims = dims;
        self
    }
}

#[derive(Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
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
        let body = EmbeddingRequest {
            input: texts.to_vec(),
            model: self.model.clone(),
        };

        let delays = [500u64, 2000, 8000];
        for (attempt, delay) in delays.iter().enumerate() {
            let resp = self
                .http_client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    if status == 401 || status == 403 {
                        return Err(ProviderError::Auth(format!(
                            "embedding auth error: {}",
                            status
                        )));
                    }
                    if status.is_success() {
                        let data: EmbeddingResponse = r
                            .json()
                            .await
                            .map_err(|e| ProviderError::Request(format!("parse error: {}", e)))?;
                        return Ok(data.data.into_iter().map(|d| d.embedding).collect());
                    }
                    if attempt < delays.len() - 1 {
                        warn!(attempt, status = %status, "embedding retry");
                        tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;
                        continue;
                    }
                    return Err(ProviderError::Request(format!(
                        "embedding error: {}",
                        status
                    )));
                }
                Err(e) => {
                    if attempt < delays.len() - 1 {
                        warn!(attempt, error = %e, "embedding retry");
                        tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;
                        continue;
                    }
                    return Err(ProviderError::Request(format!("embedding request: {}", e)));
                }
            }
        }
        Err(ProviderError::Request("embedding: max retries".to_string()))
    }
}

/// Ollama embedding provider.
pub struct OllamaEmbeddingProvider {
    base_url: String,
    model: String,
    dims: usize,
    http_client: reqwest::Client,
}

impl OllamaEmbeddingProvider {
    pub fn new(base_url: String, model: String, dims: usize) -> Self {
        Self {
            base_url,
            model,
            dims,
            http_client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct OllamaEmbedRequest {
    model: String,
    input: Vec<String>,
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
        let body = OllamaEmbedRequest {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        let delays = [500u64, 2000, 8000];
        for (attempt, delay) in delays.iter().enumerate() {
            let resp = self
                .http_client
                .post(&url)
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    if status.is_success() {
                        let data: OllamaEmbedResponse = r
                            .json()
                            .await
                            .map_err(|e| ProviderError::Request(format!("parse error: {}", e)))?;
                        return Ok(data.embeddings);
                    }
                    if attempt < delays.len() - 1 {
                        warn!(attempt, status = %status, "ollama embedding retry");
                        tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;
                        continue;
                    }
                    return Err(ProviderError::Request(format!(
                        "ollama embedding error: {}",
                        status
                    )));
                }
                Err(e) => {
                    if attempt < delays.len() - 1 {
                        warn!(attempt, error = %e, "ollama embedding retry");
                        tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;
                        continue;
                    }
                    return Err(ProviderError::Request(format!(
                        "ollama embedding request: {}",
                        e
                    )));
                }
            }
        }
        Err(ProviderError::Request(
            "ollama embedding: max retries".to_string(),
        ))
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
                let _ = self.store.insert_cached_embedding(&hash, &blob, &model, dims);

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
}
