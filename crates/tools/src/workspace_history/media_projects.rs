//! The owner's Nebo Media projects, moved once from the workspace's
//! `Media/Projects/<p>` into its Media Studio folder
//! (`config::WorkspaceFolder::MediaStudio`), with everything Nebo keeps about
//! their files: the history index, their earlier contents and what an
//! employee created there.
//!
//! - Nothing is ever overwritten: a name the Media Studio folder already
//!   holds (a link included, even a dangling one) is kept beside it as
//!   `<name>-2` (`-3`, …).
//! - Every entry moves by one rename and is checked (the new path is there,
//!   the old one gone). A project is all in one place or all in the other.
//! - What goes where is written down first (`.moving.json` in the old
//!   folder), so a move cut short (a crash, a full disk) goes on at the next
//!   start with the same names.
//! - Once moved, a project's records follow it, and the projects' own JSON
//!   files (stored timelines, `project.json`) have their paths to every MOVED
//!   project rewritten; a path to a project still in the old folder is left
//!   as it is until that one moves.
//!
//! The old `Projects` folder is removed once everything is out; `Media`
//! (Nebo Media's brand kits, looks and loose files) stays.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where Nebo Media kept its projects before the workspace had a Media
/// Studio folder, under the workspace.
const OLD: [&str; 2] = ["Media", "Projects"];
/// The move's plan, `[[old name, new name], …]`, kept in the old folder
/// until everything is out of it.
const JOURNAL: &str = ".moving.json";
/// The suffix of a JSON file being rewritten (`.<name>.moving`): a leftover
/// from a cut-short rewrite is removed.
const REWRITING: &str = ".moving";

/// What a move did, entry by entry.
#[derive(Debug, Default, PartialEq)]
pub struct MediaProjectsMove {
    /// `(old name, new name)` of every entry moved by this run; the names
    /// differ when the Media Studio folder already had one of that name.
    pub moved: Vec<(String, String)>,
    /// Entries that could not be moved: still in the old folder, tried again
    /// at the next start.
    pub left: Vec<String>,
}

/// Whether anything is at `p`, a link (even a dangling one) included.
fn taken(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// The move for `workspace` and its Media Studio folder `studio`. `home`
/// spells the `~/` form of the paths the JSON files may hold.
pub fn move_media_projects(store: &db::Store, workspace: &Path, studio: &Path, home: Option<&Path>) -> MediaProjectsMove {
    // No look runs while the files and their records move.
    let _one = super::one_at_a_time();
    let old = OLD.iter().fold(workspace.to_path_buf(), |p, part| p.join(part));
    let journal = old.join(JOURNAL);
    let mut done = MediaProjectsMove::default();
    let Ok(entries) = std::fs::read_dir(&old) else {
        return done;
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let os: OsString = entry.file_name();
        match os.into_string() {
            Ok(n) if n == ".DS_Store" || n == JOURNAL => {}
            Ok(n) => names.push(n),
            // A name that is not text can't be planned or rewritten: it
            // stays, and says so at every start.
            Err(os) => done.left.push(os.to_string_lossy().into_owned()),
        }
    }
    names.sort();
    if let Err(e) = std::fs::create_dir_all(studio) {
        tracing::warn!(error = %e, folder = %studio.display(), "the Media Studio folder could not be made; media projects stay where they are");
        done.left.extend(names);
        return done;
    }

    // Every entry's new name before anything moves; a name chosen earlier
    // that something has taken since is chosen again.
    let mut plan: Vec<(String, String)> =
        std::fs::read(&journal).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    plan.retain(|(from, to)| !(names.contains(from) && taken(&studio.join(to))));
    let unplanned: Vec<String> = names.iter().filter(|n| !plan.iter().any(|(f, _)| f == *n)).cloned().collect();
    for name in &unplanned {
        let is_dir = old.join(name).is_dir();
        let mut target = name.clone();
        let mut n = 2;
        while taken(&studio.join(&target)) || plan.iter().any(|(_, t)| *t == target) {
            target = numbered(name, n, is_dir);
            n += 1;
        }
        plan.push((name.clone(), target));
    }
    if let Err(e) = serde_json::to_vec(&plan).map_err(std::io::Error::other).and_then(|b| std::fs::write(&journal, b)) {
        tracing::warn!(error = %e, "the media projects move could not be written down; nothing is moved");
        done.left.extend(names);
        return done;
    }

    for (name, target) in plan.iter().filter(|(f, _)| names.contains(f)) {
        let (from, to) = (old.join(name), studio.join(target));
        if move_entry(&from, &to) && taken(&to) && !taken(&from) {
            done.moved.push((name.clone(), target.clone()));
        } else {
            done.left.push(name.clone());
        }
    }

    // What has moved, by this run or an earlier one: its records follow it,
    // and the JSON files' paths to it are rewritten.
    let moved: Vec<&(String, String)> =
        plan.iter().filter(|(f, t)| !taken(&old.join(f)) && taken(&studio.join(t))).collect();
    let rel = |p: &Path| p.strip_prefix(workspace).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
    let file = |p: &Path| format!("file:{}", p.to_string_lossy());
    for (from, to) in &moved {
        let (a, b) = (old.join(from), studio.join(to));
        if let (Some(ra), Some(rb)) = (rel(&a), rel(&b))
            && let Err(e) = store.move_workspace_records(&ra, &rb, &file(&a), &file(&b))
        {
            tracing::warn!(error = %e, project = %from, "a moved media project's records could not follow it; they are tried again at the next start");
            done.left.push(from.clone());
        }
    }
    let rewrites = rewrites(&old, studio, home, &moved);
    for (_, to) in &moved {
        let dir = studio.join(to);
        if dir.is_dir() && !dir.is_symlink() {
            rewrite_paths(&dir, &rewrites);
        }
    }
    if done.left.is_empty() {
        let _ = std::fs::remove_file(&journal);
        let _ = std::fs::remove_file(old.join(".DS_Store"));
        let _ = std::fs::remove_dir(&old);
    }
    done
}

/// Move one entry. A link with a relative target would point elsewhere one
/// folder shallower, so it is made again at the new place pointing where it
/// pointed, then the old link goes; everything else is one rename.
fn move_entry(from: &Path, to: &Path) -> bool {
    if let Ok(target) = std::fs::read_link(from)
        && target.is_relative()
    {
        let Some(parent) = from.parent() else { return false };
        // Where it points, as a full path that needs nothing of the old
        // folder (which goes): resolved, or spelled out when it dangles.
        let points = std::fs::canonicalize(from).unwrap_or_else(|_| lexical(&parent.join(&target)));
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&points, to);
        #[cfg(windows)]
        let made = if points.is_dir() {
            std::os::windows::fs::symlink_dir(&points, to)
        } else {
            std::os::windows::fs::symlink_file(&points, to)
        };
        return made.is_ok() && std::fs::remove_file(from).is_ok();
    }
    std::fs::rename(from, to).is_ok()
}

/// `p` with its `.` and `..` parts worked out by name alone.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// `name` numbered `n` to keep it beside one already there: `reel-2`, or
/// `notes-2.md` for a file.
fn numbered(name: &str, n: u32, is_dir: bool) -> String {
    match name.rsplit_once('.').filter(|(stem, _)| !is_dir && !stem.is_empty()) {
        Some((stem, ext)) => format!("{stem}-{n}.{ext}"),
        None => format!("{name}-{n}"),
    }
}

/// Each moved entry's old path, as a JSON file may spell it, with its new
/// one: the absolute path with `/`, the `~/` path when the workspace is
/// under `home`, and (where the OS writes `\`) the absolute path as JSON
/// escapes it. Each is followed by a separator or the string's end, so
/// `reel` never matches `reel-2`.
fn rewrites(old: &Path, studio: &Path, home: Option<&Path>, moved: &[&(String, String)]) -> Vec<(String, String)> {
    let mut roots: Vec<(String, String, &str)> = Vec::new();
    let (o, s) = (old.to_string_lossy().into_owned(), studio.to_string_lossy().into_owned());
    roots.push((o.replace('\\', "/"), s.replace('\\', "/"), "/"));
    if o.contains('\\') {
        roots.push((o.replace('\\', "\\\\"), s.replace('\\', "\\\\"), "\\\\"));
    }
    if let Some(home) = home
        && let (Ok(ro), Ok(rs)) = (old.strip_prefix(home), studio.strip_prefix(home))
    {
        let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
        roots.push((format!("~/{}", slash(ro)), format!("~/{}", slash(rs)), "/"));
    }
    let mut out = Vec::new();
    for (o, s, sep) in &roots {
        for (from, to) in moved {
            for end in [*sep, "\""] {
                out.push((format!("{o}{sep}{from}{end}"), format!("{s}{sep}{to}{end}")));
            }
        }
    }
    out
}

/// Apply `rewrites` to every JSON file under `dir`. Hidden entries and links
/// are left alone (a leftover `.<name>.moving` is removed); a file is
/// replaced whole, written beside it with its permissions, synced, then
/// renamed over it, and only when something in it changed.
fn rewrite_paths(dir: &Path, rewrites: &[(String, String)]) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if name.starts_with('.') {
            if kind.is_file() && name.ends_with(&format!(".json{REWRITING}")) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            rewrite_paths(&path, rewrites);
            continue;
        }
        if !name.ends_with(".json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let new = rewrites.iter().fold(text.clone(), |t, (a, b)| t.replace(a, b));
        if new == text {
            continue;
        }
        let part: PathBuf = dir.join(format!(".{name}{REWRITING}"));
        let written = (|| -> std::io::Result<()> {
            let perms = std::fs::metadata(&path)?.permissions();
            let mut f = std::fs::File::create(&part)?;
            std::io::Write::write_all(&mut f, new.as_bytes())?;
            f.sync_all()?;
            std::fs::set_permissions(&part, perms)?;
            std::fs::rename(&part, &path)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&part);
            tracing::warn!(error = %e, file = %path.display(), "a media project's paths could not be rewritten");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A home with `~/NeboAI` as the workspace and a database.
    struct Home {
        _tmp: tempfile::TempDir,
        store: db::Store,
        home: PathBuf,
        workspace: PathBuf,
        studio: PathBuf,
        old: PathBuf,
    }

    impl Home {
        fn new() -> Home {
            let tmp = tempfile::TempDir::new().unwrap();
            let home = tmp.path().canonicalize().unwrap();
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let workspace = home.join("NeboAI");
            fs::create_dir_all(&workspace).unwrap();
            Home { studio: workspace.join("Media Studio"), old: workspace.join("Media/Projects"), workspace, home, store, _tmp: tmp }
        }

        fn run(&self) -> MediaProjectsMove {
            move_media_projects(&self.store, &self.workspace, &self.studio, Some(&self.home))
        }

        /// A project in the old folder whose stored timeline names its own
        /// media both ways Nebo Media writes paths, and `other`'s.
        fn old_project(&self, name: &str, other: &str) {
            let dir = self.old.join(name);
            fs::create_dir_all(dir.join("work")).unwrap();
            fs::create_dir_all(dir.join("deliver")).unwrap();
            fs::write(dir.join("project.json"), r#"{"timelines":{}}"#).unwrap();
            fs::write(dir.join("deliver").join("cut.mp4"), format!("{name} video")).unwrap();
            let abs = self.old.join(name).join("frames/a.png");
            let timeline = format!(
                r#"{{"assets":{{"a":{{"src":"~/NeboAI/Media/Projects/{name}/frames/a.png"}},"b":{{"src":"{}"}},"c":{{"src":"~/NeboAI/Media/Projects/{other}/voice/v.mp3"}},"d":{{"src":"~/NeboAI/Media/Projects/{other}-old/x.png"}}}}}}"#,
                abs.display()
            );
            fs::write(dir.join("work").join(format!("{name}-v1.timeline.json")), timeline).unwrap();
        }

        fn timeline(&self, project: &str, name: &str) -> String {
            fs::read_to_string(self.studio.join(project).join("work").join(format!("{name}-v1.timeline.json"))).unwrap()
        }

        fn look(&self) -> crate::workspace_history::Look {
            crate::workspace_history::look(&self.store, &self.workspace, &self.home.join("files"), Some("c1")).unwrap()
        }
    }

    /// Every project lands in the Media Studio folder under its own name,
    /// its timelines name the new paths (never a longer name's), and the old
    /// folder is gone; `Media` itself, with Nebo Media's other folders, stays.
    #[test]
    fn projects_move_into_the_media_studio_folder() {
        let h = Home::new();
        h.old_project("reel", "launch");
        h.old_project("launch", "reel");
        fs::create_dir_all(h.workspace.join("Media/brand/acme")).unwrap();
        fs::write(h.old.join(".DS_Store"), "x").unwrap();

        let done = h.run();
        assert_eq!(done.moved, vec![("launch".into(), "launch".into()), ("reel".into(), "reel".into())]);
        assert!(done.left.is_empty());
        assert_eq!(fs::read_to_string(h.studio.join("reel/deliver/cut.mp4")).unwrap(), "reel video");
        let reel = h.timeline("reel", "reel");
        assert!(reel.contains("\"~/NeboAI/Media Studio/reel/frames/a.png\""), "{reel}");
        assert!(reel.contains(&format!("\"{}\"", h.studio.join("reel/frames/a.png").display())), "{reel}");
        assert!(reel.contains("\"~/NeboAI/Media Studio/launch/voice/v.mp3\""), "{reel}");
        assert!(reel.contains("\"~/NeboAI/Media/Projects/launch-old/x.png\""), "a project that never existed is not renamed: {reel}");
        assert!(!h.old.exists(), "the old projects folder is gone");
        assert!(h.workspace.join("Media/brand/acme").is_dir(), "Media's other folders stay");
    }

    /// Nebo's records follow the files: the next look finds nothing changed
    /// or deleted, earlier contents name the new path, and what an employee
    /// made there is still that employee's.
    #[test]
    fn the_records_follow_the_files() {
        let h = Home::new();
        h.old_project("reel", "reel");
        h.look();
        let cut = h.old.join("reel/deliver/cut.mp4");
        fs::write(&cut, "second cut").unwrap();
        assert_eq!(h.look().kept, 1, "the first cut is kept as history");
        h.store.add_employee_created("emp", &format!("file:{}", cut.display())).unwrap();

        h.run();
        let after = h.look();
        // The one change is the timeline's rewritten paths: its earlier
        // content is kept, at its new path. Nothing reads as deleted.
        assert_eq!(after.kept, 1, "{after:?}");
        let rewritten = crate::workspace_history::history(&h.store, "Media Studio/reel/work/reel-v1.timeline.json").unwrap();
        assert_eq!(rewritten.len(), 1);
        assert_eq!(rewritten[0].reason, "modified");
        let history = crate::workspace_history::history(&h.store, "Media Studio/reel/deliver/cut.mp4").unwrap();
        assert_eq!(history.len(), 1, "the earlier cut names the new path");
        assert!(crate::workspace_history::history(&h.store, "Media/Projects/reel/deliver/cut.mp4").unwrap().is_empty());
        let mine = format!("file:{}", h.studio.join("reel/deliver/cut.mp4").display());
        assert!(h.store.created_by("emp", std::slice::from_ref(&mine)).unwrap().contains(&mine), "still the employee's");
    }

    /// A second start finds nothing to do and changes nothing.
    #[test]
    fn a_second_run_does_nothing() {
        let h = Home::new();
        h.old_project("reel", "reel");
        h.run();
        let before = h.timeline("reel", "reel");
        assert_eq!(h.run(), MediaProjectsMove::default());
        assert_eq!(h.timeline("reel", "reel"), before);
        assert_eq!(fs::read_dir(&h.studio).unwrap().count(), 1);
    }

    /// A name the Media Studio folder already has is never overwritten (a
    /// dangling link counts): both are kept, the moved one numbered, its
    /// paths to itself following it.
    #[test]
    fn a_name_already_there_is_never_overwritten() {
        let h = Home::new();
        fs::create_dir_all(h.studio.join("reel")).unwrap();
        fs::write(h.studio.join("reel/project.json"), "the one already there").unwrap();
        fs::write(h.studio.join("notes.md"), "mine").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(h.home.join("nowhere"), h.studio.join("promo")).unwrap();
        h.old_project("reel", "reel");
        h.old_project("promo", "promo");
        fs::write(h.old.join("notes.md"), "moved").unwrap();

        let done = h.run();
        assert!(done.left.is_empty(), "{done:?}");
        assert!(done.moved.contains(&("reel".into(), "reel-2".into())) && done.moved.contains(&("notes.md".into(), "notes-2.md".into())));
        assert_eq!(fs::read_to_string(h.studio.join("reel/project.json")).unwrap(), "the one already there");
        assert_eq!(fs::read_to_string(h.studio.join("notes.md")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(h.studio.join("notes-2.md")).unwrap(), "moved");
        assert_eq!(fs::read_to_string(h.studio.join("reel-2/deliver/cut.mp4")).unwrap(), "reel video");
        let moved = h.timeline("reel-2", "reel");
        assert!(moved.contains("\"~/NeboAI/Media Studio/reel-2/frames/a.png\"") && !moved.contains("Media Studio/reel/"), "{moved}");
        #[cfg(unix)]
        {
            assert!(done.moved.contains(&("promo".into(), "promo-2".into())));
            assert!(fs::symlink_metadata(h.studio.join("promo")).unwrap().file_type().is_symlink(), "the owner's link is untouched");
        }
        assert!(!h.old.exists());
    }

    /// A move cut short: one project already moved but not yet rewritten
    /// (the plan was written down), one collision planned, one never
    /// started. The next start finishes it with the names first chosen.
    #[test]
    fn a_move_cut_short_is_finished_at_the_next_start() {
        let h = Home::new();
        h.old_project("reel", "launch");
        h.old_project("launch", "reel");
        h.old_project("promo", "promo");
        fs::create_dir_all(h.studio.join("promo")).unwrap();
        let plan = vec![("launch", "launch"), ("promo", "promo-2"), ("reel", "reel")];
        fs::write(h.old.join(JOURNAL), serde_json::to_vec(&plan).unwrap()).unwrap();
        fs::rename(h.old.join("reel"), h.studio.join("reel")).unwrap();
        fs::write(h.studio.join("reel/work/.reel-v1.timeline.json.moving"), "half").unwrap();

        let done = h.run();
        assert_eq!(done.moved, vec![("launch".into(), "launch".into()), ("promo".into(), "promo-2".into())]);
        assert!(done.left.is_empty());
        for (project, name) in [("reel", "reel"), ("launch", "launch"), ("promo-2", "promo")] {
            let t = h.timeline(project, name);
            assert!(!t.contains("Media/Projects/reel/") && !t.contains("Media/Projects/launch/") && !t.contains("Media/Projects/promo/"), "{project}: {t}");
        }
        assert!(h.timeline("promo-2", "promo").contains("Media Studio/promo-2/voice/v.mp3"));
        assert!(!h.studio.join("reel/work/.reel-v1.timeline.json.moving").exists(), "a cut-short rewrite's leftover is gone");
        assert!(fs::read_dir(h.studio.join("promo")).unwrap().next().is_none(), "the one already there is untouched");
        assert!(!h.old.exists(), "the plan and the old folder are gone once everything is out");
        assert_eq!(h.run(), MediaProjectsMove::default());
    }

    /// A project that is a link with a relative target still points where it
    /// did from its new place.
    #[cfg(unix)]
    #[test]
    fn a_linked_project_still_points_where_it_did() {
        let h = Home::new();
        fs::create_dir_all(h.home.join("Footage/shoot")).unwrap();
        fs::write(h.home.join("Footage/shoot/project.json"), "{}").unwrap();
        fs::create_dir_all(&h.old).unwrap();
        std::os::unix::fs::symlink("../../../Footage/shoot", h.old.join("shoot")).unwrap();
        assert!(h.old.join("shoot/project.json").exists());

        let done = h.run();
        assert_eq!(done.moved, vec![("shoot".into(), "shoot".into())]);
        assert!(h.studio.join("shoot/project.json").exists(), "the link still reaches the project");
        assert!(!h.old.exists());
    }

    /// No old folder: nothing is made, nothing moved.
    #[test]
    fn without_old_projects_nothing_happens() {
        let h = Home::new();
        assert_eq!(h.run(), MediaProjectsMove::default());
        assert!(!h.studio.exists());
    }
}
