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
//!
//! An app built in a source project of its own (a repository whose build
//! is copied into the app's pages by a deploy script) changes its pages
//! with no call naming them. A deploy is seen where it happens: a command
//! that changed an app's pages while it ran, or a linked coding agent's
//! turn that changed them. It is recorded on the work the command ran in
//! (its verification row, so a later turn resuming the work has it too),
//! or, run outside any, on the app's own pages' work; the app's live load
//! is one of that work's checks (`verify::engine`).

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
    /// Where a verification keeps its own files (its shadow):
    /// `<data_dir>/verify/<id>`. Nothing an employee or a reviewer reads is
    /// here: that is in [`Work::copies`].
    pub fn home(id: &str) -> PathBuf {
        config::data_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("verify")
            .join(id)
    }

    /// Where a verification keeps what its reviewers work in, with the
    /// working copies (`config::copies_dir`): each round's read-only copy of
    /// the change and its patch, and the one build folder its reviewers
    /// share (`review::build_dir`).
    /// `<copies>/verify/<id>`.
    pub fn copies(id: &str) -> Result<PathBuf, String> {
        config::copies_dir().map(|root| root.join("verify").join(id)).map_err(|e| e.to_string())
    }

    /// The shadow repository a work snapshots its folder with: its own, or
    /// for a coding agent's own folder the folder's, kept across turns so a
    /// snapshot only rehashes what changed since the last one.
    pub fn shadow_dir(id: &str, kind: WorkKind, root: &Path) -> PathBuf {
        match kind {
            WorkKind::Folder => config::data_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join("verify")
                .join("folders")
                .join(format!("{:016x}", tools::checkpoint::fingerprint(root.to_string_lossy().as_bytes())))
                .join("shadow.git"),
            _ => Self::home(id).join("shadow.git"),
        }
    }

    /// Remove the verification's own files and its reviewers'. The copy
    /// itself is the landing's to remove.
    pub fn clean_up(&self) {
        remove_files(&Self::home(&self.id));
        Self::clean_up_reviewers(&self.id);
    }

    /// Remove what the verification's reviewers work in ([`Work::copies`]):
    /// the rounds' copies and the build folder they share. Done the moment
    /// the verification ends, whatever its verdict, so a build's gigabytes
    /// never wait for the sweep; one checked again builds anew.
    pub fn clean_up_reviewers(id: &str) {
        if let Ok(dir) = Self::copies(id) {
            remove_files(&dir);
        }
    }
}

/// Remove a verification's folder `dir`, a failure logged.
fn remove_files(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %dir.display(), error = %e, "verification files not removed");
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
    /// A linked coding agent's own project, as it was when the turn began:
    /// its work starts (a row, a review) only if the turn changed it.
    folder: Arc<tokio::sync::Mutex<Option<(PathBuf, Snapshot)>>>,
    /// A new app's pages a call is about to write (`writes_app_pages`): the
    /// work starts once the call has made the app.
    pending_app: Arc<std::sync::Mutex<Option<String>>>,
    /// Apps' pages as they were when a linked coding agent's turn began
    /// (its own commands, which may deploy, are never Nebo's calls).
    turn_pages: Arc<std::sync::Mutex<Vec<(PathBuf, u64)>>>,
}

/// What an app's pages hold, cheaply: each file's path, size and time it
/// was written, hashed (50,000 files at most). Equal before and after: no
/// deploy wrote them.
fn pages_fingerprint(dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut files = 0usize;
    while let Some(d) = stack.pop() {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&d).into_iter().flatten().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
                continue;
            }
            files += 1;
            if files > 50_000 {
                return hash.finish();
            }
            e.path().hash(&mut hash);
            meta.len().hash(&mut hash);
            meta.modified().ok().hash(&mut hash);
        }
    }
    hash.finish()
}

/// Each of `pages` with its fingerprint now.
async fn watch(pages: Vec<PathBuf>) -> Vec<(PathBuf, u64)> {
    if pages.is_empty() {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || pages.into_iter().map(|p| {
        let f = pages_fingerprint(&p);
        (p, f)
    }).collect())
    .await
    .unwrap_or_default()
}

/// The pages of `watched` written since it was taken.
async fn written(watched: Vec<(PathBuf, u64)>) -> Vec<PathBuf> {
    if watched.is_empty() {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || {
        watched.into_iter().filter(|(p, before)| pages_fingerprint(p) != *before).map(|(p, _)| p).collect()
    })
    .await
    .unwrap_or_default()
}

/// What routing a call did.
#[derive(Debug, Default)]
pub struct Routed {
    /// The call's paths in a project now point into its copy.
    pub rewrote: bool,
    /// Work started by this call: the note the builder reads beside the
    /// call's result.
    pub started: Option<String>,
    /// A command's apps' pages as they were before it ran, and the works it
    /// runs in: handed back to [`TurnWork::after_call`] once it has, so each
    /// call is judged by its own (calls of one step run side by side).
    pub watched: Watched,
}

/// Apps' pages as they were before one command ran, and the works (ids)
/// it runs in ([`Routed::watched`]).
#[derive(Debug, Default)]
pub struct Watched {
    pages: Vec<(PathBuf, u64)>,
    works: Vec<String>,
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
            // Work still in hand: being checked, waiting on the owner, or
            // finished in a copy (this loop's, or a helper's own) that
            // hasn't landed: a change to it is checked again over the whole
            // change, never landed or reported on the old verdict.
            let open = matches!(row.state.as_str(), "running" | "paused")
                || (row.landed_ref.is_empty()
                    && (row.isolation != "none" || row.work_kind == WorkKind::HelperCopy.as_str())
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
        // is (Nebo can't move another runtime's writes into a copy). The
        // folder is snapshotted now (only what changed since its last
        // snapshot is rehashed); its work starts only if the turn changes it
        // ([`TurnWork::start_folder_if_changed`]).
        if linked
            && let Some(Folder::Project(root)) =
                cwd.and_then(|c| project::code_folder(Path::new(c), &[]))
            && !turn
                .works
                .lock()
                .await
                .iter()
                .any(|w| w.project_root == root)
            && let Ok(shadow) = Shadow::open(&Work::shadow_dir("", WorkKind::Folder, &root), &root).await
            && let Ok(base) = shadow.snapshot().await
        {
            *turn.folder.lock().await = Some((root, base));
        }
        // A linked coding agent deploys with its own commands: the apps'
        // pages as the turn found them, compared when it ends.
        if linked {
            let pages = watch(app_pages_of(who.store, who.agent_id)).await;
            *turn.turn_pages.lock().unwrap_or_else(|p| p.into_inner()) = pages;
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
    /// copy of a project, or the app's pages where they are. A command's
    /// apps' pages are watched for a deploy while it runs
    /// ([`TurnWork::after_call`]).
    pub async fn route(
        &self,
        input: &mut serde_json::Value,
        cwd: &mut Option<String>,
        changes: bool,
        who: &Starting<'_>,
    ) -> Result<Routed, String> {
        let mut routed = Routed::default();
        let app_pages = app_pages_of(who.store, who.agent_id);
        let command = changes && input.get("command").is_some_and(|c| c.is_string());
        // Taken before the works are locked: side-by-side calls never wait
        // on each other's walk of the pages.
        if command {
            routed.watched.pages = watch(app_pages.clone()).await;
        }
        let mut works = self.works.lock().await;
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
            if let Some(w) = works.iter().find(|w| w.project_root == dir || w.work_dir == dir) {
                if command {
                    routed.watched.works.push(w.id.clone());
                }
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
                    // Code work begins: the owner hears once if git can't do it.
                    worktree::git_notice(who.store, Path::new("git")).await;
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
            if command {
                routed.watched.works.push(work.id.clone());
            }
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

    /// A turn in which the linked agent ran no tools of its own: what
    /// changed in its folder, or in apps' pages, meanwhile is not its work.
    pub async fn drop_folder(&self) {
        self.folder.lock().await.take();
        self.turn_pages.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// A linked coding agent's turn changed its project, or deployed from
    /// it into an app's pages: its work starts now, from the project as the
    /// turn found it, with those deploys among its checks. A turn that
    /// changed nothing records nothing.
    pub async fn start_folder_if_changed(&self, who: &Starting<'_>) {
        let watched = std::mem::take(&mut *self.turn_pages.lock().unwrap_or_else(|p| p.into_inner()));
        let deployed = written(watched).await;
        let mut on = Vec::new();
        if let Some((root, base)) = self.folder.lock().await.take()
            && let Ok(shadow) = Shadow::open(&Work::shadow_dir("", WorkKind::Folder, &root), &root).await
            && (!deployed.is_empty() || shadow.snapshot().await.is_ok_and(|now| now != base))
        {
            match start_work(who, WorkKind::Folder, &root, &root, None, Some(base)).await {
                Ok(work) => {
                    on.push(work.id.clone());
                    self.works.lock().await.push(work);
                }
                Err(e) => tracing::warn!(project = %root.display(), error = %e, "a coding agent's change could not be recorded for review"),
            }
        }
        self.record_deploys(who, deployed, &on).await;
    }

    /// After a call ran: an app's pages a command wrote while it ran
    /// (`watched`, from its [`Routed`]) are a deploy (the builder hears
    /// that their live check joins its checks), and a new app it made
    /// starts its pages' work, from nothing (every page is the change).
    pub async fn after_call(&self, who: &Starting<'_>, watched: Watched) -> Option<String> {
        let deployed = written(watched.pages).await;
        let mut notes = self.record_deploys(who, deployed, &watched.works).await;
        notes.extend(self.after_new_app(who).await);
        (!notes.is_empty()).then(|| notes.join("\n\n"))
    }

    /// Record each app whose pages `deployed` holds as a deploy of the
    /// works `on` (ids) other than its own pages' work: its live load is
    /// one of their checks. A deploy run outside any work is its app's own
    /// pages' work, started now if none is (its round runs the live load
    /// alone). What the builder hears of it.
    async fn record_deploys(&self, who: &Starting<'_>, deployed: Vec<PathBuf>, on: &[String]) -> Vec<String> {
        let mut notes = Vec::new();
        let mut names = Vec::new();
        for pages in deployed {
            let Some(app) = app_served_from(who.store, &pages) else { continue };
            let mut works = self.works.lock().await;
            let mut gated: Vec<String> = works
                .iter()
                .filter(|w| on.contains(&w.id) && !(w.kind == WorkKind::AppPages && w.work_dir == pages))
                .map(|w| w.id.clone())
                .collect();
            if gated.is_empty() {
                // Its own pages' work: a change to the pages it already
                // checks; a new one from the pages as the deploy left them.
                if works.iter().any(|w| w.kind == WorkKind::AppPages && w.work_dir == pages) {
                    continue;
                }
                match start_work(who, WorkKind::AppPages, &pages, &pages, None, None).await {
                    Ok(work) => {
                        notes.push(started_note(&work, who));
                        gated.push(work.id.clone());
                        works.push(work);
                    }
                    Err(e) => {
                        tracing::warn!(app = %app.id, error = %e, "a deploy could not be recorded for checking");
                        continue;
                    }
                }
            }
            for id in &gated {
                let _ = who.store.add_verification_deploy(id, &app.id);
            }
            names.push(app);
        }
        if !names.is_empty() {
            let holds_reload = !tools::app_dev::withheld(who.store, who.agent_id).iter().any(|t| t == tools::app_dev::APP_RELOAD);
            let list: Vec<String> = names
                .iter()
                .map(|a| if holds_reload { format!("{} (`{}`)", a.name, live_check_call(&a.id)) } else { a.name.clone() })
                .collect();
            let how = if holds_reload {
                "Run it yourself after each deploy, and fix what it reports."
            } else {
                "It runs when you finish, and what it reports comes back to you to fix."
            };
            notes.push(format!(
                "This command deployed into the pages of {}: their live load on a phone and a desktop is one of the \
                 checks of your work now. {how}",
                list.join(", ")
            ));
        }
        notes
    }

    /// A new app a call made starts its pages' work.
    async fn after_new_app(&self, who: &Starting<'_>) -> Option<String> {
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
    let shadow = Shadow::open(&Work::shadow_dir(id, kind, root), work_dir).await?;
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

/// What `v`'s change was reviewed as (each path it touches, with what the
/// review saw there), when its folder holds exactly what its last review
/// round reviewed (a verification vouches for those bytes, never for what
/// came after); `None` otherwise.
pub async fn reviewed(store: &db::Store, v: &db::Verification) -> Option<worktree::Reviewed> {
    let reviewed = store
        .verification_rounds(&v.id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|r| r.kind == "review" && !r.head_ref.is_empty())
        .map(|r| Snapshot(r.head_ref))?;
    let kind = WorkKind::parse(&v.work_kind);
    let shadow = Shadow::open(&Work::shadow_dir(&v.id, kind, Path::new(&v.project_root)), Path::new(&v.work_dir)).await.ok()?;
    if shadow.snapshot().await.ok()? != reviewed {
        return None;
    }
    let paths = shadow.diff(&Snapshot(v.base_ref.clone()), &reviewed).await.ok()?.paths();
    shadow.contents(&reviewed, &paths).await.ok()
}

/// A work as its row left it.
async fn resume(store: &db::Store, row: &db::Verification) -> Option<Work> {
    let kind = WorkKind::parse(&row.work_kind);
    let shadow = Shadow::open(
        &Work::shadow_dir(&row.id, kind, Path::new(&row.project_root)),
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
        kind,
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

/// The app served from `pages`, if one is.
pub fn app_served_from(store: &db::Store, pages: &Path) -> Option<db::models::Agent> {
    store
        .list_agents(1_000, 0)
        .unwrap_or_default()
        .into_iter()
        .find(|a| tools::app_dev::served_dir(a).is_some_and(|d| Path::new(d) == pages))
}

/// The call that runs the app `app_id`'s live check: it names the app, so
/// it checks that one whoever makes it.
pub fn live_check_call(app_id: &str) -> String {
    format!("{}(app: \"{app_id}\", check: true)", tools::app_dev::APP_RELOAD)
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

/// How long a finished verification's own files (its shadow, its patches)
/// are kept, and how long a build folder or a folder's shadow may sit
/// unused before it goes.
pub const KEEP_FINISHED_SECS: u64 = 7 * 24 * 3600;
/// The most the verify loop's build folders may hold together; past it the
/// least recently used go first.
pub const BUILD_CAP_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Hold the build folders to [`BUILD_CAP_BYTES`] now, least recently used
/// first: run before checks write into them, so the cap holds as they are
/// written, not only at the daily sweep. Returns how many it removed.
pub fn cap_builds() -> usize {
    let Some(root) = super::checks::builds_root() else { return 0 };
    let mut builds: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let at = std::fs::metadata(e.path()).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            (at, size_of(&e.path()), e.path())
        })
        .collect();
    builds.sort();
    let mut total: u64 = builds.iter().map(|(_, n, _)| n).sum();
    let mut removed = 0;
    for (_, n, path) in builds {
        if total <= BUILD_CAP_BYTES {
            break;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            total = total.saturating_sub(n);
            removed += 1;
        }
    }
    removed
}

/// Reclaim the verify loop's disk (run at boot and daily): the files of a
/// verification finished more than [`KEEP_FINISHED_SECS`] ago (or with no
/// row), a finished worktree copy that never landed put away on its branch
/// (its work kept, the disk freed), build folders and folder shadows unused
/// that long, and the build folders over [`BUILD_CAP_BYTES`], least recently
/// used first. A scratch copy is never removed: its work lives only there.
/// Returns how many folders it removed.
pub async fn sweep(store: &db::Store, now: i64) -> usize {
    let root = config::data_dir().unwrap_or_else(|_| std::env::temp_dir()).join("verify");
    // A verification's own files, and its reviewers' with the working copies.
    let reviewers = config::copies_dir().ok().map(|c| c.join("verify"));
    let entries: Vec<std::fs::DirEntry> =
        std::iter::once(&root).chain(reviewers.as_ref()).flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten()).collect();
    let old = |at: i64| now.saturating_sub(at) as u64 > KEEP_FINISHED_SECS;
    let mut removed = 0;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("v-") {
            continue;
        }
        let idle = std::fs::metadata(entry.path())
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_secs() > KEEP_FINISHED_SECS);
        let finished = match store.get_verification(&name).ok().flatten() {
            // No row: an orphan, once nothing has touched it for as long.
            None => idle,
            Some(v) if matches!(v.state.as_str(), "running" | "paused") => false,
            Some(v) => {
                if !old(v.finished_at.unwrap_or(v.created_at)) {
                    false
                } else {
                    // Its copy, never landed: put away on its branch.
                    if v.landed_ref.is_empty()
                        && let Some(mut iso) = v.isolation_state.clone().and_then(|s| serde_json::from_value::<worktree::Isolation>(s).ok())
                        && matches!(iso, worktree::Isolation::Git(_))
                        && iso.path().exists()
                    {
                        worktree::put_away(&mut iso, "Verified work never landed, put away to free the disk").await;
                        let _ = store.set_verification_isolation(&v.id, &serde_json::to_value(&iso).unwrap_or_default());
                    }
                    true
                }
            }
        };
        if finished && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    let unused = |p: &Path| {
        std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() > KEEP_FINISHED_SECS)
    };
    for dir in std::iter::once(root.join("folders")).chain(super::checks::builds_root()) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if unused(&entry.path()) && std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed + cap_builds()
}

/// The bytes the files under `dir` hold.
fn size_of(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
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

    /// The build folder a verification's reviewers share goes when it ends,
    /// with their copies; its own files (the shadow) stay for the landing.
    #[test]
    fn an_ended_verification_leaves_no_build_behind() {
        crate::test_home::with_home(|_| {
            let scratch = Work::copies("v-ended").unwrap();
            let build = crate::harness::verify::review::build_dir(&scratch);
            std::fs::create_dir_all(build.join("debug/deps")).unwrap();
            std::fs::create_dir_all(scratch.join("round-1/project")).unwrap();
            std::fs::create_dir_all(Work::home("v-ended")).unwrap();
            Work::clean_up_reviewers("v-ended");
            assert!(!scratch.exists(), "{}", scratch.display());
            assert!(Work::home("v-ended").is_dir());
            // Ended again (nothing left): nothing to do.
            Work::clean_up_reviewers("v-ended");
        })
    }

    fn rt()-> tokio::runtime::Runtime {
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

    /// The verify loop reclaims its disk: a verification finished long ago
    /// leaves nothing behind; one still running, one finished lately, and a
    /// fresh folder with no row are kept.
    #[test]
    fn finished_verifications_leave_no_files_behind() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                let now = 10 * KEEP_FINISHED_SECS as i64;
                for (id, state, at) in [("v-old", "verified", 1), ("v-live", "running", 1), ("v-new", "not_verified", now - 60)] {
                    store
                        .create_verification(&db::NewVerification { id, session_key: "agent:a:web", isolation: "none", work_kind: "isolated", ..Default::default() }, at)
                        .unwrap();
                    store
                        .set_verification_state(id, &db::VerificationState { state, ..Default::default() }, at)
                        .unwrap();
                    std::fs::create_dir_all(Work::home(id).join("shadow.git")).unwrap();
                }
                std::fs::create_dir_all(Work::home("v-orphan")).unwrap();
                assert_eq!(sweep(&store, now).await, 1);
                assert!(!Work::home("v-old").exists(), "finished long ago: gone");
                for kept in ["v-live", "v-new", "v-orphan"] {
                    assert!(Work::home(kept).exists(), "{kept} is kept");
                }
            })
        })
    }

    /// An app built in a source project of its own: a command that wrote
    /// its pages while it ran is a deploy, recorded on the work it ran in
    /// (its row, so a later turn resuming the work has it), and the builder
    /// hears its live check joins its checks. A command that wrote nothing
    /// there deploys nothing; one run outside any work is its app's own
    /// pages' work, with the deploy to check.
    #[test]
    fn a_command_that_writes_an_apps_pages_is_a_deploy_of_the_work_it_ran_in() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                let folder = tempfile::tempdir().unwrap();
                let ui = folder.path().join("ui");
                std::fs::create_dir_all(&ui).unwrap();
                std::fs::write(ui.join("index.html"), "<h1>One</h1>").unwrap();
                store.create_agent("dev", Some("user"), "Scoreboard", "", "---\nname: Scoreboard\n---\n", "{}", None, None).unwrap();
                store.set_agent_app_fields("dev", true, Some(&ui.to_string_lossy()), None, None).unwrap();
                let projects = tempfile::tempdir().unwrap();
                let project = projects.path().join("scoreboard");
                std::fs::create_dir_all(project.join("src")).unwrap();
                std::fs::write(project.join("package.json"), "{}").unwrap();
                let who = who(&store);

                let work = TurnWork::default();
                let mut idle = json!({"command": format!("cd {} && echo hi", project.display())});
                let routed = work.route(&mut idle, &mut None, true, &who).await.unwrap();
                assert_eq!(work.after_call(&who, routed.watched).await, None, "nothing written: no deploy");
                let works = work.snapshot().await;
                assert_eq!(works.len(), 1, "the command's project is a work");
                let row = store.get_verification(&works[0].id).unwrap().unwrap();
                assert!(row.deployed_apps.is_empty() && !row.deploy_unchecked);

                let mut deploy = json!({"command": format!("cd {} && pnpm deploy", project.display())});
                let routed = work.route(&mut deploy, &mut None, true, &who).await.unwrap();
                std::fs::write(ui.join("index.html"), "<h1>Two, a longer page</h1>").unwrap();
                std::fs::write(ui.join("app.js"), "console.log(1)").unwrap();
                let note = work.after_call(&who, routed.watched).await.expect("the builder hears of the deploy");
                assert!(note.contains("deployed into the pages of Scoreboard") && note.contains(r#"app_reload(app: "dev", check: true)"#), "{note}");
                let row = store.get_verification(&works[0].id).unwrap().unwrap();
                assert_eq!((row.deployed_apps, row.deploy_unchecked), (vec!["dev".to_string()], true), "kept on the work it ran in");
                assert_eq!(work.snapshot().await.len(), 1, "no work of its own: the project's checks it");

                // Run outside any work: the app's own pages' work, from the
                // pages as the deploy left them, with the deploy to check.
                let elsewhere = TurnWork::default();
                let mut copy = json!({"command": "scoreboard-deploy"});
                let routed = elsewhere.route(&mut copy, &mut None, true, &who).await.unwrap();
                std::fs::write(ui.join("index.html"), "<h1>Three</h1>").unwrap();
                elsewhere.after_call(&who, routed.watched).await.expect("the builder hears of the deploy");
                let works = elsewhere.snapshot().await;
                assert_eq!((works.len(), works[0].kind, works[0].work_dir.clone()), (1, WorkKind::AppPages, ui.clone()));
                let row = store.get_verification(&works[0].id).unwrap().unwrap();
                assert_eq!((row.deployed_apps, row.deploy_unchecked), (vec!["dev".to_string()], true));
            })
        })
    }

    /// Review 2026-10-20 M8: a linked coding agent's turn that changed
    /// nothing records nothing (no row made and deleted), and one that
    /// changed its project starts the work from the project as the turn
    /// found it.
    #[test]
    fn a_coding_agents_turn_records_work_only_when_it_changed_its_project() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                let projects = tempfile::tempdir().unwrap();
                let project = projects.path().join("site");
                std::fs::create_dir_all(&project).unwrap();
                std::fs::write(project.join("package.json"), "{}").unwrap();
                std::fs::write(project.join("app.ts"), "export const a = 1;\n").unwrap();
                let w = who(&store);
                let cwd = project.to_string_lossy().into_owned();

                let quiet = TurnWork::load(&w, Some(&cwd), true).await;
                quiet.start_folder_if_changed(&w).await;
                assert!(quiet.is_empty().await);
                assert!(store.list_verifications(&db::VerificationQuery::default()).unwrap().is_empty(), "no row for a turn that wrote nothing");

                let busy = TurnWork::load(&w, Some(&cwd), true).await;
                std::fs::write(project.join("app.ts"), "export const a = 2;\n").unwrap();
                busy.start_folder_if_changed(&w).await;
                let works = busy.snapshot().await;
                assert_eq!(works.len(), 1);
                assert_eq!(works[0].kind, WorkKind::Folder);
                let change = works[0].shadow.diff(&works[0].base, &works[0].shadow.snapshot().await.unwrap()).await.unwrap();
                assert_eq!(change.paths(), ["app.ts"], "measured from the project as the turn found it");
            })
        })
    }

    /// Review round 3: owner, editor or watcher changes in a linked agent's
    /// folder during a turn in which the agent ran no tools are not its
    /// work: nothing is recorded.
    #[test]
    fn changes_during_a_turn_the_agent_did_not_act_are_not_its_work() {
        crate::test_home::with_home(|home| {
            rt().block_on(async {
                let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
                let projects = tempfile::tempdir().unwrap();
                let project = projects.path().join("site");
                std::fs::create_dir_all(&project).unwrap();
                std::fs::write(project.join("package.json"), "{}").unwrap();
                let w = who(&store);
                let turn = TurnWork::load(&w, Some(&project.to_string_lossy()), true).await;
                std::fs::write(project.join("notes.md"), "the owner's own note\n").unwrap();
                turn.drop_folder().await;
                turn.start_folder_if_changed(&w).await;
                assert!(turn.is_empty().await);
                assert!(store.list_verifications(&db::VerificationQuery::default()).unwrap().is_empty());
            })
        })
    }
}
