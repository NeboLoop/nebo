use super::*;
use crate::registry::ToolResult;

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

fn store(dir: &Path) -> db::Store {
    db::Store::new(&dir.join("t.db").to_string_lossy()).unwrap()
}

/// An own app at `<root>/agents/<name>`, served from its `ui/`.
fn app(store: &db::Store, root: &Path, id: &str, name: &str) -> PathBuf {
    let dir = root.join("agents").join(name);
    write(&dir, "ui/index.html", "<p>working game</p>");
    write(&dir, "src/app.tsx", "export const speed = 1;");
    write(&dir, "AGENT.md", "---\nname: x\n---\n");
    store
        .create_agent(
            id,
            Some("user"),
            name,
            "",
            "---\nname: x\n---\n",
            "{}",
            None,
            None,
        )
        .unwrap();
    store
        .set_agent_app_fields(
            id,
            true,
            Some(&dir.join("ui").to_string_lossy()),
            None,
            None,
        )
        .unwrap();
    dir
}

/// A tool that writes files, the way write_file and run_command do.
struct Writes;

impl DynTool for Writes {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> String {
        String::new()
    }
    fn schema(&self) -> Value {
        serde_json::json!({"type": "object"})
    }
    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }
    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async { ToolResult::ok("") })
    }
}

fn ctx(session: &str, agent: &str) -> ToolContext {
    ToolContext {
        session_id: session.to_string(),
        session_key: format!("agent:{agent}:web"),
        ..ToolContext::default()
    }
}

/// A write in a turn: the app's history starts (the working version is
/// saved before the change), and the turn's end saves the change with the
/// employee's own words.
#[tokio::test]
async fn a_turn_that_writes_ends_with_a_version() {
    if !git_on_bot() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let dir = app(&store, tmp.path(), "flip-1", "Flip-Flap");

    begin_turn("s1", "The game loads slowly, fix it");
    let path = dir.join("src/app.tsx");
    let input = serde_json::json!({ "path": path.to_string_lossy() });
    before_change(&store, &ctx("s1", "flip-1"), &Writes, &input).await;
    write(&dir, "src/app.tsx", "export const speed = 2;");
    end_turn("s1", || {
        "Fixed the slow load. The sprites now load once.".into()
    })
    .await;

    let versions = list(&dir, 10).unwrap();
    assert_eq!(versions.len(), 2, "{versions:?}");
    assert_eq!(versions[0].message, "Fixed the slow load.");
    assert_eq!(versions[0].files, vec!["src/app.tsx".to_string()]);
    assert_eq!(versions[1].message, "Before: The game loads slowly, fix it");
    assert!(dir.join(".git").is_dir());

    // A second change in the same turn does not snapshot again; a turn that
    // changed nothing saves nothing.
    begin_turn("s2", "nothing");
    end_turn("s2", || unreachable!("no app was reached")).await;
    assert_eq!(list(&dir, 10).unwrap().len(), 2);
}

/// The app employee's own shell and file calls reach its folder even when
/// they name a relative path.
#[tokio::test]
async fn the_apps_own_file_calls_reach_its_folder() {
    if !git_on_bot() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    let dir = app(&store, tmp.path(), "flip-2", "Flip");
    let sibling = app(&store, tmp.path(), "flip-3", "Flip-Flap");

    begin_turn("s3", "build");
    before_change(
        &store,
        &ctx("s3", "flip-2"),
        &Writes,
        &serde_json::json!({ "command": "npx vite build" }),
    )
    .await;
    assert!(dir.join(".git").is_dir());
    assert!(!sibling.join(".git").exists(), "another app is untouched");

    // A path inside Flip-Flap names Flip-Flap, never Flip.
    assert!(mentions(
        &format!("cd \"{}\" && ls", sibling.display()),
        &sibling
    ));
    assert!(!mentions(&format!("{}/ui/a.js", sibling.display()), &dir));
    end_turn("s3", String::new).await;
}

/// A restore brings back the old files, removes files the version did not
/// have, keeps the employee's settings, and is itself undoable.
#[test]
fn restore_brings_back_the_old_files_and_can_be_undone() {
    for git in [true, false] {
        if git && !git_on_bot() {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Flip-Flap");
        write(&dir, "ui/index.html", "<p>working game</p>");
        write(&dir, "AGENT.md", "persona v1");
        let good = snapshot_with(&dir, "The working game", git)
            .unwrap()
            .unwrap();

        // The rewrite: a new page, a new file, the persona changed too.
        write(&dir, "ui/index.html", "<p>react is working</p>");
        write(&dir, "ui/rewrite.js", "rewrite()");
        write(&dir, "AGENT.md", "persona v2");
        let broken = snapshot_with(&dir, "Rewrote the game", git)
            .unwrap()
            .unwrap();

        let done = restore_with(&dir, &good.id, git).unwrap();
        assert_eq!(
            read(&dir, "ui/index.html"),
            "<p>working game</p>",
            "git: {git}"
        );
        assert!(!dir.join("ui/rewrite.js").exists(), "git: {git}");
        assert_eq!(
            read(&dir, "AGENT.md"),
            "persona v2",
            "the settings stay (git: {git})"
        );
        assert_eq!(done.to.id, good.id);
        assert_eq!(
            done.undo.as_ref().map(|v| v.id.clone()),
            Some(broken.id.clone())
        );
        let saved = done.saved.expect("a restore is a new version");
        assert!(
            saved
                .message
                .starts_with(&format!("Restored to {}", good.id)),
            "{}",
            saved.message
        );

        // Undo: restore the version before the restore.
        restore_with(&dir, &broken.id, git).unwrap();
        assert_eq!(
            read(&dir, "ui/index.html"),
            "<p>react is working</p>",
            "git: {git}"
        );
        assert_eq!(read(&dir, "ui/rewrite.js"), "rewrite()", "git: {git}");

        // Restoring to how it already is saves nothing new.
        let again = restore_with(&dir, &broken.id, git).unwrap();
        assert!(again.saved.is_none(), "git: {git}");

        assert!(restore_with(&dir, "nope", git).is_err());
    }
}

/// Installed packages and build caches are never in the history.
#[test]
fn node_modules_and_build_caches_are_ignored() {
    for git in [true, false] {
        if git && !git_on_bot() {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Orbit");
        write(&dir, "src/main.tsx", "main");
        write(&dir, "node_modules/react/index.js", "react");
        write(&dir, "dist/out.js", "built");
        write(&dir, ".vite/deps.json", "{}");
        let first = snapshot_with(&dir, "first", git).unwrap().unwrap();
        assert!(
            first.files.iter().any(|f| f == "src/main.tsx"),
            "{:?}",
            first.files
        );
        assert!(
            first.files.iter().all(|f| !f.starts_with("node_modules")
                && !f.starts_with("dist")
                && !f.starts_with(".vite")),
            "git: {git} {:?}",
            first.files
        );
        // A change only under node_modules is no new version.
        write(&dir, "node_modules/react/index.js", "react 2");
        assert!(
            snapshot_with(&dir, "second", git).unwrap().is_none(),
            "git: {git}"
        );
        if git {
            assert!(read(&dir, ".gitignore").contains("node_modules/"));
        }
    }
}

/// Without git, versions are copies under `.nebo-history/`, and only a
/// change makes a new one.
#[test]
fn without_git_versions_are_copies() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Tweet");
    write(&dir, "ui/index.html", "one");
    write(&dir, "ui/assets/hero.webp", "art");
    assert_eq!(keeper(&dir, false), Keeper::Copies);
    let v1 = snapshot_with(&dir, "first", false).unwrap().unwrap();
    assert_eq!(v1.id, "1");
    assert!(snapshot_with(&dir, "same", false).unwrap().is_none());
    write(&dir, "ui/index.html", "two");
    let v2 = snapshot_with(&dir, "second", false).unwrap().unwrap();
    assert_eq!(v2.files, vec!["ui/index.html".to_string()]);
    assert!(
        dir.join(HISTORY_DIR)
            .join("2")
            .join("files/ui/assets/hero.webp")
            .is_file()
    );
    assert!(!dir.join(".git").exists());
    let listed = copies_list(&dir, 10).unwrap();
    assert_eq!(
        listed.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        ["2", "1"]
    );

    // Once a folder keeps copies it keeps them, even when git turns up.
    assert_eq!(keeper(&dir, true), Keeper::Copies);
}

/// The history lives in the folder, so it moves with a rename.
#[test]
fn the_history_moves_with_the_folder() {
    for git in [true, false] {
        if git && !git_on_bot() {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Tweet");
        write(&dir, "ui/index.html", "one");
        let v1 = snapshot_with(&dir, "first", git).unwrap().unwrap();
        let moved = tmp.path().join("Flip-Flap");
        std::fs::rename(&dir, &moved).unwrap();
        write(&moved, "ui/index.html", "two");
        snapshot_with(&moved, "second", git).unwrap().unwrap();
        restore_with(&moved, &v1.id, git).unwrap();
        assert_eq!(read(&moved, "ui/index.html"), "one", "git: {git}");
    }
}

#[test]
fn a_message_is_one_short_line() {
    assert_eq!(
        one_line("**Fixed** the `loader`. Then more.\nsecond line"),
        "Fixed the loader."
    );
    assert_eq!(one_line("\n\n## Done\n"), "Done");
    let long = "word ".repeat(40);
    assert!(one_line(&long).chars().count() <= SUBJECT_CHARS);
}
