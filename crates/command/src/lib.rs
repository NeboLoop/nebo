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
    /// Outlives Nebo: detached from Nebo's console and process group, and no
    /// window (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP |
    /// CREATE_NO_WINDOW`). The updater's helper, which swaps the install in
    /// after Nebo exits and must not die with it.
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
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        match self {
            Console::Hidden => CREATE_NO_WINDOW,
            Console::Detached => DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
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
        assert_eq!(Console::Detached.creation_flags(), 0x0800_0208);
        assert_eq!(Console::Inherit.creation_flags(), 0);
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
