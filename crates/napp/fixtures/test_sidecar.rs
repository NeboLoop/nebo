//! A sidecar for tests, compiled by `napp::test_sidecar` with the standard
//! library only. It behaves like an app sidecar — serves `NEBO_APP_SOCK`,
//! exits when Nebo's end of its stdin closes — and does what the files in
//! `NEBO_APP_DIR` tell it:
//!
//! - `fixture-mode`: `serve` (default); `exit <code>` exits at once, before
//!   serving; `deaf` binds the socket, then stops listening and stays alive —
//!   a live process behind a socket file that refuses every connection.
//! - `fixture-upstream`: a socket path each accepted connection is relayed to,
//!   so a test can answer the requests in-process.
//! - `fixture-launches`: appended with this process's pid on every start.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::{env, fs, thread};

fn main() {
    let sock = PathBuf::from(env::var("NEBO_APP_SOCK").expect("NEBO_APP_SOCK"));
    let dir = PathBuf::from(env::var("NEBO_APP_DIR").expect("NEBO_APP_DIR"));
    let mode = fs::read_to_string(dir.join("fixture-mode")).unwrap_or_default();
    let mode = mode.trim().to_string();
    let mut launches = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("fixture-launches"))
        .expect("fixture-launches");
    writeln!(launches, "{}", std::process::id()).expect("record launch");

    // Nebo holds the write end of our stdin: EOF means Nebo is gone.
    thread::spawn(|| {
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        std::process::exit(0);
    });

    if let Some(code) = mode.strip_prefix("exit ") {
        eprintln!("fixture: exiting with {} as told", code.trim());
        std::process::exit(code.trim().parse().unwrap_or(1));
    }

    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind");
    eprintln!("fixture: listening on {}", sock.display());
    if mode == "deaf" {
        drop(listener);
        eprintln!("fixture: stopped listening, staying alive");
        loop {
            thread::park();
        }
    }
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let upstream = fs::read_to_string(dir.join("fixture-upstream")).ok();
        thread::spawn(move || relay(conn, upstream));
    }
}

/// Pipe one connection to the upstream socket and back. With no upstream the
/// connection is simply accepted and closed (a liveness probe).
fn relay(down: UnixStream, upstream: Option<String>) {
    let Some(path) = upstream else { return };
    let Ok(up) = UnixStream::connect(path.trim()) else { return };
    let (mut down_r, mut up_w) = (down.try_clone().expect("clone"), up.try_clone().expect("clone"));
    let forward = thread::spawn(move || {
        let _ = std::io::copy(&mut down_r, &mut up_w);
        let _ = up_w.shutdown(std::net::Shutdown::Write);
    });
    let (mut down_w, mut up_r) = (down, up);
    let _ = std::io::copy(&mut up_r, &mut down_w);
    let _ = down_w.shutdown(std::net::Shutdown::Write);
    let _ = forward.join();
}
