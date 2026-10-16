use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// How long `drain_extractions` waits for in-flight tasks before giving up.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Global tracker for in-flight background extraction tasks (memory extraction,
/// LLM summary, indexing, personality synthesis). On shutdown the server calls
/// `drain_extractions()` to await these instead of dropping them silently.
static EXTRACTION_HANDLES: std::sync::OnceLock<Mutex<Vec<JoinHandle<()>>>> =
    std::sync::OnceLock::new();

fn extraction_handles() -> &'static Mutex<Vec<JoinHandle<()>>> {
    EXTRACTION_HANDLES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a background extraction task handle for graceful shutdown tracking.
/// Call this whenever a `tokio::spawn` is used for memory extraction, LLM
/// summary, indexing, or personality synthesis.
pub async fn track_extraction(handle: JoinHandle<()>) {
    let mut handles = extraction_handles().lock().await;
    // Prune already-finished handles to keep the vec bounded.
    handles.retain(|h| !h.is_finished());
    handles.push(handle);
}

/// Await all in-flight extraction tasks with a timeout.
/// Called from the server shutdown path to avoid dropping work silently.
pub async fn drain_extractions() {
    let handles: Vec<JoinHandle<()>> = {
        let mut guard = extraction_handles().lock().await;
        guard.drain(..).filter(|h| !h.is_finished()).collect()
    };

    if handles.is_empty() {
        return;
    }

    info!(
        count = handles.len(),
        timeout_secs = DRAIN_TIMEOUT.as_secs(),
        "draining in-flight memory extractions..."
    );

    let drain_all = async {
        for handle in handles {
            let _ = handle.await;
        }
    };

    match tokio::time::timeout(DRAIN_TIMEOUT, drain_all).await {
        Ok(()) => info!("all in-flight extractions drained"),
        Err(_) => warn!(
            "extraction drain timed out after {}s — some tasks may be lost",
            DRAIN_TIMEOUT.as_secs()
        ),
    }
}
