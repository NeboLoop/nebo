pub mod decide;
pub mod embedding;
pub mod http;
pub mod image_norm;
pub mod local_models;
pub mod providers;
pub mod sse;
pub mod transcribe;
pub mod types;

pub use decide::{Answer, Bearer, DecideClient, Decision, DecideEndpoint, Question, JEV_MODEL};
pub use embedding::{
    CachedEmbeddingProvider, EmbeddingProvider, OllamaEmbeddingProvider, OpenAIEmbeddingProvider,
    bytes_to_f32, f32_to_bytes,
};
pub use providers::{
    AnthropicProvider, CLIProvider, Connection, GeminiProvider, LinkedProvider, LocalHost, LocalProvider,
    OllamaProvider, OpenAIProvider, Relay, connection_id,
};
pub use types::*;

/// How the verify loop's word to a builder begins: the review of its code
/// work. A linked coding agent is sent it (it is for the agent), when it is
/// what came after the agent's last answer.
pub const REVIEW_NOTE: &str = "Review of your code work:";
