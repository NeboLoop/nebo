use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::advisor::{Advisor, from_db, parse_advisor_md};

/// Manages loading, caching, and hot-reloading of ADVISOR.md files.
/// DB advisors override file-based advisors with the same name.
pub struct Loader {
    /// Advisors directory (e.g. <data_dir>/advisors/).
    advisors_dir: PathBuf,
    /// Database store for DB-defined advisors.
    store: Arc<db::Store>,
    /// Loaded advisors keyed by name.
    advisors: Arc<RwLock<HashMap<String, Advisor>>>,
}

impl Loader {
    pub fn new(advisors_dir: PathBuf, store: Arc<db::Store>) -> Self {
        Self {
            advisors_dir,
            store,
            advisors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all advisors from filesystem and database.
    /// DB advisors override file-based advisors with the same name.
    pub async fn load_all(&self) -> usize {
        let loaded = scan(&self.advisors_dir, &self.store);
        let count = loaded.len();
        *self.advisors.write().await = loaded;
        info!(count, dir = %self.advisors_dir.display(), "loaded advisors");
        count
    }

    /// Get an advisor by name.
    pub async fn get(&self, name: &str) -> Option<Advisor> {
        self.advisors.read().await.get(name).cloned()
    }

    /// List all enabled advisors, sorted by priority (highest first).
    pub async fn list_enabled(&self) -> Vec<Advisor> {
        let advisors = self.advisors.read().await;
        let mut enabled: Vec<Advisor> = advisors.values().filter(|a| a.enabled).cloned().collect();
        enabled.sort_by(|a, b| b.priority.cmp(&a.priority));
        enabled
    }

    /// List all advisors (enabled and disabled).
    pub async fn list_all(&self) -> Vec<Advisor> {
        let advisors = self.advisors.read().await;
        let mut all: Vec<Advisor> = advisors.values().cloned().collect();
        all.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        all
    }

    /// Start watching for filesystem changes and reload on modification.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let advisors_dir = self.advisors_dir.clone();
        let store = self.store.clone();
        let advisors = self.advisors.clone();

        tokio::spawn(async move {
            use notify::{Event, EventKind, RecursiveMode, Watcher};
            use tokio::sync::mpsc;

            let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<Event>>();

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.send(res);
                },
                notify::Config::default().with_poll_interval(std::time::Duration::from_secs(2)),
            ) {
                Ok(w) => w,
                Err(e) => {
                    warn!(error = %e, "failed to create filesystem watcher for advisors");
                    return;
                }
            };

            if advisors_dir.exists() {
                if let Err(e) = watcher.watch(&advisors_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %advisors_dir.display(), "failed to watch advisors dir");
                }
            }

            let relevant = |event: &Event| {
                matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_))
                    && event.paths.iter().any(|p| {
                        p.file_name().and_then(|n| n.to_str()).unwrap_or("").eq_ignore_ascii_case("advisor.md")
                    })
            };
            let debounce = std::time::Duration::from_secs(1);

            // Setting up the watch is not instant (seconds, on a loaded
            // machine), and nothing written to the folder before it is in
            // place is ever reported: an advisor written then was never
            // loaded. The boot's load ran before the watch began, so the
            // first pass looks once without waiting for an event, the way an
            // event would, and finds whatever landed meanwhile.
            let mut look_now = true;
            loop {
                if !std::mem::take(&mut look_now) {
                    match rx.recv().await {
                        None => return,
                        Some(Ok(event)) if relevant(&event) => {}
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => {
                            warn!(error = %e, "filesystem watch error");
                            continue;
                        }
                    }
                }

                // Coalesce a burst into one reload once it settles, and never
                // drop a change: the old debounce discarded every event within
                // a second of the last reload, so an edit made then stayed on
                // disk and out of memory until some later write. A write
                // during the wait starts the wait again (for at most 30 s of
                // steady writes); the reload reads the folder as the last
                // write left it.
                for _ in 0..30 {
                    tokio::time::sleep(debounce).await;
                    let mut more = false;
                    while let Ok(next) = rx.try_recv() {
                        if next.as_ref().is_ok_and(|e| relevant(e)) {
                            more = true;
                        }
                    }
                    if !more {
                        break;
                    }
                }

                debug!("advisors directory changed, reloading");
                let loaded = scan(&advisors_dir, &store);
                let count = loaded.len();
                *advisors.write().await = loaded;
                info!(count, "reloaded advisors after filesystem change");
            }
        })
    }
}

/// Every advisor: the ADVISOR.md files in `advisors_dir`, then the
/// database's, which override a file of the same name. What the boot's
/// load and every watcher reload read.
fn scan(advisors_dir: &Path, store: &db::Store) -> HashMap<String, Advisor> {
    let mut loaded = HashMap::new();
    if advisors_dir.exists() {
        for advisor in load_advisors_from_dir(advisors_dir) {
            loaded.insert(advisor.name.clone(), advisor);
        }
    }
    if let Ok(db_advisors) = store.list_advisors() {
        for db_advisor in &db_advisors {
            let advisor = from_db(db_advisor);
            loaded.insert(advisor.name.clone(), advisor);
        }
    }
    loaded
}

/// Load ADVISOR.md files from a directory.
/// Each subdirectory should contain an ADVISOR.md file.
fn load_advisors_from_dir(dir: &Path) -> Vec<Advisor> {
    let mut advisors = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read advisors directory");
            return advisors;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            if let Some(md_path) = find_advisor_md(&path) {
                match std::fs::read(&md_path) {
                    Ok(data) => match parse_advisor_md(&data) {
                        Ok(mut advisor) => {
                            advisor.source_path = Some(md_path);
                            advisors.push(advisor);
                        }
                        Err(e) => {
                            warn!(path = %md_path.display(), error = %e, "failed to parse ADVISOR.md");
                        }
                    },
                    Err(e) => {
                        warn!(path = %md_path.display(), error = %e, "failed to read ADVISOR.md");
                    }
                }
            }
        }
    }

    advisors
}

/// Find an ADVISOR.md file in a directory (case-insensitive).
fn find_advisor_md(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("advisor.md") {
            return Some(entry.path());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_advisor_md(dir: &Path, name: &str, content: &str) {
        let advisor_dir = dir.join(name);
        std::fs::create_dir_all(&advisor_dir).unwrap();
        std::fs::write(advisor_dir.join("ADVISOR.md"), content).unwrap();
    }

    const BASIC_ADVISOR: &str = r#"---
name: skeptic
role: critic
description: Challenges assumptions
priority: 10
enabled: true
timeout_seconds: 30
---

You are the Skeptic. Challenge all ideas.
"#;

    const DISABLED_ADVISOR: &str = r#"---
name: historian
role: historian
description: Historical perspective
priority: 5
enabled: false
---

You provide historical context and precedents.
"#;

    #[tokio::test]
    async fn test_load_advisors_from_dir() {
        let tmp = TempDir::new().unwrap();
        create_advisor_md(tmp.path(), "skeptic", BASIC_ADVISOR);
        create_advisor_md(tmp.path(), "historian", DISABLED_ADVISOR);

        let advisors = load_advisors_from_dir(tmp.path());
        assert_eq!(advisors.len(), 2);
    }

    #[tokio::test]
    async fn test_list_enabled() {
        let tmp = TempDir::new().unwrap();
        create_advisor_md(tmp.path(), "skeptic", BASIC_ADVISOR);
        create_advisor_md(tmp.path(), "historian", DISABLED_ADVISOR);

        // Use an in-memory DB for testing (seeds 5 default advisors from migrations)
        let store = Arc::new(db::Store::new(":memory:").unwrap());
        let loader = Loader::new(tmp.path().to_path_buf(), store);
        loader.load_all().await;

        let enabled = loader.list_enabled().await;
        // DB seeds 5 default enabled advisors; file-based "skeptic" overrides the DB one,
        // "historian" from file is disabled. Net: 5 enabled (4 DB-only + file "skeptic")
        assert!(enabled.len() >= 4);
        // The file-based skeptic should be present and enabled
        assert!(enabled.iter().any(|a| a.name == "skeptic"));
        // The file-based historian should NOT be in enabled list (it's disabled)
        assert!(!enabled.iter().any(|a| a.name == "historian" && !a.enabled));
    }

    fn named(name: &str, description: &str) -> String {
        format!("---\nname: {name}\nrole: critic\ndescription: {description}\npriority: 1\nenabled: true\n---\n\nBody.\n")
    }

    /// Poll until the loader's `name` advisor reads `description`.
    async fn until_described(loader: &Loader, name: &str, description: &str, secs: u64) -> bool {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
        while tokio::time::Instant::now() < deadline {
            if loader.get(name).await.is_some_and(|a| a.description == description) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    /// An advisor written after the boot's load and before the folder watch
    /// is in place is loaded anyway: the watch looks once when it starts.
    /// Nothing is written after `watch()`, so no event ever names it.
    #[tokio::test]
    async fn an_advisor_written_before_the_watch_is_in_place_is_found() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(db::Store::new(":memory:").unwrap());
        let loader = Loader::new(tmp.path().to_path_buf(), store);
        loader.load_all().await;
        assert!(loader.get("watchman").await.is_none());

        create_advisor_md(tmp.path(), "watchman", &named("watchman", "Written meanwhile"));
        let handle = loader.watch();
        let found = until_described(&loader, "watchman", "Written meanwhile", 20).await;
        handle.abort();
        assert!(found, "the watch never found the advisor written before it was in place");
    }

    /// An edit made right after a reload is applied, not dropped: the last
    /// change always lands. Before, every event within a second of the last
    /// reload was discarded and the edit stayed out until some later write.
    #[tokio::test]
    async fn an_edit_right_after_a_reload_is_applied() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(db::Store::new(":memory:").unwrap());
        create_advisor_md(tmp.path(), "watchman", &named("watchman", "First"));
        let loader = Loader::new(tmp.path().to_path_buf(), store);
        loader.load_all().await;
        let handle = loader.watch();

        // A reload the watch makes: re-write until it is seen (the watch
        // gives no "armed" signal).
        let path = tmp.path().join("watchman").join("ADVISOR.md");
        let mut reloaded = false;
        for _ in 0..80 {
            std::fs::write(&path, named("watchman", "Second")).unwrap();
            if until_described(&loader, "watchman", "Second", 1).await {
                reloaded = true;
                break;
            }
        }
        assert!(reloaded, "the watch reloads an edit");

        // At once, inside the old one-second window, and never again.
        std::fs::write(&path, named("watchman", "Third")).unwrap();
        let applied = until_described(&loader, "watchman", "Third", 15).await;
        handle.abort();
        assert!(applied, "the edit made right after a reload was dropped");
    }
}
