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
    CLIProvider, Connection, LinkedProvider, LocalHost, LocalProvider,
    OllamaProvider, OpenAIProvider, Relay, as_received, connection_id,
};
pub use types::*;

/// How the verify loop's word to a builder begins: the review of its code
/// work. A linked coding agent is sent it (it is for the agent), when it is
/// what came after the agent's last answer.
pub const REVIEW_NOTE: &str = "Review of your code work:";

/// How the goal check's word to an agent whose goal isn't met yet begins. A
/// linked coding agent is sent it like the review of its code work: it is
/// for the agent, and the owner's message it already answered is never
/// sent again.
pub const GOAL_NOTE: &str = "The agreed goal isn't met yet:";
