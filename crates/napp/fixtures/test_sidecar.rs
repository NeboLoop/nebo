//! A sidecar for tests, compiled by `napp::test_sidecar` with the standard
//! library only. It behaves like an app sidecar — listens on `127.0.0.1:0`,
//! prints `nebo-app-listening <address>`, exits when Nebo's end of its stdin
//! closes — and does what the files in `NEBO_APP_DIR` tell it:
//!
//! - `fixture-mode`: `serve` (default); `exit <code>` exits at once, before
//!   serving; `deaf` listens and announces, then stops listening and stays
//!   alive — a live process whose address refuses every connection.
//! - `fixture-upstream`: an address each accepted connection is relayed to,
//!   so a test can answer the requests in-process.
//! - `fixture-launches`: appended with this process's pid on every start.
//! - `fixture-token`: written with the `NEBO_APP_TOKEN` this launch was given,
//!   so the in-process server can refuse calls that do not carry it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::{env, fs, thread};

fn main() {
    let dir = PathBuf::from(env::var("NEBO_APP_DIR").expect("NEBO_APP_DIR"));
    let token = env::var("NEBO_APP_TOKEN").expect("NEBO_APP_TOKEN");
    let mode = fs::read_to_string(dir.join("fixture-mode")).unwrap_or_default();
    let mode = mode.trim().to_string();
    let mut launches = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("fixture-launches"))
        .expect("fixture-launches");
    writeln!(launches, "{}", std::process::id()).expect("record launch");
    fs::write(dir.join("fixture-token"), &token).expect("fixture-token");

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

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    println!("nebo-app-listening {addr}");
    std::io::stdout().flush().expect("announce");
    eprintln!("fixture: listening on {addr}");
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

/// Pipe one connection to the upstream address and back. With no upstream
/// the connection is simply accepted and closed (a liveness probe).
fn relay(down: TcpStream, upstream: Option<String>) {
    let Some(addr) = upstream else { return };
    let Ok(up) = TcpStream::connect(addr.trim()) else { return };
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
