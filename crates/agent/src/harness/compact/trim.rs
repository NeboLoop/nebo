//! Trimming old tool results to stubs, with each rendering frozen the first
//! time it is chosen so the prompt prefix stays stable. WP2.6 moves the trim
//! here from `pruning.rs`.

/// Frozen renderings, keyed on the tool call id (the shape `pruning.rs`
/// trims with today).
pub type Frozen = std::collections::HashMap<String, String>;
