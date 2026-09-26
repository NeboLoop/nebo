//! What a command an employee runs may reach, enforced by the operating
//! system around the command itself: Nebo's own files (`nebo_files`) can't
//! be read or changed, and a run whose web access is off can't reach the
//! network. Everything else the command does runs as it would anyway.
//!
//! A limit on the command's words can be walked around (`curl`, then
//! `python -c "urllib…"`, then `nc`); a limit on the process can't, whatever
//! the command is. So the shell's one command (`ShellTool::command`) starts
//! under it:
//!
//! - macOS: `sandbox-exec` with a profile that allows everything but those
//!   paths and, offline, every IP socket (the network and this computer's
//!   own servers alike).
//! - Linux: `bwrap` over the whole filesystem, those paths covered, and
//!   offline in a network namespace of its own with nothing in it.
//!
//! The skill-script sandbox (`sandbox_policy`) is a different confinement:
//! deny-by-default for marketplace code, network only through its filtering
//! proxy. An employee's shell does the owner's work with the owner's tools,
//! so it keeps everything but what this module takes away.
//!
//! Where neither is available (Windows, a Linux without user namespaces), a
//! run that must be offline can't run commands at all: `prefix` says so and
//! the shell refuses. Nebo's own files then rest on the safeguard's check of
//! the command's text.
//!
//! What it does not stop: a name lookup still resolves (through the
//! system's resolver, outside the command), and a program the command asks
//! the system to open (`open <url>`, a script driving another app) runs
//! outside it. A program that starts a sandbox of its own (Chrome without
//! `--no-sandbox`, `swift build` without `--disable-sandbox`) can't inside
//! macOS's.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::nebo_files::NeboFiles;

/// The confinement one command runs under.
pub struct Confinement<'a> {
    /// The command may not reach the network: the run's web access is off.
    pub offline: bool,
    /// Nebo's own files, closed to the command. `None`: not fenced (a
    /// workflow's command step, which runs installed plugins that keep their
    /// data in Nebo's folder).
    pub fence: Option<&'a NeboFiles>,
}

/// This computer can't run a command offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unconfinable;

impl Confinement<'_> {
    /// The program and arguments that go before the shell to run it
    /// confined; empty when there is nothing to confine or nothing to do it
    /// with. `Err` only for an offline run on a computer that can't keep a
    /// command off the network: such a command must not run.
    pub fn prefix(&self) -> Result<Vec<OsString>, Unconfinable> {
        if !self.offline && self.fence.is_none() {
            return Ok(Vec::new());
        }
        match platform() {
            Some(Platform::Seatbelt) => Ok(vec![
                SANDBOX_EXEC.into(),
                "-p".into(),
                profile(self.offline, self.fence).into(),
            ]),
            Some(Platform::Bubblewrap) => Ok(bwrap_args(self.offline, self.fence)),
            None if self.offline => Err(Unconfinable),
            None => Ok(Vec::new()),
        }
    }
}

/// Whether this computer confines commands at all.
pub fn available() -> bool {
    platform().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Seatbelt,
    Bubblewrap,
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// What this computer confines commands with, probed once by confining a
/// command that does nothing. A Nebo that itself runs inside a sandbox, or
/// a Linux with user namespaces turned off, has none.
fn platform() -> Option<Platform> {
    static PLATFORM: OnceLock<Option<Platform>> = OnceLock::new();
    *PLATFORM.get_or_init(|| {
        let works = |program: &str, args: &[&str]| {
            std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let found = if cfg!(target_os = "macos") {
            let probe = "(version 1)(allow default)(deny file-read* (subpath \"/nonexistent-nebo-probe\"))";
            works(SANDBOX_EXEC, &["-p", probe, "/usr/bin/true"]).then_some(Platform::Seatbelt)
        } else if cfg!(target_os = "linux") {
            works("bwrap", &["--dev-bind", "/", "/", "--unshare-net", "--", "true"]).then_some(Platform::Bubblewrap)
        } else {
            None
        };
        if found.is_none() {
            tracing::warn!(
                "commands can't be confined on this computer: an employee whose web access is off can't run \
                 commands, and Nebo's own files are fenced by the command's text alone"
            );
        }
        found
    })
}

/// The Seatbelt profile: everything allowed, but for what this run may not
/// reach.
fn profile(offline: bool, fence: Option<&NeboFiles>) -> String {
    let mut p = vec!["(version 1)".to_string(), "(allow default)".to_string()];
    if let Some(fence) = fence {
        let root = fence.real_root();
        p.push(format!("(deny file-read* file-write* (subpath {}))", quoted(root)));
        let open: Vec<String> = fence.real_open().iter().map(|o| format!("(subpath {})", quoted(o))).collect();
        p.push(format!("(allow file-read* file-write* {})", open.join(" ")));
        // Moving a folder above it would carry the closed paths out from
        // under the fence.
        let ancestors: Vec<String> = root.ancestors().skip(1).map(|a| format!("(literal {})", quoted(a))).collect();
        if !ancestors.is_empty() {
            p.push(format!("(deny file-write-unlink {})", ancestors.join(" ")));
        }
    }
    if offline {
        p.extend([
            "(deny network-outbound (remote ip))".to_string(),
            "(deny network-inbound (local ip))".to_string(),
            "(deny network-bind (local ip))".to_string(),
        ]);
    }
    p.join("\n")
}

/// A path as a Seatbelt string.
fn quoted(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap_or_default()
}

/// The `bwrap` arguments: the whole filesystem as it is, each closed entry
/// of Nebo's folder covered by an empty read-only one, and offline a network
/// namespace with nothing in it.
fn bwrap_args(offline: bool, fence: Option<&NeboFiles>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["bwrap", "--dev-bind", "/", "/", "--die-with-parent"].map(OsString::from).into();
    if offline {
        args.push("--unshare-net".into());
    }
    let closed: Vec<PathBuf> = fence.map(NeboFiles::closed_entries).unwrap_or_default();
    for path in closed {
        if path.is_dir() {
            args.extend([OsString::from("--tmpfs"), path.clone().into(), "--remount-ro".into(), path.into()]);
        } else {
            args.extend([OsString::from("--ro-bind"), "/dev/null".into(), path.into()]);
        }
    }
    args.push("--".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("nebo-home");
        for d in ["files", "logs", "sessions/s1", "sessions/s2"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("settings.json"), "{\"accessSecret\": \"SECRET\"}").unwrap();
        std::fs::write(root.join("logs/nebo.log"), "LOGLINE").unwrap();
        std::fs::write(root.join("files/a.md"), "MINE").unwrap();
        std::fs::write(root.join("sessions/s1/r.txt"), "SAVED").unwrap();
        std::fs::write(root.join("sessions/s2/r.txt"), "THEIRS").unwrap();
        (dir, root)
    }

    /// Run `command` in bash under `c`, returning (exit ok, stdout+stderr).
    fn run(c: &Confinement<'_>, command: &str) -> (bool, String) {
        let prefix = c.prefix().expect("confinable");
        let mut cmd = match prefix.split_first() {
            Some((program, args)) => {
                let mut cmd = std::process::Command::new(program);
                cmd.args(args).arg("bash");
                cmd
            }
            None => std::process::Command::new("bash"),
        };
        let out = cmd.arg("-c").arg(command).output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// Nothing to confine: the shell runs as it is.
    #[test]
    fn nothing_to_confine_adds_nothing() {
        assert!(Confinement { offline: false, fence: None }.prefix().unwrap().is_empty());
    }

    /// The command can't read or change Nebo's own files however it names
    /// them, and works in the open parts as before.
    #[test]
    fn a_confined_command_cannot_reach_nebo_own_files() {
        if platform().is_none() {
            eprintln!("no confinement on this computer; the fence rests on the safeguard");
            return;
        }
        let (_d, root) = home();
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"));
        let c = Confinement { offline: false, fence: Some(&fence) };
        let r = root.to_string_lossy();
        let (_, out) = run(&c, &format!("cat {r}/settings.json; cat {r}/logs/nebo.log; cd {r}/files && cat ../settings.json; cat {r}/sessions/s2/r.txt; grep -r SECRET {r}"));
        assert!(!out.contains("SECRET") && !out.contains("LOGLINE") && !out.contains("THEIRS"), "{out}");
        let (ok, out) = run(&c, &format!("cat {r}/files/a.md {r}/sessions/s1/r.txt && echo NEW > {r}/files/b.md && cat {r}/files/b.md"));
        assert!(ok && out.contains("MINE") && out.contains("SAVED") && out.contains("NEW"), "{out}");
        let (ok, _) = run(&c, &format!("echo x > {r}/settings.json"));
        assert!(!ok, "a closed file was overwritten");
        assert!(std::fs::read_to_string(root.join("settings.json")).unwrap().contains("SECRET"));
    }

    /// Offline, nothing the command starts reaches a socket: not this
    /// computer's own server, and so not the network.
    #[test]
    fn an_offline_command_reaches_no_server() {
        if platform().is_none() {
            assert_eq!(Confinement { offline: true, fence: None }.prefix(), Err(Unconfinable));
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let probe = format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo CONNECTED");
        let (_, out) = run(&Confinement { offline: true, fence: None }, &probe);
        assert!(!out.contains("CONNECTED"), "{out}");
        assert!(listener.accept().is_err(), "the server was reached");
        let (ok, out) = run(&Confinement { offline: false, fence: None }, &probe);
        assert!(ok && out.contains("CONNECTED"), "online the same command connects: {out}");
    }
}
