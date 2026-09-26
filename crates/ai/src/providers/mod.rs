pub mod anthropic;
pub mod cli;
pub mod gemini;
pub mod linked;
pub mod local;
pub mod local_host;
#[cfg(feature = "local-inference")]
mod local_ffi;
pub mod ollama;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use cli::CLIProvider;
pub use gemini::GeminiProvider;
pub use linked::LinkedProvider;
pub use local::LocalProvider;
pub use local_host::LocalHost;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
