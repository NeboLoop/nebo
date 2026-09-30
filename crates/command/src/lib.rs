//! Every child process Nebo starts is made here: [`new`] is the one
//! constructor, for `std::process::Command` and `tokio::process::Command`
//! alike. There is no other way.
//!
//! Why this exists: on Windows, a console program started by a process that
//! has no console of its own (the desktop app) gets a NEW console window —
//! one terminal flashing open per `git`, `where`, `powershell`, MCP server or
//! plugin Nebo starts. The fix is one creation flag, `CREATE_NO_WINDOW`, and
//! it was set by hand at a handful of ~160 sites. Opening Nebo on Windows opened a
//! bunch of terminals. Here the flag is not something a caller can forget:
//! the caller names how the child relates to the console, and the
//! architecture drift gate (`crates/tools/tests/architecture_drift.rs`)
//! fails the build on a `Command::new` or `creation_flags` anywhere else.
//!
//! On macOS and Linux nothing changes: a child gets no window of its own
//! there, and the choice is a no-op.

use std::ffi::OsStr;

/// How a child relates to the console. Only Windows acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Console {
    /// No console window (`CREATE_NO_WINDOW`). Every child Nebo starts,
    /// unless it is one of the two cases below. Its piped or inherited
    /// stdio still works; there is just no window.
    Hidden,
    /// Outlives Nebo: its own process group and its own hidden console
    /// (`CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`). The updater's helper,
    /// which swaps the install in after Nebo exits and must not die with it.
    ///
    /// Not `DETACHED_PROCESS`: that leaves the helper with no console at all,
    /// so Windows opens a new window for every console program the helper
    /// script starts (`tasklist`, `find`, `ping` in its wait loop), and it
    /// makes Windows ignore `CREATE_NO_WINDOW`. With `CREATE_NO_WINDOW` alone
    /// the helper gets a console of its own that is never shown and its
    /// children share it. It is still detached from Nebo: that console is
    /// not Nebo's (closing Nebo's never reaches it), the new process group
    /// keeps Nebo's Ctrl+C from reaching it, and a Windows child is not ended
    /// when its parent exits.
    Detached,
    /// Shares Nebo's own console: no flag at all. Only for a child whose
    /// output belongs in the terminal the user started Nebo from — a CLI
    /// command's script or build that prints as it runs, or Nebo starting
    /// itself over after an update (what `exec` does on Unix). Never for
    /// anything the desktop app or the server starts: without a console of
    /// Nebo's own, Windows opens a new window for it.
    Inherit,
}

impl Console {
    /// The Windows process-creation flags for this choice.
    pub const fn creation_flags(self) -> u32 {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        match self {
            Console::Hidden => CREATE_NO_WINDOW,
            Console::Detached => CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
            Console::Inherit => 0,
        }
    }
}

/// A command for `program`, set up for `console`. `C` is
/// `std::process::Command` or `tokio::process::Command`:
///
/// ```
/// use nebo_command::{self as command, Console};
/// let sync: std::process::Command = command::new("git", Console::Hidden);
/// let not_sync = command::new::<tokio::process::Command>("git", Console::Hidden);
/// # let _ = (sync, not_sync);
/// ```
pub fn new<C: From<std::process::Command>>(program: impl AsRef<OsStr>, console: Console) -> C {
    let mut cmd = std::process::Command::new(program);
    set_creation_flags(&mut cmd, console.creation_flags());
    C::from(cmd)
}

#[cfg(windows)]
fn set_creation_flags(cmd: &mut std::process::Command, flags: u32) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(flags);
}

/// Creation flags are Windows's own; a child elsewhere never gets a window,
/// so there is nothing to set.
#[cfg(not(windows))]
fn set_creation_flags(_cmd: &mut std::process::Command, _flags: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_child_is_hidden_and_the_updater_helper_also_detached() {
        assert_eq!(Console::Hidden.creation_flags(), 0x0800_0000);
        assert_eq!(Console::Detached.creation_flags(), 0x0800_0200);
        assert_eq!(Console::Inherit.creation_flags(), 0);
    }

    /// `DETACHED_PROCESS` (0x8) leaves the helper with no console, so every
    /// program its script starts opens a window, and Windows ignores
    /// `CREATE_NO_WINDOW` beside it. No choice may carry it.
    #[test]
    fn no_choice_leaves_a_child_without_a_console() {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        for console in [Console::Hidden, Console::Detached, Console::Inherit] {
            assert_eq!(console.creation_flags() & DETACHED_PROCESS, 0, "{console:?}");
        }
    }

    /// The updater's helper outlives the process that started it. This test
    /// runs itself again as the parent: that process starts the helper as
    /// `Console::Detached` and exits at once; the helper finishes its work
    /// after the parent is gone.
    #[test]
    fn a_detached_child_outlives_its_parent() {
        const PARENT: &str = "NEBO_COMMAND_DETACHED_PARENT";
        if let Some(done) = std::env::var_os(PARENT) {
            let done = std::path::PathBuf::from(done);
            let (program, args): (&str, Vec<String>) = if cfg!(windows) {
                ("cmd", vec!["/C".into(), format!("ping -n 3 127.0.0.1 >NUL & echo done> \"{}\"", done.display())])
            } else {
                ("sh", vec!["-c".into(), format!("sleep 2; echo done > '{}'", done.display())])
            };
            // The helper outlives this process: that is what is tested.
            #[allow(clippy::zombie_processes)]
            let _helper = new::<std::process::Command>(program, Console::Detached)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("helper starts");
            return;
        }

        let dir = std::env::temp_dir().join(format!("nebo-command-detached-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let done = dir.join("done");
        let parent = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "tests::a_detached_child_outlives_its_parent", "--test-threads=1"])
            .env(PARENT, &done)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("parent runs");
        assert!(parent.success(), "the parent started the helper and exited");
        assert!(!done.exists(), "the helper was still at work when its parent exited");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(done.exists(), "the detached helper ran to the end after its parent was gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hidden_child_still_runs_and_its_output_is_read() {
        let (program, args): (&str, &[&str]) =
            if cfg!(windows) { ("cmd", &["/C", "echo nebo"]) } else { ("sh", &["-c", "echo nebo"]) };
        let out = new::<std::process::Command>(program, Console::Hidden).args(args).output().expect("runs");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "nebo");
    }

    #[tokio::test]
    async fn the_tokio_command_keeps_what_was_set() {
        let (program, args): (&str, &[&str]) =
            if cfg!(windows) { ("cmd", &["/C", "echo nebo"]) } else { ("sh", &["-c", "echo nebo"]) };
        let out = new::<tokio::process::Command>(program, Console::Hidden)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await
            .expect("runs");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "nebo");
    }
}
