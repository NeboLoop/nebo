//! A real sidecar process for supervision tests: `fixtures/test_sidecar.rs`,
//! compiled once per test process with `rustc` (a sidecar must be a native
//! binary; scripts are refused at launch), installed into a throwaway app
//! directory under a throwaway Nebo root. Nothing here touches the real data
//! directory.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SOURCE: &str = include_str!("../fixtures/test_sidecar.rs");

/// The compiled fixture, built on first use.
pub fn binary() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("nebo-test-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture build dir");
        let src = dir.join("test_sidecar.rs");
        std::fs::write(&src, SOURCE).expect("write fixture source");
        let out = dir.join("test-sidecar");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let status = std::process::Command::new(rustc)
            .args(["--edition", "2021", "-O", "-o"])
            .arg(&out)
            .arg(&src)
            .status()
            .expect("run rustc for the test sidecar");
        assert!(status.success(), "the test sidecar did not compile");
        out
    })
}

/// One app with the fixture as its sidecar, in its own temp tree:
/// `<root>/home` is the Nebo root, `<root>/agents/<id>` the app directory.
pub struct TestApp {
    _root: tempfile::TempDir,
    pub id: String,
    pub home: PathBuf,
    pub tool_dir: PathBuf,
}

impl TestApp {
    pub fn new(id: &str) -> Self {
        let root = tempfile::tempdir().expect("temp root");
        let home = root.path().join("home");
        let tool_dir = root.path().join("agents").join(id);
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(tool_dir.join("bin")).expect("app dir");
        std::fs::write(
            tool_dir.join("manifest.json"),
            serde_json::json!({ "id": id, "name": id, "version": "1.0.0", "type": "app" }).to_string(),
        )
        .expect("manifest");
        let app = Self { _root: root, id: id.to_string(), home, tool_dir };
        app.install_binary();
        app
    }

    fn binary_path(&self) -> PathBuf {
        self.tool_dir.join("bin").join(&self.id)
    }

    /// Put (or put back) the program, stamped now — as a rebuild would be.
    pub fn install_binary(&self) {
        let path = self.binary_path();
        let _ = std::fs::remove_file(&path);
        std::fs::copy(binary(), &path).expect("install the test sidecar");
        // A copy can keep the source's mtime (clonefile on macOS); a rebuild does not.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()))
            .expect("stamp the new binary");
    }

    pub fn remove_binary(&self) {
        std::fs::remove_file(self.binary_path()).expect("remove the test sidecar");
    }

    /// `serve`, `exit <code>` or `deaf` — read at each launch.
    pub fn set_mode(&self, mode: &str) {
        std::fs::write(self.tool_dir.join("fixture-mode"), mode).expect("fixture-mode");
    }

    /// Relay every accepted connection to this socket.
    pub fn set_upstream(&self, sock: &Path) {
        std::fs::write(self.tool_dir.join("fixture-upstream"), sock.to_string_lossy().as_bytes())
            .expect("fixture-upstream");
    }

    /// How many times the program has started.
    pub fn launches(&self) -> usize {
        std::fs::read_to_string(self.tool_dir.join("fixture-launches"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    pub fn sock_path(&self) -> PathBuf {
        self.tool_dir.join(format!("{}.sock", self.id))
    }
}

/// SIGKILL a process, the way an OOM kill or a stray `kill -9` would.
pub fn kill(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

/// Whether a pid names a process (a zombie counts).
pub fn exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
