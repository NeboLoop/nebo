//! The terminal a command runs in when it asks for one (`run_command` with
//! `pty: true`): a pseudo-terminal opened for it, what the model reads of the
//! program's screen output, and the bytes a named key sends.
//!
//! A program that checks for a terminal (`test -t 1`, `isatty`) behaves the
//! way it does for a person only in one: prompts, REPLs, pagers, installers
//! that ask questions, full-screen programs. A pipe gives none of that.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

/// The size a terminal opens at: wide enough that ordinary output does not
/// wrap, the height of a laptop terminal window.
pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 30;

/// What a terminal program is told it is talking to.
const TERM: &str = "xterm-256color";

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A running command's terminal: its size, how it is stopped, and which
/// codes its arrow keys send.
pub struct Control {
    /// The terminal's controlling side. `None` once the command has ended,
    /// so the terminal is closed with it.
    master: std::sync::Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// The program switched its cursor keys to application mode (`ESC[?1h`,
    /// what full-screen programs do): an arrow key is `ESC O A`, not `ESC [ A`.
    app_cursor: Arc<AtomicBool>,
    /// On Windows the process-group kill does nothing; the terminal's own
    /// killer ends the command.
    #[cfg(windows)]
    killer: std::sync::Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Terminal")
    }
}

impl Control {
    /// Set the terminal's size; the program is told (SIGWINCH on unix). A
    /// dimension left out keeps its current value.
    pub fn resize(&self, cols: Option<u16>, rows: Option<u16>) -> Result<(u16, u16), String> {
        let master = self.master.lock().unwrap_or_else(|e| e.into_inner());
        let master = master.as_ref().ok_or("the command has ended")?;
        let now = master
            .get_size()
            .map_err(|e| format!("could not read the terminal size: {e}"))?;
        let (cols, rows) = (cols.unwrap_or(now.cols), rows.unwrap_or(now.rows));
        if cols == 0 || rows == 0 {
            return Err("cols and rows must be at least 1".to_string());
        }
        master
            .resize(size(cols, rows))
            .map_err(|e| format!("could not resize the terminal: {e}"))?;
        Ok((cols, rows))
    }

    /// Whether arrow keys send application-mode codes.
    pub fn app_cursor(&self) -> bool {
        self.app_cursor.load(Ordering::Relaxed)
    }

    /// Close the terminal once its command has ended. On Windows this is what
    /// ends the reader: a pseudo console's output stays open until it is closed.
    pub(crate) fn close(&self) -> Option<Box<dyn MasterPty + Send>> {
        self.master.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// End the command. On unix the process-group kill already did.
    pub(crate) fn kill(&self) {
        #[cfg(windows)]
        {
            let _ = self.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
    }
}

/// A command started in a terminal: its control, the reader and writer of
/// the terminal, and the child to wait on.
pub struct Opened {
    pub control: Arc<Control>,
    pub reader: Box<dyn std::io::Read + Send>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

/// Start `cmd` in a new terminal. `cmd` is the command `run_command` built
/// (`ShellTool::command`): its program, arguments, folder and environment
/// carry over, and the environment is exactly the one it set, since that
/// command clears the inherited one first. On unix the command leads its own
/// session, so its pid is its process group and the group kill reaches it.
pub fn open(cmd: &tokio::process::Command) -> std::io::Result<Opened> {
    let std = cmd.as_std();
    let mut builder = CommandBuilder::new(std.get_program());
    builder.args(std.get_args());
    builder.env_clear();
    for (key, value) in std.get_envs() {
        if let Some(value) = value {
            builder.env(key, value);
        }
    }
    builder.env("TERM", TERM);
    // Left unset, the terminal would start in the home folder; a pipe starts
    // where Nebo runs.
    match std.get_current_dir() {
        Some(dir) => builder.cwd(dir),
        None => builder.cwd(std::env::current_dir()?),
    }

    let pair = portable_pty::native_pty_system()
        .openpty(size(DEFAULT_COLS, DEFAULT_ROWS))
        .map_err(std::io::Error::other)?;
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(std::io::Error::other)?;
    // The command holds its own side; ours closing is what lets the reader
    // see the end once the command is gone.
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(std::io::Error::other)?;
    let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
    let control = Arc::new(Control {
        #[cfg(windows)]
        killer: std::sync::Mutex::new(portable_pty::ChildKiller::clone_killer(&*child)),
        master: std::sync::Mutex::new(Some(pair.master)),
        app_cursor: Arc::default(),
    });
    Ok(Opened {
        control,
        reader,
        writer,
        child,
    })
}

/// Wait for a terminal's command to end, as the exit status a pipe's command
/// gives: on unix the raw wait status, so a signal is named (a command ended
/// by Ctrl-C reads as SIGINT, not as exit code 1).
pub fn wait(child: Box<dyn Child + Send + Sync>) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let pid = child.process_id()? as libc::pid_t;
        let mut status: libc::c_int = 0;
        loop {
            // SAFETY: waitpid on the pid of a child this process spawned and
            // nothing else waits on.
            let r = unsafe { libc::waitpid(pid, &mut status, 0) };
            if r == pid {
                return Some(std::process::ExitStatus::from_raw(status));
            }
            if r == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            return None;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        let mut child = child;
        child
            .wait()
            .ok()
            .map(|s| std::process::ExitStatus::from_raw(s.exit_code()))
    }
}

/// A terminal's output as the model reads it: the text the program printed,
/// without the escape codes that color it and move the cursor. The parser
/// keeps its state between reads, so a code split across two reads is still
/// removed whole.
pub struct Plain {
    parser: vte::Parser,
    out: PlainOut,
}

#[derive(Default)]
struct PlainOut {
    text: String,
    /// What the terminal answers the program with (see `csi_dispatch`).
    replies: Vec<u8>,
    /// The last thing was a carriage return: a line feed after it is the
    /// same line end.
    cr: bool,
    app_cursor: Arc<AtomicBool>,
}

impl Plain {
    pub fn new(control: &Control) -> Self {
        Self::with_cursor_mode(control.app_cursor.clone())
    }

    fn with_cursor_mode(app_cursor: Arc<AtomicBool>) -> Self {
        Self {
            parser: vte::Parser::new(),
            out: PlainOut {
                app_cursor,
                ..Default::default()
            },
        }
    }

    /// Read `bytes`: the plain text in them, and what the terminal answers
    /// the program with.
    pub fn feed(&mut self, bytes: &[u8]) -> (String, Vec<u8>) {
        self.parser.advance(&mut self.out, bytes);
        (
            std::mem::take(&mut self.out.text),
            std::mem::take(&mut self.out.replies),
        )
    }
}

impl vte::Perform for PlainOut {
    fn print(&mut self, c: char) {
        self.cr = false;
        self.text.push(c);
    }

    /// A terminal ends a line with `\r\n`; a lone `\r` (a progress line
    /// redrawn in place) is a line end too, so each state reads on its own line.
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => {
                if !self.cr {
                    self.text.push('\n');
                }
                self.cr = true;
            }
            b'\n' => {
                if !self.cr {
                    self.text.push('\n');
                }
                self.cr = false;
            }
            b'\t' => {
                self.cr = false;
                self.text.push('\t');
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let first = params.iter().next().and_then(|p| p.first().copied());
        match (intermediates, action) {
            // A program asking where the cursor is waits for the answer;
            // nobody is at this terminal to give it, so the terminal does.
            ([], 'n') if first == Some(6) => self.replies.extend_from_slice(b"\x1b[1;1R"),
            ([b'?'], 'h' | 'l') if params.iter().any(|p| p.first() == Some(&1)) => {
                self.app_cursor.store(action == 'h', Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// The key names `send_input` takes, for its error.
const KEY_NAMES: &str = "Enter, Tab, Shift-Tab, Escape, Backspace, Space, Up, Down, Left, Right, Home, End, \
     PageUp, PageDown, Insert, Delete, F1–F12, Ctrl-<key> (Ctrl-C, Ctrl-D, …), Alt-<key>";

/// The bytes named keys send, in order. `app_cursor`: the program switched
/// its cursor keys to application mode.
pub fn encode_keys(keys: &[String], app_cursor: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for key in keys {
        let bytes = encode_key(key.trim(), app_cursor)
            .ok_or_else(|| format!("unknown key '{key}'. Keys: {KEY_NAMES}"))?;
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

fn encode_key(key: &str, app_cursor: bool) -> Option<Vec<u8>> {
    // `^C` is Ctrl-C.
    if let Some(rest) = key.strip_prefix('^')
        && rest.chars().count() == 1
    {
        return ctrl(rest.chars().next()?).map(|b| vec![b]);
    }
    if let Some((modifier, rest)) = key.split_once(['-', '+'])
        && !rest.is_empty()
    {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "c" => {
                let mut chars = rest.chars();
                let (c, None) = (chars.next()?, chars.next()) else {
                    return None;
                };
                return ctrl(c).map(|b| vec![b]);
            }
            "alt" | "meta" | "m" => {
                let mut bytes = vec![0x1b];
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => bytes.extend_from_slice(c.to_string().as_bytes()),
                    _ => bytes.extend(encode_key(rest, app_cursor)?),
                }
                return Some(bytes);
            }
            "shift" if rest.eq_ignore_ascii_case("tab") => return Some(b"\x1b[Z".to_vec()),
            _ => {}
        }
    }
    let cursor = |c: u8| {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let bytes = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "btab" | "backtab" => b"\x1b[Z".to_vec(),
        "escape" | "esc" => vec![0x1b],
        "backspace" | "bspace" => vec![0x7f],
        "space" => vec![b' '],
        "up" => cursor(b'A'),
        "down" => cursor(b'B'),
        "right" => cursor(b'C'),
        "left" => cursor(b'D'),
        "home" => cursor(b'H'),
        "end" => cursor(b'F'),
        "pageup" | "pgup" => b"\x1b[5~".to_vec(),
        "pagedown" | "pgdn" => b"\x1b[6~".to_vec(),
        "insert" => b"\x1b[2~".to_vec(),
        "delete" | "del" => b"\x1b[3~".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => b"\x1b[15~".to_vec(),
        "f6" => b"\x1b[17~".to_vec(),
        "f7" => b"\x1b[18~".to_vec(),
        "f8" => b"\x1b[19~".to_vec(),
        "f9" => b"\x1b[20~".to_vec(),
        "f10" => b"\x1b[21~".to_vec(),
        "f11" => b"\x1b[23~".to_vec(),
        "f12" => b"\x1b[24~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

/// The control byte Ctrl with `c` sends.
fn ctrl(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(k: &[&str]) -> Result<Vec<u8>, String> {
        encode_keys(&k.iter().map(|s| s.to_string()).collect::<Vec<_>>(), false)
    }

    #[test]
    fn named_keys_send_what_a_terminal_sends() {
        assert_eq!(keys(&["Enter"]).unwrap(), b"\r");
        assert_eq!(
            keys(&["Ctrl-C", "C-d", "^Z", "ctrl+l"]).unwrap(),
            [3, 4, 26, 12]
        );
        assert_eq!(
            keys(&["Up", "Tab", "Shift-Tab", "Alt-b", "F5"]).unwrap(),
            b"\x1b[A\t\x1b[Z\x1bb\x1b[15~"
        );
        let app = encode_keys(&["Up".to_string(), "Home".to_string()], true).unwrap();
        assert_eq!(app, b"\x1bOA\x1bOH", "application cursor mode");
        let err = keys(&["Enter", "Hyper-X"]).unwrap_err();
        assert!(
            err.contains("unknown key 'Hyper-X'") && err.contains("Ctrl-C"),
            "{err}"
        );
        assert!(
            keys(&["a"]).is_err(),
            "a single character is text, not a key"
        );
    }

    #[test]
    fn the_model_reads_text_without_escape_codes() {
        let app_cursor = Arc::new(AtomicBool::new(false));
        let mut plain = Plain::with_cursor_mode(app_cursor.clone());
        // Colour, a code split across two reads, CRLF line ends, a redrawn line.
        let (a, _) = plain.feed(b"\x1b[1;32mok\x1b[0m\r\n\x1b[3");
        let (b, _) = plain.feed(b"1mred\x1b[0m\r\n10%\r20%\r\n");
        assert_eq!(a + b.as_str(), "ok\nred\n10%\n20%\n");
        // A cursor-position query is answered; application cursor mode is seen.
        let (text, replies) = plain.feed(b"\x1b[6n\x1b[?1h>");
        assert_eq!(
            (text.as_str(), replies.as_slice()),
            (">", b"\x1b[1;1R".as_slice())
        );
        assert!(app_cursor.load(Ordering::Relaxed));
    }
}
