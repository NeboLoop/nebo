//! Signing the owner in to a command-line tool on this computer.
//!
//! The login runs in a terminal (`run_command` with `pty: true, background:
//! true`), preferring the tool's no-browser option. When its screen shows a
//! sign-in link, `send_input` with `sign_in` hands it to the owner: a card
//! in the conversation the work came from, with the link, the code to type
//! there when the tool shows one, and a field for the code the tool asks
//! back for when it asks. Nebo reads the screen itself; the model never
//! copies the link or handles the code.
//!
//! The owner's code goes to the terminal and nowhere else: not to the model,
//! not into the conversation, not into a log. Everything read from the
//! session after it was typed has the code masked ([`Masks`]), the card
//! closes everywhere as "code entered", and the model is told only that.

use std::time::Duration;

use serde_json::{Value, json};

use crate::origin::{Asked, ExecutionMode, SKIP_SENTINEL, ToolContext};
use crate::plugin_tool::CARD_FAILED_PREFIX;
use crate::process::ProcessRegistry;
use crate::registry::ToolResult;

/// The card's widget type.
pub const CARD: &str = "sign_in";
/// What the card shows once the owner's code went into the terminal. Every
/// client shows this, never the code.
pub const CODE_ENTERED: &str = "code_entered";
/// What the card shows when the sign-in finished in the terminal.
pub const SIGNED_IN: &str = "signed_in";

/// What replaces the owner's code wherever the terminal shows it.
const MASK: &str = "[code entered]";

/// The no-browser sign-ins of the tools owners ask for most.
const NO_BROWSER: &str = "gh auth login --web, gcloud auth login --no-launch-browser, \
     NO_BROWSER=true gemini, firebase login --no-localhost";

/// How long the hand-over waits: for the sign-in link to show, for the
/// owner, and for the tool to finish after the code went in.
#[derive(Debug, Clone, Copy)]
pub struct Waits {
    pub link: Duration,
    pub owner: Duration,
    pub after: Duration,
}

impl Default for Waits {
    /// Sign-in links and codes expire in about ten minutes (Google's codes,
    /// GitHub's device codes in fifteen): the owner gets that long.
    fn default() -> Self {
        Self {
            link: Duration::from_secs(30),
            owner: Duration::from_secs(10 * 60),
            after: Duration::from_secs(60),
        }
    }
}

/// A sign-in on a terminal's screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// The last link on the screen that isn't this computer.
    pub url: String,
    /// The code the owner types on the sign-in page (a device code), when
    /// the tool shows one.
    pub code: Option<String>,
    /// The tool is waiting for a code pasted back.
    pub asks_for_code: bool,
    /// The link sends the browser back to this computer (a localhost
    /// redirect), which a phone or another computer can't reach.
    pub sends_back_here: bool,
}

/// The sign-in on `screen`, if it shows a link.
pub fn read(screen: &str) -> Option<Screen> {
    let lines: Vec<&str> = screen.lines().collect();
    let (at, url) = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, line)| link_in(line).map(|url| (i, url)))?;
    let code = lines[at.saturating_sub(3)..]
        .iter()
        .rev()
        .find_map(|line| device_code(line));
    let asks_for_code = lines[at + 1..]
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| is_code_prompt(line, code.as_deref()));
    let sends_back_here = redirects_here(&url);
    Some(Screen { url, code, asks_for_code, sends_back_here })
}

/// Whether `screen` shows a sign-in rather than any link: a code to type, a
/// code asked for, or the words of one.
fn looks_like_sign_in(screen: &str, s: &Screen) -> bool {
    const WORDS: [&str; 6] = ["sign in", "sign-in", "log in", "login", "authoriz", "authenticat"];
    let lower = screen.to_lowercase();
    s.code.is_some() || s.asks_for_code || WORDS.iter().any(|w| lower.contains(w))
}

/// The last link in `line` whose host isn't this computer.
fn link_in(line: &str) -> Option<String> {
    line.split_whitespace()
        .filter_map(|word| {
            let word = word.trim_start_matches(['(', '<', '"', '\'', '`', '[']);
            let word = word.trim_end_matches(['.', ',', ';', ':', ')', '>', '"', '\'', '`', ']', '!']);
            let url = url::Url::parse(word).ok()?;
            (matches!(url.scheme(), "http" | "https") && !is_this_computer(url.host_str()?)).then(|| word.to_string())
        })
        .last()
}

fn is_this_computer(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost") || host == "0.0.0.0" || host == "::1" || host.starts_with("127.")
}

/// A sign-in link whose `redirect_uri` is this computer.
fn redirects_here(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.query_pairs().any(|(k, v)| {
            k == "redirect_uri" && url::Url::parse(&v).is_ok_and(|r| r.host_str().is_some_and(is_this_computer))
        })
    })
}

/// A device code on a line that speaks of a code: `1A2B-3C4D`,
/// `ABCD12345`. Capitals, digits and dashes, shaped like a code rather than
/// a word.
fn device_code(line: &str) -> Option<String> {
    if !line.to_lowercase().contains("code") {
        return None;
    }
    line.split_whitespace()
        .filter(|word| !word.contains("://"))
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|word| {
            let len = word.chars().count();
            let shaped = word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-');
            let digits = word.chars().filter(char::is_ascii_digit).count();
            let letters = word.chars().filter(char::is_ascii_uppercase).count();
            let dashed = word.contains('-') && word.split('-').all(|part| part.len() >= 3);
            shaped && (6..=12).contains(&len) && (dashed || (len >= 8 && digits >= 2 && letters >= 2))
        })
        .map(str::to_string)
}

/// A prompt for a code pasted back: "Enter the authorization code:",
/// "? Paste authorization code here:". A line that shows the device code
/// asks the owner to type it elsewhere, not here.
fn is_code_prompt(line: &str, device_code: Option<&str>) -> bool {
    let line = line.trim();
    let lower = line.to_lowercase();
    let prompt = line.ends_with(':') || line.ends_with('?') || line.ends_with('>');
    let about_a_code = ["code", "token", "paste"].iter().any(|w| lower.contains(w));
    prompt && about_a_code && !device_code.is_some_and(|c| line.contains(c))
}

/// Whether `command` runs a tool's login (`gh auth login`, `firebase
/// login`).
fn is_login_command(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'))
        .any(|word| word == "login")
}

/// What the model is told about a sign-in a command shows: in a terminal
/// session (`terminal`, its id), to hand it to the owner, or to use the
/// tool's no-browser option when the link would send the browser back to
/// this computer; for a command run without a terminal, that a sign-in runs
/// in one. `screen` is what the command printed.
pub fn hint(command: &str, screen: &str, terminal: Option<&str>) -> Option<String> {
    let shown = read(screen).filter(|s| s.sends_back_here || looks_like_sign_in(screen, s));
    let no_browser = format!(
        "This sign-in sends the browser back to this computer (a localhost link), which the owner's \
         phone can't open. Stop it and start the tool's no-browser sign-in in a terminal instead \
         ({NO_BROWSER}), then hand it over with send_input's sign_in."
    );
    match (terminal, shown) {
        (_, Some(s)) if s.sends_back_here => Some(no_browser),
        (Some(id), Some(_)) => Some(format!(
            "This is a sign-in waiting for the owner. Hand it to them: send_input(task_id: \"{id}\", \
             sign_in: \"<what they sign in to>\"). They get the link on a card in this conversation and \
             their code goes straight into the terminal. Don't copy the link into your reply or ask for \
             the code in chat."
        )),
        (Some(_), None) => None,
        (None, shown) => (shown.is_some() || is_login_command(command)).then(|| {
            format!(
                "A sign-in waits for the owner, and it runs in a terminal: run_command with pty: true \
                 and background: true, using the tool's no-browser option ({NO_BROWSER}). When it \
                 shows the link, hand it over with send_input(task_id, sign_in: \"<what they sign in to>\")."
            )
        }),
    }
}

/// What the card shows for `value`, the answer given to a card defined by
/// `widgets`: a sign-in card never shows the owner's code, only that it was
/// entered; every other card shows its answer.
pub fn answer_as_shown(widgets: Option<&Value>, value: String) -> String {
    let settled = value == SKIP_SENTINEL || value == SIGNED_IN || value.starts_with(CARD_FAILED_PREFIX);
    if is_card(widgets) && !settled { CODE_ENTERED.to_string() } else { value }
}

/// Whether a card's `widgets` are a sign-in card.
pub fn is_card(widgets: Option<&Value>) -> bool {
    widgets.and_then(|w| w.get(0)).and_then(|w| w.get("type")).and_then(Value::as_str) == Some(CARD)
}

/// The owner's code as it goes into the terminal: its first line, without
/// control characters. None when nothing is left.
fn clean_code(answer: &str) -> Option<String> {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let code: String = line.chars().filter(|c| !c.is_control()).collect();
    (!code.is_empty()).then_some(code)
}

/// The codes typed into a terminal, masked in everything read from it after.
/// A code is masked wherever six or more of its characters show in a row
/// (the whole of a shorter code), so a program that echoes it wrapped,
/// boxed or in pieces still shows none of it. The end of a read that could
/// run on into a code is held until the next read shows whether it does.
#[derive(Default)]
pub struct Masks {
    codes: Vec<Vec<char>>,
    held: String,
}

/// Shows how many codes it masks, never the codes.
impl std::fmt::Debug for Masks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Masks({} codes)", self.codes.len())
    }
}

impl Masks {
    /// Mask `code` from here on.
    pub fn add(&mut self, code: &str) {
        let code: Vec<char> = code.chars().collect();
        if !code.is_empty() && !self.codes.contains(&code) {
            self.codes.push(code);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    /// `text` as it may be read.
    pub fn mask(&mut self, text: &str) -> String {
        self.mask_until(text, false)
    }

    /// What was held back, once nothing more will come: a piece of a code
    /// is masked, anything else shown as it was.
    pub fn flush(&mut self) -> String {
        self.mask_until("", true)
    }

    /// `text` as it may be read; `end`: nothing follows it, so nothing is
    /// held.
    fn mask_until(&mut self, text: &str, end: bool) -> String {
        if self.codes.is_empty() {
            return text.to_string();
        }
        let s: Vec<char> = std::mem::take(&mut self.held).chars().chain(text.chars()).collect();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            let (len, rest_of_code) = self.longest_piece_at(&s[i..]);
            if !end && len > 0 && i + len == s.len() && rest_of_code > 0 {
                // It runs to the end of what came, and the code goes on:
                // the next read decides.
                self.held = s[i..].iter().collect();
                break;
            }
            if len >= self.min_piece() {
                out.push_str(MASK);
                i += len;
            } else {
                out.push(s[i]);
                i += 1;
            }
        }
        out
    }

    /// The fewest characters in a row that are masked: six, or the whole of
    /// a shorter code.
    fn min_piece(&self) -> usize {
        self.codes.iter().map(Vec::len).min().unwrap_or(0).min(6)
    }

    /// The longest run at the start of `s` that is a piece of a code, and
    /// how much of that code follows the piece.
    fn longest_piece_at(&self, s: &[char]) -> (usize, usize) {
        let mut best = (0, 0);
        for code in &self.codes {
            for start in 0..code.len() {
                let len = s.iter().zip(&code[start..]).take_while(|(a, b)| a == b).count();
                if len > best.0 {
                    best = (len, code.len() - start - len);
                }
            }
        }
        best
    }
}

/// How often a wait looks at the terminal again.
const TICK: Duration = Duration::from_millis(200);
/// A terminal that printed after the code and then printed nothing for this
/// long has said what it will say for now.
const QUIET: Duration = Duration::from_secs(3);
/// How much of the screen a result carries.
const SHOWN_CHARS: usize = 1500;

/// Where a session stands: its whole output (masked), and its exit when it
/// ended (`Some(None)`: ended by a signal or a stop).
async fn look(registry: &ProcessRegistry, id: &str) -> (String, Option<Option<i32>>) {
    match registry.get_any_session(id).await {
        Some(s) => (s.get_output().await, s.exited.then_some(s.exit_code)),
        None => (String::new(), Some(None)),
    }
}

/// The end of `text`, at most [`SHOWN_CHARS`].
fn tail(text: &str) -> String {
    let count = text.chars().count();
    let text: String = text.chars().skip(count.saturating_sub(SHOWN_CHARS)).collect();
    text.trim().to_string()
}

fn status(exit: Option<Option<i32>>) -> String {
    match exit {
        None => "It is still running.".to_string(),
        Some(Some(0)) => "It finished (exit code 0).".to_string(),
        Some(Some(code)) => format!("It ended with exit code {code}."),
        Some(None) => "It was ended by a signal or a stop.".to_string(),
    }
}

/// Hand the sign-in on terminal session `id` to the owner (`what`: what they
/// sign in to, as they'd name it). Waits for the link to show, raises the
/// card in this conversation, and then: types the owner's code into the
/// terminal and waits for the tool to take it; or, when the tool shows a
/// code to type on the page and asks for nothing back, waits for it to
/// finish. A cancel from the owner, a stopped turn or the owner's wait
/// running out stops the login.
pub async fn hand_over(registry: &ProcessRegistry, ctx: &ToolContext, id: &str, what: &str, waits: Waits) -> ToolResult {
    let what = what.trim();
    let Some(session) = registry.get_any_session(id).await else {
        return ToolResult::error(format!("Session not found: {id}"));
    };
    if !session.terminal {
        return ToolResult::error(format!(
            "Session {id} has no terminal, and a sign-in needs one. Stop it with stop_task and start the \
             login with run_command(pty: true, background: true), using the tool's no-browser option ({NO_BROWSER})."
        ));
    }
    if ExecutionMode::from(ctx.origin) != ExecutionMode::Interactive || ctx.ask_channels.is_none() || ctx.stream_tx.is_none() {
        return ToolResult::error(format!(
            "Signing in needs the owner in this conversation, and nobody is here to open the link. Stop \
             the login with stop_task(task_id: \"{id}\") and tell the owner the sign-in is ready to do \
             when they're in a chat or on a call with you."
        ));
    }

    // The link, once the tool has shown it.
    let deadline = tokio::time::Instant::now() + waits.link;
    let screen = loop {
        let (output, exit) = look(registry, id).await;
        if let Some(screen) = read(&output) {
            break screen;
        }
        if exit.is_some() || tokio::time::Instant::now() >= deadline {
            let shown = tail(&output);
            let shown = if shown.is_empty() { "nothing".to_string() } else { format!(":\n{shown}") };
            return ToolResult::error(format!(
                "No sign-in link is on session {id}'s screen. {} The screen shows{shown}\n\
                 Answer its questions with send_input (text, keys) until it shows the link, then hand \
                 it over again.",
                status(exit)
            ));
        }
        tokio::time::sleep(TICK).await;
    };
    if screen.sends_back_here {
        return ToolResult::error(hint("", &screen.url, Some(id)).unwrap_or_default());
    }

    let widgets = json!([{
        "type": CARD,
        "tool": what,
        "url": screen.url,
        "code": screen.code,
        "input": screen.asks_for_code,
    }]);
    let raised_at = look(registry, id).await.0.len();
    // What ends the card without the owner: the login ending, or the
    // owner's time running out.
    let asks_for_code = screen.asks_for_code;
    let ends = async {
        let deadline = tokio::time::Instant::now() + waits.owner;
        loop {
            match look(registry, id).await.1 {
                Some(Some(0)) if !asks_for_code => return SIGNED_IN.to_string(),
                Some(_) => return format!("{CARD_FAILED_PREFIX}The sign-in ended before it finished."),
                None if tokio::time::Instant::now() >= deadline => {
                    return format!("{CARD_FAILED_PREFIX}The sign-in timed out.");
                }
                None => tokio::time::sleep(TICK).await,
            }
        }
    };
    let asked = ctx.ask_user_until(&format!("Sign in to {what}"), widgets, ends).await;

    let stop = move || async move {
        let _ = registry.kill_session(id).await;
    };
    let since = |output: String| tail(output.get(raised_at..).unwrap_or(&output));
    let check = "Check the sign-in with the tool's own status command (gh auth status, gcloud auth list) \
                 and tell the owner how it went.";
    match asked {
        None => {
            stop().await;
            ToolResult::error(format!("The turn was stopped before the owner signed in; the login in {id} was stopped with it."))
        }
        Some(Asked::Answer(answer)) if answer == SKIP_SENTINEL => {
            stop().await;
            ToolResult::ok(format!("The owner cancelled the sign-in to {what}. The login in {id} was stopped."))
        }
        Some(Asked::Answer(answer)) if asks_for_code => {
            let Some(code) = clean_code(&answer) else {
                stop().await;
                return ToolResult::error(format!(
                    "The owner's answer had no code in it. The login in {id} was stopped; start it again to retry."
                ));
            };
            let typed_at = look(registry, id).await.0.len();
            if let Err(e) = registry.write_stdin(id, format!("{code}\r").as_bytes(), true).await {
                return ToolResult::error(format!("The owner's code could not be typed into {id}: {e}"));
            }
            // Until the tool finishes, or has answered and gone quiet.
            let deadline = tokio::time::Instant::now() + waits.after;
            let (mut last_len, mut quiet_since) = (typed_at, tokio::time::Instant::now());
            let (output, exit) = loop {
                let (output, exit) = look(registry, id).await;
                let now = tokio::time::Instant::now();
                if output.len() != last_len {
                    (last_len, quiet_since) = (output.len(), now);
                }
                let answered = last_len > typed_at && now.duration_since(quiet_since) >= QUIET;
                if exit.is_some() || answered || now >= deadline {
                    break (output, exit);
                }
                tokio::time::sleep(TICK).await;
            };
            ToolResult::ok(format!(
                "The owner's code for {what} was entered into {id} (you never see it, and it is masked on the \
                 screen). {} The screen since the card:\n{}\n\n{check}",
                status(exit),
                since(output)
            ))
        }
        Some(Asked::Answer(_)) => {
            // A message the owner typed while a card with nothing to type
            // back was open: not a code, typed nowhere. The login goes on.
            let deadline = tokio::time::Instant::now() + waits.after;
            let (output, exit) = loop {
                let (output, exit) = look(registry, id).await;
                if exit.is_some() || tokio::time::Instant::now() >= deadline {
                    break (output, exit);
                }
                tokio::time::sleep(TICK).await;
            };
            let said = match exit {
                Some(Some(0)) => format!("The owner signed in: {what}'s login in {id} finished (exit code 0)."),
                None => format!("The owner answered the sign-in card; the login in {id} is still waiting. Read it later with read_output."),
                Some(_) => format!("The sign-in to {what} didn't finish. {}", status(exit)),
            };
            ToolResult::ok(format!("{said} The screen since the card:\n{}\n\n{check}", since(output)))
        }
        Some(Asked::Ended(shown)) => {
            let (output, exit) = look(registry, id).await;
            let said = if shown == SIGNED_IN {
                format!("The owner signed in: {what}'s login in {id} finished (exit code 0).")
            } else if exit.is_none() {
                stop().await;
                format!("The owner didn't sign in to {what} within {}; the login in {id} was stopped.", span(waits.owner))
            } else {
                format!("The sign-in to {what} didn't finish. {}", status(exit))
            };
            let result = format!("{said} The screen since the card:\n{}\n\n{check}", since(output));
            if shown == SIGNED_IN { ToolResult::ok(result) } else { ToolResult::error(result) }
        }
    }
}

/// `d` in words: "10 minutes", "5 seconds".
fn span(d: Duration) -> String {
    match d.as_secs() {
        s if s >= 60 && s % 60 == 0 => format!("{} minute{}", s / 60, if s == 60 { "" } else { "s" }),
        s => format!("{s} second{}", if s == 1 { "" } else { "s" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screens as the tools print them.
    pub(super) const GCLOUD: &str = "Go to the following link in your browser, and complete the sign-in prompts:\n\n    \
        https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=32555940559.apps.googleusercontent.com\
        &redirect_uri=https%3A%2F%2Fsdk.cloud.google.com%2Fauthcode.html&scope=openid&state=Xy7&prompt=consent\n\n\
        Once finished, enter the verification code provided in your browser: ";
    pub(super) const GCLOUD_LOCAL: &str = "Your browser has been opened to visit:\n\n    \
        https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=1.apps.googleusercontent.com\
        &redirect_uri=http%3A%2F%2Flocalhost%3A8085%2F&scope=openid&state=Q\n\n";
    pub(super) const GH: &str = "! First copy your one-time code: 1A2B-3C4D\n\
        Press Enter to open https://github.com/login/device in your browser... ";
    pub(super) const GEMINI: &str = "Please visit the following URL to authorize the application:\n\n\
        https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=https%3A%2F%2Fcodeassist.google.com%2Fauthcode&state=a\n\n\
        Enter the authorization code: ";
    pub(super) const FIREBASE: &str = "Visit this URL on this device to log in:\n\
        https://accounts.google.com/o/oauth2/auth?client_id=563584335869&redirect_uri=urn%3Aietf%3Awg%3Aoauth%3A2.0%3Aoob\n\n\
        ? Paste authorization code here: ";
    pub(super) const AZURE: &str = "To sign in, use a web browser to open the page https://microsoft.com/devicelogin \
        and enter the code F7GHXQ2LN to authenticate.";
    pub(super) const VERCEL: &str = "> Please visit the following URL in your web browser:\n\
        > https://vercel.com/oauth/device?user_code=WBQF-KPJC\n\
        > Waiting for authentication...";
    pub(super) const DEV_SERVER: &str = "  ▲ Next.js 15.0.0\n  - Local:        http://localhost:3000\n\n ✓ Ready in 1.2s";

    #[test]
    fn a_paste_back_sign_in_shows_its_link_and_asks_for_the_code() {
        for screen in [GCLOUD, GEMINI, FIREBASE] {
            let s = read(screen).unwrap_or_else(|| panic!("no sign-in on {screen:?}"));
            assert!(s.url.starts_with("https://accounts.google.com/"), "{s:?}");
            assert!(s.asks_for_code && s.code.is_none() && !s.sends_back_here, "{s:?}");
        }
    }

    #[test]
    fn a_device_code_sign_in_shows_the_code_and_asks_for_nothing() {
        let gh = read(GH).unwrap();
        assert_eq!((gh.url.as_str(), gh.code.as_deref(), gh.asks_for_code), ("https://github.com/login/device", Some("1A2B-3C4D"), false));
        let az = read(AZURE).unwrap();
        assert_eq!((az.url.as_str(), az.code.as_deref(), az.asks_for_code), ("https://microsoft.com/devicelogin", Some("F7GHXQ2LN"), false));
        let vercel = read(VERCEL).unwrap();
        assert_eq!((vercel.code.as_deref(), vercel.asks_for_code), (None, false), "the code is in the link");
        assert!(looks_like_sign_in(VERCEL, &vercel));
    }

    #[test]
    fn a_link_back_to_this_computer_is_named_and_a_dev_server_is_no_sign_in() {
        assert!(read(GCLOUD_LOCAL).unwrap().sends_back_here);
        assert_eq!(read(DEV_SERVER), None, "localhost is this computer, not a sign-in");
        let hint = hint("gcloud auth login", GCLOUD_LOCAL, Some("bg-1")).unwrap();
        assert!(hint.contains("no-launch-browser") && hint.contains("localhost"), "{hint}");
    }

    #[test]
    fn the_hint_hands_a_terminal_sign_in_over_and_sends_others_to_a_terminal() {
        let terminal = hint("gh auth login --web", GH, Some("bg-9")).unwrap();
        assert!(terminal.contains("send_input(task_id: \"bg-9\", sign_in:"), "{terminal}");
        assert!(terminal.contains("Don't copy the link"), "{terminal}");
        let pipe = hint("gcloud auth login --no-launch-browser", GCLOUD, None).unwrap();
        assert!(pipe.contains("pty: true") && pipe.contains("background: true"), "{pipe}");
        let bare = hint("firebase login", "Error: Cannot run login in non-interactive mode.", None).unwrap();
        assert!(bare.contains("firebase login --no-localhost"), "a login command without a terminal: {bare}");
        assert_eq!(hint("ls -la", "total 0", None), None);
        assert_eq!(hint("npm run dev", DEV_SERVER, Some("bg-2")), None);
        assert_eq!(hint("curl https://example.com", "see https://example.com/docs for more", Some("bg-3")), None);
    }

    #[test]
    fn a_sign_in_card_never_shows_the_code() {
        let card = json!([{ "type": CARD, "tool": "gcloud" }]);
        assert_eq!(answer_as_shown(Some(&card), "4/0Secret".into()), CODE_ENTERED);
        for kept in [SKIP_SENTINEL, SIGNED_IN, "failed:The sign-in timed out."] {
            assert_eq!(answer_as_shown(Some(&card), kept.into()), kept);
        }
        let options = json!([{ "type": "options", "options": ["A"] }]);
        assert_eq!(answer_as_shown(Some(&options), "A".into()), "A");
        assert_eq!(answer_as_shown(None, "A".into()), "A");
    }

    #[test]
    fn the_code_is_its_first_line_without_control_characters() {
        assert_eq!(clean_code("  4/0AbC\u{1b}-x_9 \nrm -rf /\n").as_deref(), Some("4/0AbC-x_9"));
        assert_eq!(clean_code(" \n\t"), None);
    }

    #[test]
    fn a_typed_code_is_masked_whole_wrapped_or_split_across_reads() {
        let code = "4/0AVMBsJh-xYz_123456789";
        let mut m = Masks::default();
        assert_eq!(m.mask(&format!("before {code}")), format!("before {code}"), "nothing is masked before a code is typed");
        m.add(code);
        assert_eq!(m.mask(&format!("{code}\nYou are now logged in.\n")), "[code entered]\nYou are now logged in.\n");
        // Echoed in a box that wraps it.
        let boxed = m.mask(&format!("│ > {} │\n│ {} │\n", &code[..14], &code[14..]));
        assert!(!boxed.contains("Jh-xYz") && !boxed.contains("3456789"), "{boxed}");
        // Split across two reads: the first read's end is held, not shown.
        let a = m.mask(&format!("got {}", &code[..4]));
        let b = m.mask(&format!("{} ok", &code[4..]));
        assert_eq!(a + b.as_str(), "got [code entered] ok");
        // A read that ends in what could start the code is shown once the
        // session has nothing more to say.
        let short = m.mask("path 4/");
        assert_eq!(short + m.flush().as_str(), "path 4/");
        // A long piece still held when the session ends is masked, not shown.
        let cut = m.mask(&format!("got {}", &code[..10]));
        assert_eq!(cut + m.flush().as_str(), "got [code entered]");
    }

    #[test]
    fn a_short_code_is_masked_even_when_it_arrives_a_character_at_a_time() {
        let mut m = Masks::default();
        m.add("ABCD12");
        let shown: String = "Code: ABCD12\n".chars().map(|c| m.mask(&c.to_string())).collect();
        assert_eq!(shown + m.flush().as_str(), "Code: [code entered]\n");
    }
}

/// The whole hand-over, through the command tools, against stand-in command
/// line tools that print what real ones do and check the code they are given.
#[cfg(all(test, unix))]
mod flow {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::command_tools::{ReadOutputTool, RunCommandTool, SendInputTool};
    use crate::file_tools::Machine;
    use crate::origin::{AskChannels, Origin};
    use crate::process::ProcessRegistry;
    use crate::registry::DynTool;

    /// The owner's code; the stand-in tool accepts only this.
    const CODE: &str = "4/0AfakeOwnerCode-XYZ_12345";

    /// A stand-in login that shows `screen` (a real tool's sign-in screen),
    /// reads the code back, echoes it as some tools do, and writes whether
    /// it matched.
    fn paste_cli(dir: &std::path::Path, screen: &str) -> String {
        let (shown, result) = (dir.join("screen"), dir.join("result"));
        std::fs::write(&shown, screen).unwrap();
        script(
            dir,
            "fakecli",
            &format!(
                "cat '{s}'\n\
                 IFS= read -r code\n\
                 echo \"Received $code\"\n\
                 if [ \"$code\" = '{CODE}' ]; then echo match > '{r}'; echo 'You are now logged in as owner@example.com.'; \
                 else echo mismatch > '{r}'; echo 'Invalid code.'; exit 1; fi\n",
                s = shown.display(),
                r = result.display()
            ),
        )
    }

    /// A stand-in device login: shows `screen` (a real tool's device-code
    /// screen) and waits until the owner has signed in on the page
    /// (`approved` appears).
    fn device_cli(dir: &std::path::Path, screen: &str) -> String {
        let shown = dir.join("screen");
        std::fs::write(&shown, format!("{screen}\n")).unwrap();
        script(
            dir,
            "fakecli",
            &format!(
                "cat '{s}'\n\
                 while [ ! -f '{a}' ]; do sleep 0.1; done\n\
                 echo 'Authentication complete. Logged in as owner.'\n",
                s = shown.display(),
                a = dir.join("approved").display()
            ),
        )
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// The owner at the app, as the tools see them: the cards the run
    /// raises, and the one answer path.
    struct Owner {
        events: tokio::sync::mpsc::Receiver<ai::StreamEvent>,
        channels: AskChannels,
        /// Every event the run sent, for what the model or a client could see.
        seen: Vec<String>,
    }

    impl Owner {
        async fn next(&mut self, kind: ai::StreamEventType) -> ai::StreamEvent {
            loop {
                let e = tokio::time::timeout(std::time::Duration::from_secs(20), self.events.recv())
                    .await
                    .expect("the run said nothing")
                    .expect("the run's stream closed");
                self.seen.push(format!("{} {:?} {:?} {:?}", e.text, e.error, e.widgets, e.payload));
                if e.event_type == kind {
                    return e;
                }
            }
        }

        /// The next card: its request id and its widget.
        async fn card(&mut self) -> (String, Value) {
            let e = self.next(ai::StreamEventType::AskRequest).await;
            assert_eq!(e.text.split(" to ").next(), Some("Sign in"), "the card's heading: {}", e.text);
            (e.error.unwrap(), e.widgets.unwrap()[0].clone())
        }

        async fn answer(&self, id: &str, value: &str) {
            let tx = self.channels.lock().await.remove(id).expect("the card waits for an answer");
            tx.send(value.to_string()).unwrap();
        }
    }

    fn with_owner() -> (ToolContext, Owner) {
        let (tx, events) = tokio::sync::mpsc::channel(64);
        let channels: AskChannels = Default::default();
        let mut ctx = ToolContext::new(Origin::User).with_session("agent:it:thread:sign-in", "s1");
        ctx.stream_tx = Some(tx);
        ctx.ask_channels = Some(channels.clone());
        (ctx, Owner { events, channels, seen: Vec::new() })
    }

    fn machine() -> Arc<Machine> {
        Arc::new(Machine::new(Arc::new(ProcessRegistry::new()), None))
    }

    async fn start(m: &Arc<Machine>, ctx: &ToolContext, command: &str) -> String {
        let r = RunCommandTool(m.clone())
            .execute_dyn(ctx, json!({"command": command, "description": "Sign in", "background": true, "pty": true}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        r.content.split("**").nth(1).expect("session id between ** markers").to_string()
    }

    fn sign_in(m: &Arc<Machine>, ctx: ToolContext, id: &str, what: &str) -> tokio::task::JoinHandle<ToolResult> {
        let (m, input) = (m.clone(), json!({"task_id": id, "sign_in": what}));
        tokio::spawn(async move { SendInputTool(m).execute_dyn(&ctx, input).await })
    }

    async fn read_all(m: &Arc<Machine>, id: &str, raw: bool) -> String {
        let helpers = crate::command_tools::Helpers {
            orchestrator: crate::orchestrator::new_handle(),
            store: None,
            runs: None,
            workflows: Default::default(),
        };
        let read = ReadOutputTool { machine: m.clone(), helpers };
        read.execute_dyn(&ToolContext::new(Origin::User), json!({"task_id": id, "raw": raw})).await.content
    }

    /// Paste-back: the card carries the link and a field; the owner's code
    /// goes to the terminal, the tool takes it, and the code shows nowhere:
    /// not in the result the model reads, not in the session's output (raw
    /// included), not in anything the run sent.
    #[tokio::test]
    async fn the_owners_code_goes_to_the_terminal_and_nowhere_else() {
        use super::tests::{FIREBASE, GCLOUD, GEMINI};
        for (tool, screen) in [("Google Cloud", GCLOUD), ("Gemini CLI", GEMINI), ("Firebase", FIREBASE)] {
            the_owners_code_goes_to_the_terminal(tool, screen).await;
        }
    }

    async fn the_owners_code_goes_to_the_terminal(tool: &str, screen: &str) {
        let dir = tempfile::tempdir().unwrap();
        let cli = paste_cli(dir.path(), screen);
        let registry = Arc::new(ProcessRegistry::new());
        let m = Arc::new(Machine::new(registry.clone(), None));
        let (ctx, mut owner) = with_owner();
        let id = start(&m, &ctx, &cli).await;
        let call = sign_in(&m, ctx, &id, tool);

        let (request, card) = owner.card().await;
        assert_eq!(card["type"], CARD);
        assert_eq!(card["tool"], tool);
        assert_eq!(card["url"].as_str(), read(screen).map(|s| s.url).as_deref(), "{tool}");
        assert_eq!((card["input"].as_bool(), card["code"].is_null()), (Some(true), true), "{tool}");
        owner.answer(&request, &format!("  {CODE}\n")).await;

        let r = call.await.unwrap();
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("result")).unwrap().trim(), "match", "the tool got the owner's code");
        assert!(r.content.contains("was entered into") && r.content.contains("You are now logged in"), "{}", r.content);
        assert!(r.content.contains("Received [code entered]"), "the tool's own echo is masked: {}", r.content);
        // A raw read after a code went in is the plain text, masked.
        let raw = read_all(&m, &id, true).await;
        assert!(raw.contains("Received [code entered]") && !raw.contains('\x1b'), "{raw}");
        let whole = registry.get_any_session(&id).await.unwrap().get_output().await;
        for (what, text) in [("the result", r.content.as_str()), ("the whole output", whole.as_str()), ("the raw read", raw.as_str())] {
            assert!(!text.contains(CODE) && !text.contains("fakeOwnerCode"), "{what} shows the code: {text}");
        }
        while let Ok(e) = owner.events.try_recv() {
            owner.seen.push(format!("{} {:?} {:?}", e.text, e.error, e.widgets));
        }
        assert!(owner.seen.iter().all(|e| !e.contains(CODE)), "{tool}: a stream event carried the code: {:?}", owner.seen);
    }

    /// Device code: the card shows the code to type on the page and asks for
    /// nothing back; the login finishing settles the card as signed in.
    #[tokio::test]
    async fn a_device_code_sign_in_settles_when_the_login_finishes() {
        use super::tests::{AZURE, GH};
        for (tool, screen, url, code) in [
            ("GitHub CLI", GH, "https://github.com/login/device", "1A2B-3C4D"),
            ("Azure CLI", AZURE, "https://microsoft.com/devicelogin", "F7GHXQ2LN"),
        ] {
            a_device_code_sign_in_settles(tool, screen, url, code).await;
        }
    }

    async fn a_device_code_sign_in_settles(tool: &str, screen: &str, url: &str, code: &str) {
        let dir = tempfile::tempdir().unwrap();
        let cli = device_cli(dir.path(), screen);
        let m = machine();
        let (ctx, mut owner) = with_owner();
        let id = start(&m, &ctx, &cli).await;
        let call = sign_in(&m, ctx, &id, tool);

        let (request, card) = owner.card().await;
        assert_eq!(card["url"], url);
        assert_eq!((card["code"].as_str(), card["input"].as_bool()), (Some(code), Some(false)), "{tool}");
        std::fs::write(dir.path().join("approved"), "").unwrap();

        let settled = owner.next(ai::StreamEventType::AskSettled).await;
        assert_eq!((settled.error.as_deref(), settled.text.as_str()), (Some(request.as_str()), SIGNED_IN));
        let r = call.await.unwrap();
        assert!(!r.is_error && r.content.starts_with("The owner signed in"), "{}", r.content);
        assert!(r.content.contains("Authentication complete"), "{}", r.content);
    }

    /// Nobody signs in: the card is settled as timed out, the login is
    /// stopped, and the model hears so.
    #[tokio::test]
    async fn an_unanswered_sign_in_times_out_and_stops_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let cli = paste_cli(dir.path(), super::tests::GCLOUD);
        let registry = Arc::new(ProcessRegistry::new());
        let m = Arc::new(Machine::new(registry.clone(), None));
        let (ctx, mut owner) = with_owner();
        let id = start(&m, &ctx, &cli).await;
        let waits = Waits { owner: std::time::Duration::from_secs(1), ..Waits::default() };
        let (reg, sid) = (registry.clone(), id.clone());
        let call = tokio::spawn(async move { hand_over(&reg, &ctx, &sid, "Fakecloud", waits).await });

        owner.card().await;
        let settled = owner.next(ai::StreamEventType::AskSettled).await;
        assert_eq!(settled.text, format!("{CARD_FAILED_PREFIX}The sign-in timed out."));
        let r = call.await.unwrap();
        assert!(r.is_error && r.content.contains("didn't sign in to Fakecloud within 1 second"), "{}", r.content);
        let session = registry.get_any_session(&id).await.expect("kept for read_output");
        assert!(session.exited, "the login was stopped");
    }

    /// The owner taps Cancel: the login is stopped. A stopped turn stops it
    /// too.
    #[tokio::test]
    async fn a_cancelled_sign_in_stops_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let cli = paste_cli(dir.path(), super::tests::GCLOUD);
        let registry = Arc::new(ProcessRegistry::new());
        let m = Arc::new(Machine::new(registry.clone(), None));

        let (ctx, mut owner) = with_owner();
        let id = start(&m, &ctx, &cli).await;
        let call = sign_in(&m, ctx, &id, "Fakecloud");
        let (request, _) = owner.card().await;
        owner.answer(&request, SKIP_SENTINEL).await;
        let r = call.await.unwrap();
        assert!(!r.is_error && r.content.contains("The owner cancelled the sign-in"), "{}", r.content);
        assert!(registry.get_any_session(&id).await.is_none_or(|s| s.exited), "the login was stopped");

        let (ctx, mut owner) = with_owner();
        let turn = ctx.cancel_token.clone();
        let id = start(&m, &ctx, &cli).await;
        let call = sign_in(&m, ctx, &id, "Fakecloud");
        owner.card().await;
        turn.cancel();
        let r = call.await.unwrap();
        assert!(r.is_error && r.content.contains("The turn was stopped"), "{}", r.content);
        assert!(registry.get_any_session(&id).await.is_none_or(|s| s.exited), "the login was stopped with the turn");
    }

    /// A run nobody is at (a workflow) raises no card; a command without a
    /// terminal can't be handed over.
    #[tokio::test]
    async fn a_sign_in_needs_the_owner_and_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let cli = paste_cli(dir.path(), super::tests::GCLOUD);
        let m = machine();
        let (ctx, _owner) = with_owner();
        let id = start(&m, &ctx, &cli).await;
        let unattended = ToolContext::new(Origin::Workflow);
        let r = SendInputTool(m.clone()).execute_dyn(&unattended, json!({"task_id": id, "sign_in": "Fakecloud"})).await;
        assert!(r.is_error && r.content.contains("needs the owner in this conversation"), "{}", r.content);

        let piped = RunCommandTool(m.clone())
            .execute_dyn(&ctx, json!({"command": cli, "description": "Sign in", "background": true}))
            .await;
        let piped = piped.content.split("**").nth(1).unwrap().to_string();
        let r = SendInputTool(m.clone()).execute_dyn(&ctx, json!({"task_id": piped, "sign_in": "Fakecloud"})).await;
        assert!(r.is_error && r.content.contains("has no terminal"), "{}", r.content);
        let both = SendInputTool(m).validate_input(&json!({"task_id": id, "sign_in": "Fakecloud", "text": "x"}));
        assert!(both.is_err(), "sign_in stands alone in its call");
    }

    /// A login run in a terminal in the foreground doesn't sit out its
    /// timeout: once it shows a sign-in it moves to the background and the
    /// model is told to hand it over.
    #[tokio::test]
    async fn a_foreground_terminal_login_moves_to_the_background_at_its_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let cli = device_cli(dir.path(), super::tests::GH);
        let m = machine();
        let (ctx, _owner) = with_owner();
        let started = std::time::Instant::now();
        let r = RunCommandTool(m)
            .execute_dyn(&ctx, json!({"command": cli, "description": "Sign in", "pty": true}))
            .await;
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "it waited for its timeout");
        assert!(r.content.starts_with("Command is waiting on a sign-in and was moved to the background"), "{}", r.content);
        assert!(r.content.contains("sign_in: \"<what they sign in to>\""), "{}", r.content);
        std::fs::write(dir.path().join("approved"), "").unwrap();
    }
}
