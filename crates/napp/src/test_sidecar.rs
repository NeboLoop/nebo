//! A real sidecar process for supervision tests: `fixtures/test_sidecar.rs`,
//! compiled once per test process with `rustc` (a sidecar must be a native
//! binary; scripts are refused at launch), installed into a throwaway app
//! directory under a throwaway Nebo root. Nothing here touches the real data
//! directory.
//!
//! The fixture relays every connection to a stand-in sidecar server each
//! [`TestApp`] runs in this process ([`serve_as_sidecar`]): it answers
//! `UIService` the way the SDKs do, refusing any call that does not carry the
//! launch's `NEBO_SIDECAR_TOKEN` and answering the health challenge with the
//! proof ([`crate::runtime::proof`]) — or, told to be an imposter, with the
//! proof a process that only saw the sidecar token could make.

use std::net::SocketAddr;
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
        let status = command::new::<std::process::Command>(rustc, command::Console::Hidden)
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
/// `<root>/home` is the Nebo root, `<root>/agents/<id>` the app directory
/// (or a deeper one, [`TestApp::at_depth`]).
pub struct TestApp {
    _root: tempfile::TempDir,
    pub id: String,
    pub home: PathBuf,
    pub tool_dir: PathBuf,
}

impl TestApp {
    pub fn new(id: &str) -> Self {
        Self::at_depth(id, Path::new("agents"))
    }

    /// An app whose folder is `<root>/<parent>/<id>`: a long `parent` gives
    /// the app a folder path as long as a real one can be.
    pub fn at_depth(id: &str, parent: &Path) -> Self {
        let root = tempfile::tempdir().expect("temp root");
        let home = root.path().join("home");
        let tool_dir = root.path().join(parent).join(id);
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(tool_dir.join("bin")).expect("app dir");
        std::fs::write(
            tool_dir.join("manifest.json"),
            serde_json::json!({ "id": id, "name": id, "version": "1.0.0", "type": "app" }).to_string(),
        )
        .expect("manifest");
        let app = Self { _root: root, id: id.to_string(), home, tool_dir };
        app.install_binary();
        let upstream = serve_as_sidecar(app.tool_dir.clone());
        std::fs::write(app.tool_dir.join("fixture-upstream"), upstream.to_string()).expect("fixture-upstream");
        app
    }

    fn binary_path(&self) -> PathBuf {
        self.tool_dir.join("bin").join(&self.id)
    }

    /// Put (or put back) the program, stamped now — as a rebuild would be.
    pub fn install_binary(&self) {
        let path = self.binary_path();
        // Windows can't delete or overwrite a program that is running, but it
        // can rename it: move the running one aside first, as a rebuild of a
        // running program there has to.
        #[cfg(windows)]
        if path.exists() {
            let aside = path.with_extension(format!("old-{}", uuid::Uuid::new_v4().simple()));
            std::fs::rename(&path, aside).expect("move the running test sidecar aside");
        }
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

    /// Answer the health challenge as an imposter would: something that took
    /// the sidecar's place and saw the sidecar token, but never the app token.
    pub fn set_imposter(&self, imposter: bool) {
        let flag = self.tool_dir.join("fixture-imposter");
        if imposter {
            std::fs::write(flag, "").expect("fixture-imposter");
        } else {
            let _ = std::fs::remove_file(flag);
        }
    }

    /// How many times the program has started.
    pub fn launches(&self) -> usize {
        std::fs::read_to_string(self.tool_dir.join("fixture-launches"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
}

/// Kill a process outright, the way an OOM kill or a stray `kill -9` (on
/// Windows, Task Manager's End task) would.
pub fn kill(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Whether a pid names a process that has not exited (on Unix a zombie
/// counts).
pub fn exists(pid: u32) -> bool {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, 0) == 0
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        running
    }
}

/// A stand-in sidecar server on a loopback address of its own, run on its own
/// thread for the life of the test process: answers `HandleRequest` with
/// "<method> <path>" and `HealthCheck` as healthy, and refuses every call
/// whose bearer token is not the `NEBO_SIDECAR_TOKEN` the sidecar was launched
/// with. Reads what the fixture writes into `dir` on each launch.
fn serve_as_sidecar(dir: PathBuf) -> SocketAddr {
    use proto::ui_service_server::{UiService, UiServiceServer};

    struct Echo {
        dir: PathBuf,
    }

    impl Echo {
        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
        }

        fn check<T>(&self, req: &tonic::Request<T>) -> Result<(), tonic::Status> {
            let token = self.read("fixture-token");
            let bearer = req.metadata().get("authorization").and_then(|v| v.to_str().ok());
            if token.is_empty() || bearer != Some(format!("Bearer {token}").as_str()) {
                return Err(tonic::Status::unauthenticated("no valid sidecar token"));
            }
            Ok(())
        }
    }

    #[tonic::async_trait]
    impl UiService for Echo {
        async fn health_check(
            &self,
            req: tonic::Request<proto::HealthCheckRequest>,
        ) -> Result<tonic::Response<proto::HealthCheckResponse>, tonic::Status> {
            self.check(&req)?;
            let sidecar_token = self.read("fixture-token");
            // An imposter never saw the app token; the best it can do is key
            // the proof with what it did see.
            let key = if self.dir.join("fixture-imposter").exists() { sidecar_token.clone() } else { self.read("fixture-app-token") };
            let port: u16 = self.read("fixture-port").trim().parse().unwrap_or(0);
            let nonce = req
                .metadata()
                .get("x-nebo-challenge")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| hex::decode(v).ok())
                .unwrap_or_default();
            let mut answer = tonic::Response::new(proto::HealthCheckResponse { healthy: true, ..Default::default() });
            let proof = hex::encode(crate::runtime::proof(&key, &nonce, &sidecar_token, port));
            answer.metadata_mut().insert("x-nebo-proof", proof.parse().expect("hex is a header value"));
            Ok(answer)
        }
        async fn configure(
            &self,
            req: tonic::Request<proto::SettingsMap>,
        ) -> Result<tonic::Response<proto::Empty>, tonic::Status> {
            self.check(&req)?;
            Ok(tonic::Response::new(proto::Empty {}))
        }
        async fn handle_request(
            &self,
            req: tonic::Request<proto::HttpRequest>,
        ) -> Result<tonic::Response<proto::HttpResponse>, tonic::Status> {
            self.check(&req)?;
            let r = req.into_inner();
            Ok(tonic::Response::new(proto::HttpResponse {
                status_code: 200,
                headers: Default::default(),
                body: format!("{} {}", r.method, r.path).into_bytes(),
            }))
        }
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the stand-in sidecar server");
    let addr = listener.local_addr().expect("its address");
    listener.set_nonblocking(true).expect("nonblocking");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            let incoming = futures::stream::unfold(listener, |l| async move { Some((l.accept().await.map(|(s, _)| s), l)) });
            let _ = tonic::transport::Server::builder()
                .add_service(UiServiceServer::new(Echo { dir }))
                .serve_with_incoming(incoming)
                .await;
        });
    });
    addr
}
