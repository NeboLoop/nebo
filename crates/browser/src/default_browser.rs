//! The person's default web browser: which one it is, and how to start it
//! with arguments of our own. The ONE place Nebo asks the OS — the
//! extension bridge's routing and the desktop app's sign-in window both read
//! it here.
//!
//! Each OS answers from its own record of the default `https` handler:
//! LaunchServices on macOS (`NSWorkspace`), the shell's association on
//! Windows (`AssocQueryStringW`, which honours the user's `UserChoice`), and
//! `xdg-settings` on Linux.

use std::ffi::OsString;

use tracing::{info, warn};

/// The size of a sign-in window, in logical pixels: what a provider's own
/// sign-in popup is laid out for.
const SIGN_IN_WINDOW_SIZE: (u32, u32) = (520, 720);

/// How a browser takes "open this in its own window".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Chrome, Edge, Brave, Vivaldi, Opera, Chromium: `--app=<url>` opens a
    /// window with no tabs or toolbar, in the person's own profile.
    Chromium,
    /// `-new-window <url>`: a window of its own, not a tab in theirs.
    Firefox,
    /// No way to ask for a window (Safari, Arc, anything unknown).
    Other,
}

/// The default browser, and the command that starts it.
#[derive(Debug, Clone)]
pub struct DefaultBrowser {
    /// Short name: chrome, edge, brave, vivaldi, opera, chromium, firefox,
    /// safari, arc — or `other`.
    pub name: &'static str,
    /// The program to run, and the arguments that come before ours.
    program: OsString,
    args: Vec<OsString>,
}

impl DefaultBrowser {
    pub fn family(&self) -> Family {
        family_of(self.name)
    }
}

/// The short name for a browser, from what the OS calls it: a bundle id
/// (macOS), an executable and its folder (Windows), a desktop entry and its
/// program (Linux).
fn identify(hint: &str) -> &'static str {
    let h = hint.to_lowercase();
    // Order matters: Edge, Brave, Opera and Chromium before the "chrome"
    // their names or vendors can contain.
    if h.contains("edge") {
        "edge"
    } else if h.contains("brave") {
        "brave"
    } else if h.contains("vivaldi") {
        "vivaldi"
    } else if h.contains("opera") {
        "opera"
    } else if h.contains("chromium") {
        "chromium"
    } else if h.contains("chrome") {
        "chrome"
    } else if h.contains("firefox") {
        "firefox"
    } else if h.contains("safari") {
        "safari"
    } else if h.contains("thebrowser") || h.split(|c: char| !c.is_alphanumeric()).any(|w| w == "arc") {
        "arc"
    } else {
        "other"
    }
}

fn family_of(name: &str) -> Family {
    match name {
        "chrome" | "edge" | "brave" | "vivaldi" | "opera" | "chromium" => Family::Chromium,
        "firefox" => Family::Firefox,
        // Arc is built on Chromium but is its own app: whether it honours
        // `--app` is unconfirmed, and a browser that ignores the switch opens
        // nothing at all. It takes the plain open instead.
        _ => Family::Other,
    }
}

/// The arguments that open `url` in a window of its own, or `None` when the
/// browser has no way to ask for one.
fn window_args(family: Family, url: &str) -> Option<Vec<OsString>> {
    let (w, h) = SIGN_IN_WINDOW_SIZE;
    match family {
        Family::Chromium => Some(vec![format!("--app={url}").into(), format!("--window-size={w},{h}").into()]),
        Family::Firefox => Some(vec!["-new-window".into(), url.into()]),
        Family::Other => None,
    }
}

/// Open `url` in a window of its own in the default browser — the person's
/// real browser and profile, so a provider that refuses embedded web views
/// (Google) still signs them in, and closing the window closes the sign-in,
/// not a tab among theirs.
///
/// `false` when that can't be done here (not an http(s) link, no default
/// browser found, a browser with no such window, or it failed to start): the
/// caller opens the link the ordinary way.
pub fn open_window(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "https" | "http") => {}
        _ => return false,
    }
    let Some(browser) = detect() else {
        return false;
    };
    let Some(ours) = window_args(browser.family(), url) else {
        info!(browser = browser.name, "default browser has no app window; opening the link plainly");
        return false;
    };
    let mut cmd = command::new::<std::process::Command>(&browser.program, command::Console::Detached);
    cmd.args(&browser.args)
        .args(ours)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            info!(browser = browser.name, "opened a window in the default browser");
            // Reaped off the caller's thread: a browser that wasn't running
            // yet lives on as this child until the person closes it.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(e) => {
            warn!(browser = browser.name, error = %e, "default browser failed to start");
            false
        }
    }
}

/// The default browser, or `None` when the OS doesn't say. Blocking (it
/// may run a command): call it off an async runtime's workers.
pub fn detect() -> Option<DefaultBrowser> {
    detect_os()
}

#[cfg(target_os = "macos")]
fn detect_os() -> Option<DefaultBrowser> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSString, NSURL};

    let probe = NSURL::URLWithString(&NSString::from_str("https://neboai.com"))?;
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&probe)?;
    let path = app.path()?.to_string();
    let bundle_id = NSBundle::bundleWithURL(&app)?.bundleIdentifier()?.to_string();
    Some(DefaultBrowser {
        name: identify(&bundle_id),
        // `-n`: a new launch, so the arguments reach a browser that is
        // already running (Chromium hands them to its open instance).
        program: "open".into(),
        args: vec!["-n".into(), "-a".into(), path.into(), "--args".into()],
    })
}

#[cfg(target_os = "windows")]
fn detect_os() -> Option<DefaultBrowser> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::UI::Shell::{ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, AssocQueryStringW};

    let wide = |s: &str| std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (scheme, verb) = (wide("https"), wide("open"));
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: both inputs are NUL-terminated UTF-16; `buf` holds `len` u16s
    // and the call writes at most that many, reporting the count in `len`.
    let hr = unsafe {
        AssocQueryStringW(ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, scheme.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len)
    };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
    let exe = std::path::PathBuf::from(OsString::from_wide(&buf[..end]));
    if !exe.is_file() {
        return None;
    }
    // Opera's executable is `launcher.exe`: its folder names it.
    let folder = exe.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = exe.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(DefaultBrowser { name: identify(&format!("{folder} {stem}")), program: exe.into_os_string(), args: Vec::new() })
}

#[cfg(target_os = "linux")]
fn detect_os() -> Option<DefaultBrowser> {
    let out = command::new::<std::process::Command>("xdg-settings", command::Console::Hidden)
        .args(["get", "default-web-browser"])
        .output()
        .ok()?;
    let desktop_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || desktop_id.is_empty() {
        return None;
    }
    let entry = desktop_entry_dirs()
        .into_iter()
        .map(|d| d.join("applications").join(&desktop_id))
        .find_map(|p| std::fs::read_to_string(p).ok())?;
    let mut tokens = exec_tokens(&desktop_exec(&entry)?);
    if tokens.is_empty() {
        return None;
    }
    let program = tokens.remove(0);
    let base = std::path::Path::new(&program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(DefaultBrowser {
        name: identify(&format!("{desktop_id} {base}")),
        program: program.into(),
        args: tokens.into_iter().map(OsString::from).collect(),
    })
}

/// Where desktop entries live, most specific first (XDG Base Directory).
#[cfg(target_os = "linux")]
fn desktop_entry_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var("XDG_DATA_HOME") {
        Ok(d) if !d.is_empty() => dirs.push(d.into()),
        _ => {
            if let Ok(home) = std::env::var("HOME") {
                dirs.push(std::path::Path::new(&home).join(".local/share"));
            }
        }
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty());
    dirs.extend(data_dirs.as_deref().unwrap_or("/usr/local/share:/usr/share").split(':').filter(|d| !d.is_empty()).map(Into::into));
    dirs
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn detect_os() -> Option<DefaultBrowser> {
    None
}

/// The `Exec=` of a desktop entry's `[Desktop Entry]` group.
#[cfg(any(target_os = "linux", test))]
fn desktop_exec(entry: &str) -> Option<String> {
    let mut in_main = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
        } else if in_main && let Some(exec) = line.strip_prefix("Exec=") {
            return Some(exec.to_string());
        }
    }
    None
}

/// An `Exec=` line's arguments (Desktop Entry spec): split on spaces outside
/// double quotes, backslash escapes inside them, field codes (`%u`, `%U`…)
/// and Flatpak's `@@u`/`@@` markers dropped — ours go on the end.
#[cfg(any(target_os = "linux", test))]
fn exec_tokens(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(cur);
    }
    tokens.retain(|t| {
        let field_code = t.len() == 2 && t.starts_with('%');
        !field_code && t != "@@u" && t != "@@"
    });
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_every_browser_from_what_each_os_calls_it() {
        // macOS bundle ids
        assert_eq!(identify("com.google.Chrome"), "chrome");
        assert_eq!(identify("com.google.chrome.canary"), "chrome");
        assert_eq!(identify("com.microsoft.edgemac"), "edge");
        assert_eq!(identify("com.brave.browser"), "brave");
        assert_eq!(identify("com.vivaldi.Vivaldi"), "vivaldi");
        assert_eq!(identify("com.operasoftware.Opera"), "opera");
        assert_eq!(identify("org.chromium.Chromium"), "chromium");
        assert_eq!(identify("org.mozilla.firefox"), "firefox");
        assert_eq!(identify("com.apple.Safari"), "safari");
        assert_eq!(identify("company.thebrowser.Browser"), "arc");
        // Windows: folder + executable
        assert_eq!(identify("Application chrome"), "chrome");
        assert_eq!(identify("Application msedge"), "edge");
        assert_eq!(identify("Application brave"), "brave");
        assert_eq!(identify("Opera launcher"), "opera");
        assert_eq!(identify("Mozilla Firefox firefox"), "firefox");
        // Linux: desktop id + program
        assert_eq!(identify("google-chrome.desktop google-chrome-stable"), "chrome");
        assert_eq!(identify("microsoft-edge.desktop microsoft-edge-stable"), "edge");
        assert_eq!(identify("firefox_firefox.desktop env"), "firefox");
        assert_eq!(identify("org.gnome.Epiphany.desktop epiphany"), "other");
    }

    #[test]
    fn only_browsers_with_a_window_of_their_own_get_one() {
        let url = "https://accounts.google.com/o/oauth2/auth?a=1&b=2";
        assert_eq!(
            window_args(family_of("edge"), url).unwrap(),
            vec![OsString::from(format!("--app={url}")), OsString::from("--window-size=520,720")]
        );
        assert_eq!(window_args(family_of("firefox"), url).unwrap(), vec![OsString::from("-new-window"), OsString::from(url)]);
        assert!(window_args(family_of("safari"), url).is_none());
        assert!(window_args(family_of("arc"), url).is_none());
        assert!(window_args(family_of("other"), url).is_none());
    }

    #[test]
    fn never_opens_a_link_that_is_not_a_web_page() {
        assert!(!open_window("javascript:alert(1)"));
        assert!(!open_window("file:///etc/passwd"));
        assert!(!open_window("--app=https://x.test"));
    }

    /// Every Mac has a default https handler (Safari when never changed).
    #[cfg(target_os = "macos")]
    #[test]
    fn launch_services_names_the_default() {
        let b = detect().expect("LaunchServices names an https handler");
        let path = b.args.get(2).expect("open -n -a <app> --args");
        assert!(std::path::Path::new(path).exists(), "{path:?}");
        eprintln!("default browser: {} ({path:?})", b.name);
    }

    #[test]
    fn reads_the_main_groups_exec_line() {
        let entry = "[Desktop Entry]\nName=Chrome\nExec=/usr/bin/google-chrome-stable %U\n\n[Desktop Action new-window]\nExec=/usr/bin/google-chrome-stable --new-window\n";
        assert_eq!(desktop_exec(entry).as_deref(), Some("/usr/bin/google-chrome-stable %U"));
        let action_first = "[Desktop Action x]\nExec=wrong\n[Desktop Entry]\nExec=right %u\n";
        assert_eq!(desktop_exec(action_first).as_deref(), Some("right %u"));
    }

    #[test]
    fn splits_exec_lines_and_drops_field_codes() {
        assert_eq!(exec_tokens("/usr/bin/google-chrome-stable %U"), vec!["/usr/bin/google-chrome-stable"]);
        assert_eq!(
            exec_tokens("env BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop /snap/bin/firefox %u"),
            vec!["env", "BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop", "/snap/bin/firefox"]
        );
        assert_eq!(
            exec_tokens("/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=brave --file-forwarding com.brave.Browser @@u %U @@"),
            vec!["/usr/bin/flatpak", "run", "--branch=stable", "--arch=x86_64", "--command=brave", "--file-forwarding", "com.brave.Browser"]
        );
        assert_eq!(exec_tokens(r#""/opt/My Browser/browser" --flag "a \"q\"" %U"#), vec!["/opt/My Browser/browser", "--flag", r#"a "q""#]);
    }
}
