mod anthropic_wire;
pub mod cli;
pub mod connection;
pub mod linked;
pub mod local;
pub mod local_host;
pub mod oal;
#[cfg(feature = "local-inference")]
mod local_ffi;
pub mod ollama;
pub mod openai;

pub use cli::CLIProvider;
pub use connection::{Connection, connection_id};
pub use linked::{LinkedProvider, Relay};
pub use local::LocalProvider;
pub use local_host::LocalHost;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
