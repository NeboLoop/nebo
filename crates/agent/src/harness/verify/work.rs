//! The code work of a turn: which projects it changes, where, and from what.
//!
//! The builder never works in the owner's own copy of a project. The first
//! call of a turn on a code surface that would change a project folder
//! ([`project::project_root`]) moves the work into an isolated copy of it
//! (`worktree::create`: a git worktree, or a scratch copy of a folder that
//! is not a repository), and from then on every call of the turn that names
//! a path in the project is pointed at the copy instead ([`TurnWork::route`]).
//! The copy lands on the project only once the work is verified, or when the
//! owner chooses to land it anyway (`verify::engine`).
//!
//! Three kinds of work are not copied: an app's own pages (its app history
//! is the undo, so they are checked where they are), a helper already
//! working in its own copy, and a linked coding agent's own folder (it runs
//! its own tools). Each is still verified.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::project;
use super::shadow::{Shadow, Snapshot};
use crate::worktree;

/// What kind of work a project's change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// Moved into an isolated copy by this loop, which lands it.
    Isolated,
    /// An app's own pages, checked where they are.
    AppPages,
    /// A helper's own copy; the helper's end lands it.
    HelperCopy,
    /// A linked coding agent's own folder.
    Folder,
}

impl WorkKind {
    /// The `isolation` column of its verification row.
    pub fn isolation(self, iso: Option<&worktree::Isolation>) -> &'static str {
        match iso {
            Some(worktree::Isolation::Git(_)) => "worktree",
            Some(worktree::Isolation::Copy(_)) => "copy",
            None => "none",
        }
    }
}

/// One project a turn's code work changes.
#[derive(Debug, Clone)]
pub struct Work {
    /// The verification's id.
    pub id: String,
    pub kind: WorkKind,
    /// The owner's project folder.
    pub project_root: PathBuf,
    /// Where the work happens.
    pub work_dir: PathBuf,
    /// The isolated copy this loop made (`Isolated` only).
    pub iso: Option<worktree::Isolation>,
    pub shadow: Shadow,
    /// The project as it was before the work.
    pub base: Snapshot,
    /// The project as the last review round saw it.
    pub last: Option<Snapshot>,
    /// The checks' and reviewers' build folder.
    pub build_dir: PathBuf,
}

impl Work {
    /// Where a verification keeps its own files (its shadow, build folder,
    /// saved diffs): `<data_dir>/verify/<id>`.
    pub fn home(id: &str) -> PathBuf {
        config::data_dir().unwrap_or_else(|_| std::env::temp_dir()).join("verify").join(id)
    }

    /// Start work on `project_root` in `work_dir`, snapshotting it first.
    pub async fn start(id: &str, kind: WorkKind, project_root: &Path, work_dir: &Path, iso: Option<worktree::Isolation>) -> Result<Self, String> {
        let home = Self::home(id);
        let shadow = Shadow::open(&home.join("shadow.git"), work_dir).await?;
        let base = shadow.snapshot().await?;
        Ok(Self {
            id: id.to_string(),
            kind,
            project_root: project_root.to_path_buf(),
            work_dir: work_dir.to_path_buf(),
            iso,
            shadow,
            base,
            last: None,
            build_dir: home.join("build"),
        })
    }

    /// Remove the verification's own files. The copy itself is the
    /// landing's to remove.
    pub fn clean_up(&self) {
        let home = Self::home(&self.id);
        if let Err(e) = std::fs::remove_dir_all(&home)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %home.display(), error = %e, "verification files not removed");
        }
    }
}

/// A new verification id: a valid worktree task id.
pub fn new_id() -> String {
    format!("v-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// The code work of one turn, shared by its tool calls (which route paths
/// into the copies) and its end check (which verifies them).
#[derive(Clone, Default)]
pub struct TurnWork {
    works: Arc<tokio::sync::Mutex<Vec<Work>>>,
}

/// What routing a call did.
#[derive(Debug, Default)]
pub struct Routed {
    /// The call's paths in a project now point into its copy.
    pub rewrote: bool,
    /// Work started by this call: the note the builder reads beside the
    /// call's result.
    pub started: Option<String>,
}

/// Who a newly started piece of work is for, and what it is told about.
pub struct Starting<'a> {
    pub store: &'a db::Store,
    pub agent_id: &'a str,
    pub session_key: &'a str,
    pub chat_id: &'a str,
    pub spec: &'a str,
    /// The owner's standing rules for the employee.
    pub owner_rules: &'a str,
}

impl TurnWork {
    pub fn new(works: Vec<Work>) -> Self {
        Self { works: Arc::new(tokio::sync::Mutex::new(works)) }
    }

    pub async fn snapshot(&self) -> Vec<Work> {
        self.works.lock().await.clone()
    }

    /// Replace one work's state (its last round's snapshot, its copy after
    /// catching up).
    pub async fn put(&self, work: Work) {
        let mut works = self.works.lock().await;
        if let Some(w) = works.iter_mut().find(|w| w.id == work.id) {
            *w = work;
        } else {
            works.push(work);
        }
    }

    /// Drop a finished work.
    pub async fn remove(&self, id: &str) {
        self.works.lock().await.retain(|w| w.id != id);
    }

    pub async fn add(&self, work: Work) {
        self.works.lock().await.push(work);
    }

    pub async fn is_empty(&self) -> bool {
        self.works.lock().await.is_empty()
    }

    /// Point a call at the copies: every path in its input inside a project
    /// this turn works on in a copy is rewritten to the copy, and so is the
    /// call's working directory. A call that would change a project not yet
    /// in a copy (`changes`, on a code surface) first starts the work: the
    /// copy is made and the verification recorded.
    pub async fn route(
        &self,
        input: &mut serde_json::Value,
        cwd: &mut Option<String>,
        changes: bool,
        code_surface: bool,
        who: &Starting<'_>,
    ) -> Result<Routed, String> {
        let mut works = self.works.lock().await;
        let mut routed = Routed::default();
        if changes && code_surface {
            for path in candidate_paths(input, cwd.as_deref()) {
                if works.iter().any(|w| project::inside(&path, &w.project_root) || project::inside(&path, &w.work_dir)) {
                    continue;
                }
                if let Some(app_dir) = app_pages_of(who.store, who.agent_id, &path) {
                    let work = start_work(who, WorkKind::AppPages, &app_dir, &app_dir, None).await?;
                    routed.started = Some(started_note(&work, who));
                    works.push(work);
                    continue;
                }
                let Some(root) = project::project_root(&path, &[]) else { continue };
                if inside_any_copy(&root) {
                    continue;
                }
                let id = new_id();
                let iso = worktree::create(&root, &id).await.map_err(|e| format!("Could not make a working copy of {}: {e}", root.display()))?;
                let copy = iso.path().to_path_buf();
                let work = match start_work_with_id(who, &id, WorkKind::Isolated, &root, &copy, Some(iso.clone())).await {
                    Ok(w) => w,
                    Err(e) => {
                        let mut isos = [iso];
                        worktree::merge_all(&mut isos, "", None).await;
                        return Err(e);
                    }
                };
                routed.started = Some(started_note(&work, who));
                works.push(work);
            }
        }
        for work in works.iter().filter(|w| w.kind == WorkKind::Isolated) {
            let (from, to) = (work.project_root.to_string_lossy().into_owned(), work.work_dir.to_string_lossy().into_owned());
            if rewrite(input, &from, &to) {
                routed.rewrote = true;
            }
            if let Some(c) = cwd.as_mut()
                && let Some(mapped) = mapped(c, &from, &to)
            {
                *c = mapped;
                routed.rewrote = true;
            }
        }
        Ok(routed)
    }
}

async fn start_work(who: &Starting<'_>, kind: WorkKind, root: &Path, work_dir: &Path, iso: Option<worktree::Isolation>) -> Result<Work, String> {
    start_work_with_id(who, &new_id(), kind, root, work_dir, iso).await
}

/// Snapshot the work's folder and record its verification.
pub async fn start_work_with_id(who: &Starting<'_>, id: &str, kind: WorkKind, root: &Path, work_dir: &Path, iso: Option<worktree::Isolation>) -> Result<Work, String> {
    let work = Work::start(id, kind, root, work_dir, iso.clone()).await?;
    let branch = match &iso {
        Some(worktree::Isolation::Git(wt)) => wt.branch.clone(),
        _ => String::new(),
    };
    who.store
        .create_verification(
            &db::NewVerification {
                id,
                agent_id: who.agent_id,
                session_key: who.session_key,
                chat_id: who.chat_id,
                spec: who.spec,
                project_root: &root.to_string_lossy(),
                work_dir: &work_dir.to_string_lossy(),
                isolation: kind.isolation(iso.as_ref()),
                branch: &branch,
                base_ref: &work.base.0,
                ..Default::default()
            },
            chrono::Utc::now().timestamp(),
        )
        .map_err(|e| format!("Could not record the work: {e}"))?;
    Ok(work)
}

/// What the builder is told when its work moves into a copy: where it
/// works now, the rules it works by, and who else is working there.
fn started_note(work: &Work, who: &Starting<'_>) -> String {
    let mut note = match work.kind {
        WorkKind::Isolated => format!(
            "This project ({}) is code, so your changes to it now go to your own working copy at {}: \
             every path in the project points there. They land on the project when the work is verified by \
             independent reviewers and its own checks, or when the owner chooses to land it anyway. Never \
             change the owner's own copy.",
            work.project_root.display(),
            work.work_dir.display()
        ),
        _ => format!(
            "This is code ({}): when you finish, independent reviewers and the project's own checks verify it \
             before it is reported done.",
            work.project_root.display()
        ),
    };
    let others = who
        .store
        .list_verifications(&db::VerificationQuery {
            project_root: Some(&work.project_root.to_string_lossy()),
            live: true,
            ..Default::default()
        })
        .unwrap_or_default();
    for other in others.iter().filter(|o| o.session_key != who.session_key) {
        let name = who
            .store
            .get_agent(&other.agent_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "Another employee".to_string());
        let files = who
            .store
            .verification_rounds(&other.id)
            .ok()
            .and_then(|r| r.last().map(|r| r.diff_stat.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "files not reviewed yet".to_string());
        note.push_str(&format!(
            "\n\n{name} is also working in this project ({files}). Message them before you touch the same \
             files, and stay off theirs."
        ));
    }
    note.push_str("\n\n");
    note.push_str(&super::rules::brief_rules(&work.project_root, who.owner_rules));
    note
}

/// Paths a call names: the path-like fields of its input (relative ones
/// against its working directory), the absolute paths in a command, and the
/// working directory a command runs in.
fn candidate_paths(input: &serde_json::Value, cwd: Option<&str>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let resolve = |p: &str| -> PathBuf {
        let p = shellexpand_home(p);
        let path = PathBuf::from(&p);
        if path.is_relative() && let Some(c) = cwd { Path::new(c).join(path) } else { path }
    };
    for key in ["path", "file_path", "notebook_path", "cwd", "dir", "folder"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()).filter(|p| !p.trim().is_empty()) {
            out.push(resolve(p));
        }
    }
    if let Some(paths) = input.get("paths").and_then(|v| v.as_array()) {
        out.extend(paths.iter().filter_map(|p| p.as_str()).map(resolve));
    }
    if let Some(command) = input.get("command").and_then(|v| v.as_str()) {
        let cmd_cwd = input.get("cwd").and_then(|v| v.as_str()).map(str::to_string).or_else(|| cwd.map(str::to_string));
        if let Some(c) = cmd_cwd {
            out.push(PathBuf::from(shellexpand_home(&c)));
        }
        for word in shlex::split(command).unwrap_or_default() {
            let word = word.trim_matches(|c| c == ';' || c == '&' || c == '|' || c == '(' || c == ')');
            let word = word.split_once('=').map(|(_, v)| v).unwrap_or(word);
            if word.starts_with('/') || word.starts_with("~/") {
                out.push(PathBuf::from(shellexpand_home(word)));
            }
        }
    }
    out
}

fn shellexpand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

/// The app page folder `path` is in, when the employee may work on that app.
fn app_pages_of(store: &db::Store, agent_id: &str, path: &Path) -> Option<PathBuf> {
    let apps = store.list_agents(1_000, 0).ok()?;
    apps.into_iter().filter(|a| a.is_app.unwrap_or(0) != 0).find_map(|app| {
        let dir = PathBuf::from(tools::app_dev::served_dir(&app)?);
        (project::inside(path, &dir) && tools::app_dev::may_work_on(store, agent_id, &app)).then_some(dir)
    })
}

/// Whether `root` is one of Nebo's own copies (a helper's, or this loop's).
fn inside_any_copy(root: &Path) -> bool {
    config::data_dir().is_ok_and(|d| root.starts_with(d.join("worktrees")))
}

/// `value` with `from` (a folder) rewritten to `to` where it is a path or
/// the start of one.
fn mapped(value: &str, from: &str, to: &str) -> Option<String> {
    let rest = value.strip_prefix(from)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("{to}{rest}"))
}

/// Rewrite every occurrence of the folder `from` as a path (followed by the
/// end, a separator, a quote or a space) in every string of `input` to `to`.
fn rewrite(input: &mut serde_json::Value, from: &str, to: &str) -> bool {
    match input {
        serde_json::Value::String(s) => {
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            let mut changed = false;
            while let Some(at) = rest.find(from) {
                let after = &rest[at + from.len()..];
                let boundary = after.is_empty() || after.starts_with(['/', '"', '\'', ' ', '\n', ';', ')']);
                let before_ok = at == 0 || !rest[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
                out.push_str(&rest[..at]);
                if boundary && before_ok {
                    out.push_str(to);
                    changed = true;
                } else {
                    out.push_str(from);
                }
                rest = after;
            }
            out.push_str(rest);
            if changed {
                *s = out;
            }
            changed
        }
        serde_json::Value::Array(items) => items.iter_mut().fold(false, |acc, v| rewrite(v, from, to) || acc),
        serde_json::Value::Object(map) => map.values_mut().fold(false, |acc, v| rewrite(v, from, to) || acc),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_projects_paths_point_into_its_copy_and_nothing_else_moves() {
        let mut input = json!({
            "path": "/work/site/src/app.ts",
            "command": "cd /work/site && pnpm test > /work/site-notes.txt",
            "other": ["/work/site", "/work/sitemap/x"]
        });
        assert!(rewrite(&mut input, "/work/site", "/copies/v-1"));
        assert_eq!(input["path"], "/copies/v-1/src/app.ts");
        assert_eq!(input["command"], "cd /copies/v-1 && pnpm test > /work/site-notes.txt");
        assert_eq!(input["other"], json!(["/copies/v-1", "/work/sitemap/x"]), "a neighbour with the same prefix stays");
        assert_eq!(mapped("/work/site/src", "/work/site", "/c"), Some("/c/src".into()));
        assert_eq!(mapped("/work/sitemap", "/work/site", "/c"), None);
    }

    #[test]
    fn a_calls_paths_are_its_fields_its_commands_paths_and_its_folder() {
        let paths = candidate_paths(
            &json!({"command": "cargo test --manifest-path /repo/Cargo.toml && OUT=/tmp/x make", "description": "run tests"}),
            Some("/repo/crates"),
        );
        assert_eq!(paths, vec![PathBuf::from("/repo/crates"), PathBuf::from("/repo/Cargo.toml"), PathBuf::from("/tmp/x")]);
        let paths = candidate_paths(&json!({"path": "src/lib.rs"}), Some("/repo"));
        assert_eq!(paths, vec![PathBuf::from("/repo/src/lib.rs")]);
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn the_first_change_to_a_project_moves_the_work_into_a_copy_and_records_it() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                // The owner's project, outside Nebo's own folder.
                let projects = tempfile::tempdir().unwrap();
                let project = projects.path().join("site");
                std::fs::create_dir_all(project.join("src")).unwrap();
                std::fs::write(project.join("package.json"), "{}").unwrap();
                std::fs::write(project.join("src/app.ts"), "export const a = 1;\n").unwrap();
                let work = TurnWork::default();
                let who = Starting { store: &store, agent_id: "dev", session_key: "agent:dev:web", chat_id: "c1", spec: "Make a = 2", owner_rules: "" };

                // A read changes nothing: no copy.
                let mut read = json!({"path": project.join("src/app.ts").to_string_lossy()});
                let r = work.route(&mut read, &mut None, false, true, &who).await.unwrap();
                assert!(!r.rewrote && r.started.is_none() && work.is_empty().await);

                // Without code tools, a write is not code work.
                let mut write = json!({"path": project.join("src/app.ts").to_string_lossy(), "content": "export const a = 2;\n"});
                let r = work.route(&mut write.clone(), &mut None, true, false, &who).await.unwrap();
                assert!(r.started.is_none() && work.is_empty().await);

                let r = work.route(&mut write, &mut None, true, true, &who).await.unwrap();
                let note = r.started.expect("the work started");
                assert!(note.contains("your own working copy") && note.contains("Never touch the owner's working copy"), "{note}");
                let works = work.snapshot().await;
                assert_eq!(works.len(), 1);
                let copy = works[0].work_dir.clone();
                assert_eq!(write["path"], copy.join("src/app.ts").to_string_lossy().as_ref(), "the write goes to the copy");
                assert!(copy.join("src/app.ts").exists());
                let row = store.get_verification(&works[0].id).unwrap().unwrap();
                assert_eq!((row.state.as_str(), row.isolation.as_str(), row.spec.as_str()), ("running", "copy", "Make a = 2"));
                assert_eq!(row.project_root, project.to_string_lossy());

                // Later calls on the project go to the copy too; no second copy.
                let mut again = json!({"command": format!("cd {} && npm test", project.display())});
                let r = work.route(&mut again, &mut None, true, true, &who).await.unwrap();
                assert!(r.rewrote && r.started.is_none());
                assert_eq!(again["command"], format!("cd {} && npm test", copy.display()));
                assert_eq!(work.snapshot().await.len(), 1);
                assert_eq!(std::fs::read_to_string(project.join("src/app.ts")).unwrap(), "export const a = 1;\n", "the owner's copy is untouched");
            })
        })
    }
}
