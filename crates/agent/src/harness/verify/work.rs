//! The code work of a turn: which projects it changes, where, and from what.
//!
//! The builder never works in the owner's own copy of a project. The first
//! call of a turn that would change a project folder (`verify::code_folders`)
//! moves the work into an isolated copy of it (`worktree::create`: a git
//! worktree, or a scratch copy of a folder that is no repository), and from
//! then on every call of the turn that names a path in the project is
//! pointed at the copy instead ([`TurnWork::route`]). The copy lands on the
//! project only once the work is verified, or when the owner chooses to land
//! it anyway (`verify::engine`).
//!
//! Three kinds of work are not copied: an app's own pages (its app history
//! is the undo, so they are checked where they are), a helper already
//! working in its own copy (its parent lands it), and a linked coding agent's
//! own folder (it runs its own tools). Each is still verified.
//!
//! A work is recorded (its verification row) when it starts, with its copy's
//! whole state, so a later turn of the same conversation resumes it exactly
//! where it was ([`TurnWork::load`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::project::{self, Folder};
use super::shadow::{EMPTY_TREE, Shadow, Snapshot};
use crate::worktree;

/// What kind of work a project's change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkKind {
    /// Moved into an isolated copy by this loop, which lands it.
    Isolated,
    /// An app's own pages, checked where they are.
    AppPages,
    /// A helper's own copy; its parent lands it.
    HelperCopy,
    /// A linked coding agent's own folder.
    Folder,
}

impl WorkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkKind::Isolated => "isolated",
            WorkKind::AppPages => "app_pages",
            WorkKind::HelperCopy => "helper_copy",
            WorkKind::Folder => "folder",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "app_pages" => WorkKind::AppPages,
            "helper_copy" => WorkKind::HelperCopy,
            "folder" => WorkKind::Folder,
            _ => WorkKind::Isolated,
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
}

impl Work {
    /// Where a verification keeps its own files (its shadow, the diffs its
    /// reviewers read): `<data_dir>/verify/<id>`.
    pub fn home(id: &str) -> PathBuf {
        config::data_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("verify")
            .join(id)
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

/// The session-key prefix of every reviewer verification `id` of the
/// conversation `session_key` runs (`review::node`, `delegation::helper_key`):
/// its cost in the run ledger is theirs.
pub fn reviewer_prefix(session_key: &str, id: &str) -> String {
    format!("subagent:{session_key}:sa-verify-{id}-")
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
    pub workflow_run_id: Option<&'a str>,
    pub activity_id: Option<&'a str>,
}

/// The code work of one turn, shared by its tool calls (which route paths
/// into the copies) and its end check (which verifies them).
#[derive(Clone, Default)]
pub struct TurnWork {
    works: Arc<tokio::sync::Mutex<Vec<Work>>>,
    /// A new app's pages a call is about to write (`writes_app_pages`): the
    /// work starts once the call has made the app.
    pending_app: Arc<std::sync::Mutex<Option<String>>>,
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

impl TurnWork {
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

    /// Drop a work that turned out to change nothing.
    pub async fn remove(&self, id: &str) {
        self.works.lock().await.retain(|w| w.id != id);
    }

    pub async fn is_empty(&self) -> bool {
        self.works.lock().await.is_empty()
    }

    /// The conversation's code work still in hand, as its rows left it: work
    /// being checked or paused on the owner, and finished work whose copy
    /// still waits (not landed). A turn of a helper working in its own copy
    /// starts its work here too, before its first write.
    pub async fn load(who: &Starting<'_>, cwd: Option<&str>, linked: bool) -> Self {
        let turn = Self::default();
        let rows = who
            .store
            .list_verifications(&db::VerificationQuery {
                session_key: Some(who.session_key),
                ..Default::default()
            })
            .unwrap_or_default();
        for row in rows {
            let open = matches!(row.state.as_str(), "running" | "paused")
                || (row.landed_ref.is_empty()
                    && row.isolation != "none"
                    && Path::new(&row.work_dir).is_dir()
                    && row.state != "stopped");
            if !open
                || turn
                    .works
                    .lock()
                    .await
                    .iter()
                    .any(|w| w.project_root.to_string_lossy() == row.project_root)
            {
                continue;
            }
            if let Some(work) = resume(who.store, &row).await {
                turn.works.lock().await.push(work);
            }
        }
        // A linked coding agent runs its own tools in its own folder: when
        // that folder is a project, it is the agent's work, checked where it
        // is (Nebo can't move another runtime's writes into a copy).
        if linked
            && let Some(Folder::Project(root)) =
                cwd.and_then(|c| project::code_folder(Path::new(c), &[]))
            && !turn
                .works
                .lock()
                .await
                .iter()
                .any(|w| w.project_root == root)
            && let Ok(work) = start_work(who, WorkKind::Folder, &root, &root, None, None).await
        {
            turn.works.lock().await.push(work);
        }
        // A helper working in its own copy: its work is that copy.
        if let Some(copy) = cwd.and_then(|c| match project::code_folder(Path::new(c), &[]) {
            Some(Folder::Copy(copy)) => Some(copy),
            _ => None,
        }) && !turn.works.lock().await.iter().any(|w| w.work_dir == copy)
            && let Ok(work) = start_work(who, WorkKind::HelperCopy, &copy, &copy, None, None).await
        {
            turn.works.lock().await.push(work);
        }
        turn
    }

    /// Point a call at the copies: every path in its input inside a project
    /// this turn works on in a copy is rewritten to the copy, and so is the
    /// call's working directory. A call that changes something (`changes`)
    /// in a code folder not yet worked on first starts the work there: a
    /// copy of a project, or the app's pages where they are.
    pub async fn route(
        &self,
        input: &mut serde_json::Value,
        cwd: &mut Option<String>,
        changes: bool,
        who: &Starting<'_>,
    ) -> Result<Routed, String> {
        let mut works = self.works.lock().await;
        let mut routed = Routed::default();
        let app_pages = app_pages_of(who.store, who.agent_id);
        if changes && super::writes_app_pages(input) {
            match existing_app(who.store, input)
                .and_then(|a| tools::app_dev::served_dir(&a).map(PathBuf::from))
            {
                Some(dir)
                    if app_pages.contains(&dir) && !works.iter().any(|w| w.work_dir == dir) =>
                {
                    let work = start_work(who, WorkKind::AppPages, &dir, &dir, None, None).await?;
                    routed.started = Some(started_note(&work, who));
                    works.push(work);
                }
                Some(_) => {}
                // A new app: its pages exist once the call has made it.
                None => {
                    *self.pending_app.lock().unwrap_or_else(|p| p.into_inner()) =
                        input["name"].as_str().map(str::to_string)
                }
            }
        }
        for folder in super::code_folders(input, cwd.as_deref(), changes, &app_pages) {
            let dir = folder.path().to_path_buf();
            if works
                .iter()
                .any(|w| w.project_root == dir || w.work_dir == dir)
            {
                continue;
            }
            let work = match folder {
                Folder::AppPages(pages) => {
                    start_work(who, WorkKind::AppPages, &pages, &pages, None, None).await?
                }
                // Nebo's own copies are a helper's; its turn started their work.
                Folder::Copy(_) => continue,
                Folder::Project(root) => {
                    let id = new_id();
                    let iso = worktree::create(&root, &id).await.map_err(|e| {
                        format!("Could not make a working copy of {}: {e}", root.display())
                    })?;
                    let copy = iso.path().to_path_buf();
                    match start_work_with_id(
                        who,
                        &id,
                        WorkKind::Isolated,
                        &root,
                        &copy,
                        Some(iso.clone()),
                        None,
                    )
                    .await
                    {
                        Ok(w) => w,
                        Err(e) => {
                            let mut iso = iso;
                            worktree::put_away(&mut iso, "Not started").await;
                            return Err(e);
                        }
                    }
                }
            };
            routed.started = Some(started_note(&work, who));
            works.push(work);
        }
        for work in works.iter().filter(|w| w.kind == WorkKind::Isolated) {
            let (from, to) = (
                work.project_root.to_string_lossy().into_owned(),
                work.work_dir.to_string_lossy().into_owned(),
            );
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

    /// After a call ran: a new app it made starts its pages' work, from
    /// nothing (every page is the change).
    pub async fn after_call(&self, who: &Starting<'_>) -> Option<String> {
        let name = self
            .pending_app
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()?;
        let app = who
            .store
            .list_agents(1_000, 0)
            .ok()?
            .into_iter()
            .find(|a| a.is_app.unwrap_or(0) != 0 && a.name == name)?;
        let dir = PathBuf::from(tools::app_dev::served_dir(&app)?);
        let mut works = self.works.lock().await;
        if works.iter().any(|w| w.work_dir == dir) {
            return None;
        }
        let work = start_work(
            who,
            WorkKind::AppPages,
            &dir,
            &dir,
            None,
            Some(Snapshot(EMPTY_TREE.into())),
        )
        .await
        .ok()?;
        let note = started_note(&work, who);
        works.push(work);
        Some(note)
    }
}

async fn start_work(
    who: &Starting<'_>,
    kind: WorkKind,
    root: &Path,
    work_dir: &Path,
    iso: Option<worktree::Isolation>,
    base: Option<Snapshot>,
) -> Result<Work, String> {
    start_work_with_id(who, &new_id(), kind, root, work_dir, iso, base).await
}

/// Snapshot the work's folder (or take `base`) and record its verification.
async fn start_work_with_id(
    who: &Starting<'_>,
    id: &str,
    kind: WorkKind,
    root: &Path,
    work_dir: &Path,
    iso: Option<worktree::Isolation>,
    base: Option<Snapshot>,
) -> Result<Work, String> {
    let shadow = Shadow::open(&Work::home(id).join("shadow.git"), work_dir).await?;
    let base = match base {
        Some(b) => b,
        None => shadow.snapshot().await?,
    };
    let (isolation, branch) = match &iso {
        Some(worktree::Isolation::Git(wt)) => ("worktree", wt.branch.clone()),
        Some(worktree::Isolation::Copy(_)) => ("copy", String::new()),
        None => ("none", String::new()),
    };
    let state = iso.as_ref().and_then(|i| serde_json::to_value(i).ok());
    who.store
        .create_verification(
            &db::NewVerification {
                id,
                agent_id: who.agent_id,
                session_key: who.session_key,
                chat_id: who.chat_id,
                workflow_run_id: who.workflow_run_id,
                activity_id: who.activity_id,
                spec: who.spec,
                project_root: &root.to_string_lossy(),
                work_dir: &work_dir.to_string_lossy(),
                isolation,
                work_kind: kind.as_str(),
                isolation_state: state.as_ref(),
                branch: &branch,
                base_ref: &base.0,
                reviewer_prefix: &reviewer_prefix(who.session_key, id),
                ..Default::default()
            },
            chrono::Utc::now().timestamp(),
        )
        .map_err(|e| format!("Could not record the work: {e}"))?;
    Ok(Work {
        id: id.to_string(),
        kind,
        project_root: root.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        iso,
        shadow,
        base,
        last: None,
    })
}

/// A work as its row left it.
async fn resume(store: &db::Store, row: &db::Verification) -> Option<Work> {
    let shadow = Shadow::open(
        &Work::home(&row.id).join("shadow.git"),
        Path::new(&row.work_dir),
    )
    .await
    .ok()?;
    let iso = row
        .isolation_state
        .clone()
        .and_then(|s| serde_json::from_value::<worktree::Isolation>(s).ok());
    let last = store
        .verification_rounds(&row.id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|r| r.kind == "review" && !r.head_ref.is_empty())
        .map(|r| Snapshot(r.head_ref));
    Some(Work {
        id: row.id.clone(),
        kind: WorkKind::parse(&row.work_kind),
        project_root: PathBuf::from(&row.project_root),
        work_dir: PathBuf::from(&row.work_dir),
        iso,
        shadow,
        base: Snapshot(row.base_ref.clone()),
        last,
    })
}

/// What the builder is told when its work starts: where it works now, the
/// rules it works by, and who else is working there.
fn started_note(work: &Work, who: &Starting<'_>) -> String {
    let mut note = match work.kind {
        WorkKind::Isolated => format!(
            "This project ({}) is code, so your changes to it go to your own working copy at {}: every path in the project \
             points there. They land on the project when independent reviewers and the project's own checks have verified \
             them, or when the owner chooses to land them anyway. Never change the owner's own copy.",
            work.project_root.display(),
            work.work_dir.display()
        ),
        _ => format!(
            "This is code ({}): when you finish, independent reviewers and the project's own checks verify it before it is \
             reported done.",
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
            .and_then(|r| {
                r.into_iter()
                    .rev()
                    .find(|r| !r.diff_stat.is_empty())
                    .map(|r| r.diff_stat)
            })
            .unwrap_or_else(|| "files not reviewed yet".to_string());
        note.push_str(&format!(
            "\n\n{name} is also working in this project ({}). Message them before you touch the same files, and stay off theirs.",
            files.replace('\n', "; ")
        ));
    }
    note.push_str("\n\n");
    note.push_str(&super::rules::brief_rules(
        &work.project_root,
        who.owner_rules,
    ));
    note
}

/// The page folders of the apps `agent_id` may work on.
fn app_pages_of(store: &db::Store, agent_id: &str) -> Vec<PathBuf> {
    store
        .list_agents(1_000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_app.unwrap_or(0) != 0 && tools::app_dev::may_work_on(store, agent_id, a))
        .filter_map(|a| tools::app_dev::served_dir(&a).map(PathBuf::from))
        .collect()
}

/// The app an employee call names (`id`, else `name`), if it exists.
fn existing_app(store: &db::Store, input: &serde_json::Value) -> Option<db::models::Agent> {
    if let Some(id) = input["id"].as_str() {
        return store.get_agent(id).ok().flatten();
    }
    let name = input["name"].as_str()?;
    store
        .list_agents(1_000, 0)
        .ok()?
        .into_iter()
        .find(|a| a.is_app.unwrap_or(0) != 0 && a.name == name)
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
                let boundary =
                    after.is_empty() || after.starts_with(['/', '"', '\'', ' ', '\n', ';', ')']);
                let before_ok = at == 0
                    || !rest[..at].ends_with(|c: char| {
                        c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
                    });
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
        serde_json::Value::Array(items) => items
            .iter_mut()
            .fold(false, |acc, v| rewrite(v, from, to) || acc),
        serde_json::Value::Object(map) => map
            .values_mut()
            .fold(false, |acc, v| rewrite(v, from, to) || acc),
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
        assert_eq!(
            input["command"],
            "cd /copies/v-1 && pnpm test > /work/site-notes.txt"
        );
        assert_eq!(
            input["other"],
            json!(["/copies/v-1", "/work/sitemap/x"]),
            "a neighbour with the same prefix stays"
        );
        assert_eq!(
            mapped("/work/site/src", "/work/site", "/c"),
            Some("/c/src".into())
        );
        assert_eq!(mapped("/work/sitemap", "/work/site", "/c"), None);
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn who<'a>(store: &'a db::Store) -> Starting<'a> {
        Starting {
            store,
            agent_id: "dev",
            session_key: "agent:dev:web",
            chat_id: "c1",
            spec: "Make a = 2",
            owner_rules: "",
            workflow_run_id: None,
            activity_id: None,
        }
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
                let who = who(&store);

                // A read changes nothing: no copy.
                let mut read = json!({"path": project.join("src/app.ts").to_string_lossy()});
                let r = work.route(&mut read, &mut None, false, &who).await.unwrap();
                assert!(!r.rewrote && r.started.is_none() && work.is_empty().await);

                // A letter beside the project is not code.
                let mut letter = json!({"path": projects.path().join("letter.txt").to_string_lossy(), "content": "Dear"});
                let r = work.route(&mut letter, &mut None, true, &who).await.unwrap();
                assert!(r.started.is_none() && work.is_empty().await);

                let mut write = json!({"path": project.join("src/app.ts").to_string_lossy(), "content": "export const a = 2;\n"});
                let r = work.route(&mut write, &mut None, true, &who).await.unwrap();
                let note = r.started.expect("the work started");
                assert!(note.contains("your own working copy") && note.contains("Never touch the owner's working copy"), "{note}");
                let works = work.snapshot().await;
                assert_eq!(works.len(), 1);
                let copy = works[0].work_dir.clone();
                assert_eq!(write["path"], copy.join("src/app.ts").to_string_lossy().as_ref(), "the write goes to the copy");
                assert!(copy.join("src/app.ts").exists());
                let row = store.get_verification(&works[0].id).unwrap().unwrap();
                assert_eq!((row.state.as_str(), row.isolation.as_str(), row.work_kind.as_str(), row.spec.as_str()), ("running", "copy", "isolated", "Make a = 2"));
                assert_eq!(row.project_root, project.to_string_lossy());
                assert_eq!(row.reviewer_prefix, format!("subagent:agent:dev:web:sa-verify-{}-", works[0].id));
                assert!(row.isolation_state.is_some(), "the copy's whole state is kept");

                // Later calls on the project go to the copy too; no second copy.
                let mut again = json!({"command": format!("cd {} && npm test", project.display())});
                let r = work.route(&mut again, &mut None, true, &who).await.unwrap();
                assert!(r.rewrote && r.started.is_none());
                assert_eq!(again["command"], format!("cd {} && npm test", copy.display()));
                assert_eq!(work.snapshot().await.len(), 1);
                assert_eq!(std::fs::read_to_string(project.join("src/app.ts")).unwrap(), "export const a = 1;\n", "the owner's copy is untouched");

                // A later turn of the conversation resumes the same work.
                let resumed = TurnWork::load(&who, None, false).await.snapshot().await;
                assert_eq!(resumed.len(), 1);
                assert_eq!((resumed[0].id.as_str(), resumed[0].work_dir.as_path()), (works[0].id.as_str(), copy.as_path()));
                assert!(matches!(resumed[0].iso, Some(worktree::Isolation::Copy(_))));
            })
        })
    }

    #[test]
    fn a_helper_in_its_own_copy_starts_its_work_with_its_turn() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                let copy = home.join("worktrees/site/h-1");
                std::fs::create_dir_all(&copy).unwrap();
                std::fs::write(copy.join("a.rs"), "fn a() {}\n").unwrap();
                let mut w = who(&store);
                w.session_key = "subagent:agent:dev:web:h-1";
                let turn = TurnWork::load(&w, Some(&copy.to_string_lossy()), false).await;
                let works = turn.snapshot().await;
                assert_eq!(works.len(), 1);
                assert_eq!(
                    (works[0].kind, works[0].work_dir.as_path()),
                    (WorkKind::HelperCopy, copy.as_path())
                );
            })
        })
    }
}
