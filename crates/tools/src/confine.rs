//! What a command an employee runs may reach, enforced by the operating
//! system around the command itself: Nebo's own files (`nebo_files`) can't
//! be read or changed, Nebo's own ports (`types::own_ports`: its local API,
//! its browser's debugging port) can't be connected to, and a run whose web
//! access is off can't reach the network. Everything else the command does
//! runs as it would anyway, a server the employee started on this computer
//! included.
//!
//! A limit on the command's words can be walked around (`curl`, then
//! `python -c "urllib…"`, then `nc`); a limit on the process can't, whatever
//! the command is. So the shell's one command (`ShellTool::command`) starts
//! under it:
//!
//! - macOS: `sandbox-exec` with a profile that allows everything but those
//!   paths, connections to those ports on any address, and offline every IP
//!   socket (the network and this computer's own servers alike).
//! - Linux: `bwrap` over the whole filesystem, those paths covered, and
//!   offline in a network namespace of its own with nothing in it. The
//!   ports: the command is started from a thread Landlock keeps from
//!   connecting to them (`spawn_with`), which needs Linux 6.7; an older
//!   kernel leaves them reachable, and says so once.
//!
//! The skill-script sandbox (`sandbox_policy`) is a different confinement:
//! deny-by-default for marketplace code, network only through its filtering
//! proxy. An employee's shell does the owner's work with the owner's tools,
//! so it keeps everything but what this module takes away.
//!
//! What a command may reach is the permission check's to decide, once per
//! call ([`Reach`], `GateVerdict::Run`). An employee with Full access runs
//! its commands without any of this, as the owner chose: a program that
//! starts a sandbox of its own (Chrome, `swift build`, a Homebrew build)
//! works there. Its web access, when the owner turned it off, still holds:
//! its commands run offline, and nothing else is closed.
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

/// What the command a call starts may reach, as the permission check
/// decided it for that call (`GateVerdict::Run`, then `ToolContext::reach`).
/// The default is every employee's: Nebo's own files and ports closed by
/// the operating system, the network open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reach {
    /// The run's web access is off: the command may not reach the network.
    pub offline: bool,
    /// The employee holds Full access, and so does every grant it runs
    /// under: the operating system closes nothing for the command but the
    /// network when `offline`. The command's text is still checked
    /// (`safeguard`), and Nebo's own settings stay out of its environment.
    pub unconfined: bool,
}

/// The confinement one command runs under.
pub struct Confinement<'a> {
    /// The command may not reach the network: the run's web access is off.
    pub offline: bool,
    /// Nebo's own files, closed to the command. `None`: Nebo's folder can't
    /// be found, so there is nothing to fence, or the employee has Full
    /// access (`Reach::unconfined`).
    pub fence: Option<&'a NeboFiles>,
    /// Ports the command may not connect to, on any address: Nebo's own.
    /// On Linux the spawn carries it (`spawn_with`), not the prefix.
    pub closed_ports: &'a [u16],
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
        if !self.offline && self.fence.is_none() && self.closed_ports.is_empty() {
            return Ok(Vec::new());
        }
        match platform() {
            Some(Platform::Seatbelt) => Ok(vec![
                SANDBOX_EXEC.into(),
                "-p".into(),
                profile(self.offline, self.fence, self.closed_ports).into(),
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
fn profile(offline: bool, fence: Option<&NeboFiles>, closed_ports: &[u16]) -> String {
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
    for port in closed_ports {
        p.push(format!("(deny network-outbound (remote ip \"*:{port}\"))"));
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

/// Start a command (`spawn`: the fork) so it can't connect to
/// `closed_ports`. On Linux the fork happens on a thread of its own that
/// Landlock keeps from connecting to them, and what it starts inherits
/// that; a pipe and a terminal start alike. Elsewhere the ports are in the
/// prefix's profile, and `spawn` runs as it is.
pub fn spawn_with<R: Send>(closed_ports: &[u16], spawn: impl FnOnce() -> R + Send) -> R {
    #[cfg(target_os = "linux")]
    if !closed_ports.is_empty()
        && let Some(rules) = landlock::ruleset(closed_ports)
    {
        let runtime = tokio::runtime::Handle::try_current().ok();
        return std::thread::scope(|s| {
            s.spawn(|| {
                let _entered = runtime.as_ref().map(|r| r.enter());
                if !landlock::restrict_self(&rules) {
                    tracing::warn!("Landlock refused the spawner thread: Nebo's own ports stay reachable from this command");
                }
                spawn()
            })
            .join()
            .expect("the spawner thread panicked")
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = closed_ports;
    spawn()
}

/// Landlock's TCP-connect rules (Linux 6.7, ABI 4): a ruleset that lets a
/// thread connect to every port but Nebo's own, built once per set of
/// ports and applied to the thread that forks a command.
#[cfg(target_os = "linux")]
mod landlock {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::{Arc, Mutex};

    const CREATE_RULESET_VERSION: libc::c_uint = 1;
    const ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;
    const RULE_NET_PORT: libc::c_uint = 2;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
    }

    #[repr(C, packed)]
    struct NetPortAttr {
        allowed_access: u64,
        port: u64,
    }

    /// The ruleset closing `closed`, or `None` when this kernel has no
    /// TCP rules (told once).
    pub fn ruleset(closed: &[u16]) -> Option<Arc<OwnedFd>> {
        static BUILT: Mutex<Option<(Vec<u16>, Arc<OwnedFd>)>> = Mutex::new(None);
        let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ports, rules)) = built.as_ref()
            && ports == closed
        {
            return Some(rules.clone());
        }
        let rules = Arc::new(build(closed)?);
        *built = Some((closed.to_vec(), rules.clone()));
        Some(rules)
    }

    fn build(closed: &[u16]) -> Option<OwnedFd> {
        // SAFETY: the documented version query: no attribute, size 0.
        let abi = unsafe {
            libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<RulesetAttr>(), 0usize, CREATE_RULESET_VERSION)
        };
        if abi < 4 {
            static TOLD: std::sync::Once = std::sync::Once::new();
            TOLD.call_once(|| {
                tracing::warn!(abi, "this Linux has no Landlock TCP rules (6.7+): employee commands can reach Nebo's own ports")
            });
            return None;
        }
        let attr = RulesetAttr { handled_access_fs: 0, handled_access_net: ACCESS_NET_CONNECT_TCP };
        // SAFETY: `attr` outlives the call and its size is passed.
        let fd = unsafe {
            libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, std::mem::size_of::<RulesetAttr>(), 0)
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: a new descriptor the kernel just returned, owned here alone.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        for port in (0..=u16::MAX).filter(|p| !closed.contains(p)) {
            let rule = NetPortAttr { allowed_access: ACCESS_NET_CONNECT_TCP, port: port as u64 };
            // SAFETY: `rule` outlives the call; the ruleset fd is open.
            let added = unsafe {
                libc::syscall(libc::SYS_landlock_add_rule, fd.as_raw_fd(), RULE_NET_PORT, &rule as *const NetPortAttr, 0)
            };
            if added != 0 {
                return None;
            }
        }
        Some(fd)
    }

    /// Hold the calling thread, and everything it starts, to `rules`.
    pub fn restrict_self(rules: &OwnedFd) -> bool {
        // SAFETY: plain prctl and syscall on the calling thread; the fd is open.
        unsafe {
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0
                && libc::syscall(libc::SYS_landlock_restrict_self, rules.as_raw_fd(), 0) == 0
        }
    }
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
        cmd.arg("-c").arg(command);
        let out = spawn_with(c.closed_ports, || cmd.output()).unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// Nothing to confine: the shell runs as it is.
    #[test]
    fn nothing_to_confine_adds_nothing() {
        assert!(Confinement { offline: false, fence: None, closed_ports: &[] }.prefix().unwrap().is_empty());
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
        let fence = NeboFiles::at(&root, &root.join("sessions/s1"), false);
        let c = Confinement { offline: false, fence: Some(&fence), closed_ports: &[] };
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
            assert_eq!(Confinement { offline: true, fence: None, closed_ports: &[] }.prefix(), Err(Unconfinable));
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let probe = format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo CONNECTED");
        let (_, out) = run(&Confinement { offline: true, fence: None, closed_ports: &[] }, &probe);
        assert!(!out.contains("CONNECTED"), "{out}");
        assert!(listener.accept().is_err(), "the server was reached");
        let (ok, out) = run(&Confinement { offline: false, fence: None, closed_ports: &[] }, &probe);
        assert!(ok && out.contains("CONNECTED"), "online the same command connects: {out}");
    }

    /// Nebo's own ports are closed to the command on every address; a
    /// server the employee started on this computer still answers it.
    #[test]
    fn a_command_cannot_reach_nebo_own_ports() {
        if platform().is_none() {
            eprintln!("no confinement on this computer: Nebo's own ports are reachable");
            return;
        }
        let nebo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dev = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (nebo_port, dev_port) = (nebo.local_addr().unwrap().port(), dev.local_addr().unwrap().port());
        nebo.set_nonblocking(true).unwrap();
        let c = Confinement { offline: false, fence: None, closed_ports: &[nebo_port] };
        let probe = |port: u16| format!("exec 3<>/dev/tcp/127.0.0.1/{port} && echo CONNECTED");
        let (_, out) = run(&c, &probe(nebo_port));
        assert!(!out.contains("CONNECTED"), "Nebo's own port: {out}");
        assert!(nebo.accept().is_err(), "Nebo's own port took the connection");
        let (ok, out) = run(&c, &probe(dev_port));
        assert!(ok && out.contains("CONNECTED"), "the employee's own server: {out}");
    }
}
