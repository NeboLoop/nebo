//! Human-readable NeboLoop traffic log for `tail -f`.
//!
//! Writes a single file with timestamped lines showing connects, joins,
//! inbound deliveries, and outbound sends. File is truncated on each new
//! connection so it always reflects the current session.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// DevLog writes human-readable NeboLoop traffic to a file for `tail -f`.
#[derive(Clone)]
pub struct DevLog(Arc<Mutex<BufWriter<File>>>);

impl DevLog {
    /// Create a new devlog, truncating the file. Returns `None` if the path
    /// cannot be created or opened.
    pub fn open(path: &Path) -> Option<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .ok()?;
        Some(Self(Arc::new(Mutex::new(BufWriter::new(file)))))
    }

    /// Log a lifecycle event (CONNECT, AUTH_OK, DISCONNECT, etc.).
    pub fn event(&self, msg: &str) {
        self.write_line(&format!("── {}", msg));
    }

    /// Log an outbound join request.
    pub fn join_request(&self, stream: &str) {
        self.write_line(&format!("→ JOIN stream={}", stream));
    }

    /// Log an inbound join result.
    pub fn join_result(&self, info: &str) {
        self.write_line(&format!("← JOIN_OK {}", info));
    }

    /// Log an inbound message delivery.
    pub fn inbound(&self, stream: &str, from: &str, agent_slug: &str, conv_id: &str, content: &str) {
        let agent_part = if agent_slug.is_empty() {
            String::new()
        } else {
            format!(" agent={}", agent_slug)
        };
        let truncated = truncate_content(content);
        self.write_line(&format!(
            "← IN  stream={} from={}{} conv={}",
            stream, from, agent_part, short_id(conv_id)
        ));
        self.write_line(&format!("            \"{}\"", truncated));
    }

    /// Log an outbound send.
    pub fn outbound(&self, stream: &str, conv_id: &str, content: &str) {
        let truncated = truncate_content(content);
        self.write_line(&format!(
            "→ OUT stream={} conv={}",
            stream, short_id(conv_id)
        ));
        self.write_line(&format!("            \"{}\"", truncated));
    }

    fn write_line(&self, text: &str) {
        let ts = format_timestamp();
        if let Ok(mut w) = self.0.lock() {
            let _ = writeln!(w, "{} {}", ts, text);
            let _ = w.flush();
        }
    }
}

/// Format current time as HH:MM:SS.mmm (local-ish via UNIX_EPOCH offset).
fn format_timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();
    // Use UTC — good enough for a dev log; avoids pulling in chrono.
    let secs_of_day = total_secs % 86400;
    let h = secs_of_day / 3600;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, millis)
}

/// Truncate content to ~200 chars for readability.
fn truncate_content(content: &str) -> String {
    let clean = content.replace('\n', " ");
    if clean.chars().count() <= 200 {
        clean
    } else {
        let truncated: String = clean.chars().take(200).collect();
        format!("{}…", truncated)
    }
}

/// Shorten a UUID to first 8 chars for display.
fn short_id(id: &str) -> &str {
    if id.len() > 8 { &id[..8] } else { id }
}
