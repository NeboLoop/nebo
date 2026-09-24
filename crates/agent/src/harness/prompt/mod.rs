//! The system prompt: a fixed part in our words, the employee, the cache
//! boundary, then the per-session part. Built once per turn and byte-stable
//! across a session. Date and environment live in the fixed part's
//! environment section; there is no per-call state block. WP2.2 builds it.

pub mod sections;

/// A turn's system prompt, in the order it is sent.
pub struct SystemPrompt {
    pub fixed: String,
    pub employee: String,
    pub boundary: String,
    pub per_session: String,
}
