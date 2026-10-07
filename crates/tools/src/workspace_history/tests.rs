use super::*;

/// A throwaway Nebo root: its database and its workspace (`files`).
struct Home {
    _dir: tempfile::TempDir,
    store: db::Store,
    files: PathBuf,
}

fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    Home { _dir: dir, store, files }
}

impl Home {
    fn write(&self, rel: &str, text: &str) {
        let path = self.files.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.files.join(rel)).unwrap_or_default()
    }

    fn look(&self, chat: &str) -> Look {
        look(&self.store, &self.files, Some(chat)).unwrap()
    }

    fn history(&self, rel: &str) -> Vec<FileHistoryEntry> {
        history(&self.store, rel).unwrap()
    }

    /// The bytes an entry kept.
    fn kept(&self, e: &FileHistoryEntry) -> String {
        std::fs::read_to_string(self.files.join(blob_rel(&e.hash, &e.ext))).unwrap()
    }

    fn sh(&self, script: &str) {
        let status = std::process::Command::new("sh").arg("-c").arg(script).current_dir(&self.files).status().unwrap();
        assert!(status.success(), "{script}");
    }
}

#[test]
fn a_shell_overwrite_keeps_what_the_file_was() {
    let h = home();
    h.write("growth-model.xlsx", "nine sheets");
    h.look("c1");
    h.sh("echo partial > growth-model.xlsx");
    let look = h.look("c1");
    assert_eq!(look.kept, 1);
    let entries = h.history("growth-model.xlsx");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, "modified");
    assert_eq!(entries[0].chat_id.as_deref(), Some("c1"));
    assert_eq!(h.kept(&entries[0]), "nine sheets");
    assert_eq!(blob_url(&entries[0].hash, "xlsx"), format!("/api/v1/files/work/blobs/{}.xlsx", entries[0].hash));
}

#[test]
fn a_write_through_the_file_tool_keeps_what_the_file_was() {
    let h = home();
    let path = h.files.join("notes.md");
    h.write("notes.md", "# first");
    h.look("c1");
    let tool = crate::file_tool::FileTool::new();
    let ctx = crate::origin::ToolContext::default();
    let read = tool.execute(&ctx, serde_json::json!({"action": "read", "path": path}));
    assert!(!read.is_error, "{}", read.content);
    let wrote = tool.execute(&ctx, serde_json::json!({"action": "write", "path": path, "content": "# second"}));
    assert!(!wrote.is_error, "{}", wrote.content);
    assert_eq!(h.read("notes.md"), "# second");
    h.look("c1");
    let entries = h.history("notes.md");
    assert_eq!(entries.len(), 1);
    assert_eq!(h.kept(&entries[0]), "# first");
}

#[test]
fn a_shell_delete_keeps_what_the_file_was() {
    let h = home();
    h.write("uploads/contract.pdf", "signed");
    h.look("c1");
    h.sh("rm uploads/contract.pdf");
    h.look("c1");
    let entries = h.history("uploads/contract.pdf");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].reason, "deleted");
    assert_eq!(h.kept(&entries[0]), "signed");
    assert!(h.store.workspace_index().unwrap().is_empty());
}

#[test]
fn unchanged_files_and_the_same_content_rewritten_keep_nothing() {
    let h = home();
    h.write("a.md", "alpha");
    h.write("sub/b.csv", "1,2");
    let first = h.look("c1");
    assert_eq!((first.files, first.hashed, first.kept), (2, 2, 0));
    // Nothing moved: nothing is hashed again.
    let warm = h.look("c1");
    assert_eq!((warm.files, warm.hashed, warm.kept), (2, 0, 0));
    // The same bytes written again: hashed (its mtime moved), nothing kept.
    std::thread::sleep(std::time::Duration::from_millis(5));
    h.sh("printf alpha > a.md");
    let again = h.look("c1");
    assert_eq!((again.hashed, again.kept), (1, 0));
    assert!(h.history("a.md").is_empty());
    assert!(h.history("sub/b.csv").is_empty());
}

#[test]
fn a_file_made_and_overwritten_within_one_turn_keeps_its_first_content() {
    let h = home();
    h.look("c1"); // the turn starts
    h.write("report.md", "draft one"); // round one makes it
    h.look("c1"); // before round two
    h.sh("echo draft two > report.md"); // round two overwrites it
    h.look("c1"); // the turn ends
    let entries = h.history("report.md");
    assert_eq!(entries.len(), 1);
    assert_eq!(h.kept(&entries[0]), "draft one");
}

#[test]
fn nebos_own_folders_hidden_files_links_and_build_folders_are_left_out() {
    let h = home();
    h.write("work/blobs/x.md", "a blob");
    h.write(".shared/abc/clip.gif", "shared");
    h.write(".DS_Store", "finder");
    h.write("app/node_modules/pkg/index.js", "dep");
    h.write("app/target/debug/out", "build");
    h.write("app/src/main.rs", "fn main() {}");
    #[cfg(unix)]
    std::os::unix::fs::symlink(h.files.join("app/src/main.rs"), h.files.join("link.rs")).unwrap();
    let look = h.look("c1");
    let paths: Vec<String> = h.store.workspace_index().unwrap().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, vec!["app/src/main.rs".to_string()]);
    assert_eq!(look.files, 1);
}

#[test]
fn a_file_over_the_ceiling_is_left_out() {
    let h = home();
    let big = std::fs::File::create(h.files.join("video.mov")).unwrap();
    big.set_len(MAX_FILE_BYTES + 1).unwrap();
    let look = h.look("c1");
    assert_eq!(look.files, 0);
    assert!(h.store.workspace_index().unwrap().is_empty());
    assert!(!h.files.join(BLOBS_DIR).exists() || std::fs::read_dir(h.files.join(BLOBS_DIR)).unwrap().next().is_none());
    // A kept file that grows past it keeps what it was.
    h.write("log.txt", "small");
    h.look("c1");
    std::fs::OpenOptions::new().write(true).open(h.files.join("log.txt")).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
    h.look("c1");
    let entries = h.history("log.txt");
    assert_eq!(entries.len(), 1);
    assert_eq!(h.kept(&entries[0]), "small");
}

#[test]
fn two_turns_looking_at_once_record_each_change_once() {
    let h = std::sync::Arc::new(home());
    for i in 0..20 {
        h.write(&format!("doc-{i}.md"), &format!("v1 {i}"));
    }
    h.look("c0");
    for i in 0..20 {
        h.write(&format!("doc-{i}.md"), &format!("v2 {i} longer"));
    }
    let turns: Vec<_> = ["c1", "c2"]
        .into_iter()
        .map(|chat| {
            let h = h.clone();
            std::thread::spawn(move || h.look(chat).kept)
        })
        .collect();
    let kept: usize = turns.into_iter().map(|t| t.join().unwrap()).sum();
    assert_eq!(kept, 20, "each change is recorded by the one look that saw it first");
    for i in 0..20 {
        let entries = h.history(&format!("doc-{i}.md"));
        assert_eq!(entries.len(), 1);
        assert_eq!(h.kept(&entries[0]), format!("v1 {i}"));
    }
}

#[test]
fn a_restore_keeps_the_current_content_first_and_can_be_undone() {
    let h = home();
    h.write("model.xlsx", "36 KB nine sheets");
    h.look("c1");
    h.sh("echo partial > model.xlsx");
    h.look("c1");
    let before = h.history("model.xlsx")[0].clone();

    let restored = restore(&h.store, &h.files, before.id, Some("c1")).unwrap();
    assert_eq!(h.read("model.xlsx"), "36 KB nine sheets");
    let saved = restored.saved.expect("the content restored over is kept");
    assert_eq!(h.kept(&saved), "partial\n");
    assert!(restored.work.is_none());
    // The restore is not taken for a change by the next look.
    assert_eq!(h.look("c1").kept, 0);

    // Restoring the content it replaced undoes it.
    restore(&h.store, &h.files, saved.id, None).unwrap();
    assert_eq!(h.read("model.xlsx"), "partial\n");
    assert_eq!(h.history("model.xlsx").len(), 2);
}

#[test]
fn a_deleted_file_is_restored_and_a_work_document_gets_the_version() {
    let h = home();
    h.store.create_chat("c1", "Growth").unwrap();
    let doc = h.store.upsert_work_document("c1", "plan.md", "document").unwrap();
    h.store.add_work_version(&doc.id, None, "/api/v1/files/work/blobs/old.md", Some("old"), None, None).unwrap();
    h.write("plan.md", "the plan");
    h.look("c1");
    h.sh("rm plan.md");
    h.look("c1");
    let gone = h.history("plan.md")[0].clone();
    let restored = restore(&h.store, &h.files, gone.id, None).unwrap();
    assert_eq!(h.read("plan.md"), "the plan");
    assert!(restored.saved.is_none(), "nothing was there to keep");
    let (d, v) = restored.work.expect("the work document gets the restored content");
    assert_eq!(d.id, doc.id);
    assert_eq!(v.version_number, 2);
    assert_eq!(v.url, blob_url(&gone.hash, "md"));
}

#[test]
fn workspace_paths_stay_inside_the_workspace() {
    let files = Path::new("/data/files");
    assert_eq!(workspace_path(files, "/data/files/a/b.md").as_deref(), Some("a/b.md"));
    assert_eq!(workspace_path(files, "a/./b.md").as_deref(), Some("a/b.md"));
    assert_eq!(workspace_path(files, "../settings.json"), None);
    assert_eq!(workspace_path(files, "/etc/passwd"), None);
    assert_eq!(workspace_path(files, "/data/files"), None);
}

/// "Restore the previous version of the model": the employee lists the
/// file's earlier versions with list_checkpoints(path) and puts one back with
/// restore_checkpoint(fh-…).
#[test]
fn the_employee_lists_and_restores_a_files_earlier_version() {
    use crate::registry::DynTool;
    // The env lock is held for the whole test, so its futures run here.
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    // SAFETY: serialized by the crate-wide lock; the workspace is NEBO_HOME/files.
    unsafe { std::env::set_var("NEBO_HOME", root.path()) };
    let files = root.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let store = std::sync::Arc::new(db::Store::new(&root.path().join("t.db").to_string_lossy()).unwrap());
    std::fs::write(files.join("model.xlsx"), "nine sheets").unwrap();
    look(&store, &files, Some("c1")).unwrap();
    std::fs::write(files.join("model.xlsx"), "partial").unwrap();
    look(&store, &files, Some("c1")).unwrap();

    let machine = std::sync::Arc::new(
        crate::file_tools::Machine::new(std::sync::Arc::new(crate::process::ProcessRegistry::new()), None).with_store(Some(store.clone())),
    );
    let ctx = crate::origin::ToolContext::default();
    let path = files.join("model.xlsx").to_string_lossy().into_owned();
    let list = crate::file_tools::ListCheckpointsTool(machine.clone());
    let listed = rt.block_on(list.execute_dyn(&ctx, serde_json::json!({"path": path})));
    assert!(!listed.is_error, "{}", listed.content);
    let id = listed.content.split_whitespace().find(|w| w.starts_with(ID_PREFIX)).unwrap().to_string();
    assert!(listed.content.contains("before it changed"), "{}", listed.content);

    let restore = crate::file_tools::RestoreCheckpointTool(machine.clone());
    let restored = rt.block_on(restore.execute_dyn(&ctx, serde_json::json!({"checkpoint": id})));
    assert!(!restored.is_error, "{}", restored.content);
    assert_eq!(std::fs::read_to_string(files.join("model.xlsx")).unwrap(), "nine sheets");
    assert!(restored.content.contains("restore that to undo this"), "{}", restored.content);

    let outside = rt.block_on(list.execute_dyn(&ctx, serde_json::json!({"path": "/etc/hosts"})));
    assert!(outside.is_error);
    unsafe { std::env::remove_var("NEBO_HOME") };
}

/// What a look costs a turn: a 2,000-file, 2 GB workspace looked at the first
/// time (every file hashed and kept) and warm (nothing changed, then ten
/// files changed). `cargo test --release -p nebo-tools measure_a_look -- --ignored --nocapture`
#[test]
#[ignore = "writes 2 GB; run by hand to measure"]
fn measure_a_look_at_a_2gb_workspace() {
    use std::io::Write;
    let h = home();
    let mut block = vec![0u8; 1024 * 1024];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..2000 {
        for b in block.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b.copy_from_slice(&x.to_le_bytes()[..b.len()]);
        }
        let dir = h.files.join(format!("folder-{}", i % 40));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::File::create(dir.join(format!("file-{i}.bin"))).unwrap().write_all(&block).unwrap();
    }
    let timed = |label: &str| {
        let t = std::time::Instant::now();
        let look = h.look("bench");
        println!("{label}: {:?} ({look:?})", t.elapsed());
    };
    timed("first look");
    timed("warm look");
    timed("warm look");
    for i in 0..10 {
        std::fs::write(h.files.join(format!("folder-{}/file-{i}.bin", i % 40)), format!("changed {i}")).unwrap();
    }
    timed("ten files changed");
}
