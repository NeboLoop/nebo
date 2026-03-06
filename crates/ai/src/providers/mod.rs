pub mod anthropic;
pub mod cli;
pub mod gemini;
pub mod local;
#[cfg(feature = "local-inference")]
mod local_ffi;
pub mod ollama;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use cli::CLIProvider;
pub use gemini::GeminiProvider;
pub use local::LocalProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
