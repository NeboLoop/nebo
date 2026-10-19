//! The person's default web browser: which one it is, and how to open a
//! link in a window of its own there. The ONE place Nebo asks the OS — the
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
    /// Chrome, Edge, Brave, Vivaldi, Opera (and GX), Chromium and its forks:
    /// `--app=<url>` opens a window with no tabs or toolbar, in the person's
    /// own profile.
    Chromium,
    /// Firefox and its forks: `-new-window <url>`, a window of its own, not a
    /// tab in theirs.
    Firefox,
    /// GNOME Web: `--new-window <url>`.
    Epiphany,
    /// Falkon: `--open-window=<url>`.
    Falkon,
    /// Safari has no command line: AppleScript makes it a new window and
    /// sizes it (macOS asks the person once to let Nebo control Safari).
    Safari,
    /// Arc opens links from other apps in Little Arc, its own small window,
    /// by default; it has no switch for one, so the ordinary open is it.
    Arc,
    /// No known way to ask for a window.
    Other,
}

/// The default browser, and the command that starts it.
#[derive(Debug, Clone)]
pub struct DefaultBrowser {
    /// Short name: chrome, edge, brave, vivaldi, opera, chromium, thorium,
    /// yandex, firefox, librewolf, waterfox, zen, floorp, epiphany, falkon,
    /// safari, arc — or `other`.
    pub name: &'static str,
    /// What the OS calls it: a bundle id (macOS), the executable's last path
    /// parts (Windows), a desktop entry and its program (Linux).
    pub id: String,
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
    let word = |w: &str| h.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
    // Order matters: Edge, Brave, Opera and the Chromium forks before the
    // "chrome" their names or vendors can contain.
    if h.contains("edge") {
        "edge"
    } else if h.contains("brave") {
        "brave"
    } else if h.contains("vivaldi") {
        "vivaldi"
    } else if h.contains("opera") {
        "opera"
    } else if h.contains("thorium") {
        "thorium"
    } else if h.contains("yandex") {
        "yandex"
    } else if h.contains("chromium") || h.contains("cromite") || h.contains("supermium") {
        "chromium"
    } else if h.contains("chrome") {
        "chrome"
    } else if h.contains("librewolf") {
        "librewolf"
    } else if h.contains("waterfox") {
        "waterfox"
    } else if h.contains("floorp") {
        "floorp"
    } else if word("zen") {
        "zen"
    } else if h.contains("firefox") || h.contains("mullvad") || h.contains("torbrowser") || h.contains("tor-browser") {
        "firefox"
    } else if h.contains("epiphany") {
        "epiphany"
    } else if h.contains("falkon") {
        "falkon"
    } else if h.contains("safari") {
        "safari"
    } else if h.contains("company.thebrowser.browser") || word("arc") {
        // Arc's bundle id; its sibling Dia (company.thebrowser.dia) is not Arc.
        "arc"
    } else {
        "other"
    }
}

fn family_of(name: &str) -> Family {
    match name {
        "chrome" | "edge" | "brave" | "vivaldi" | "opera" | "chromium" | "thorium" | "yandex" => Family::Chromium,
        "firefox" | "librewolf" | "waterfox" | "zen" | "floorp" => Family::Firefox,
        "epiphany" => Family::Epiphany,
        "falkon" => Family::Falkon,
        "safari" => Family::Safari,
        // Arc is built on Chromium but is its own app: whether it honours
        // `--app` is unconfirmed, and a browser that ignores the switch opens
        // nothing at all. Its own Little Arc is the window.
        "arc" => Family::Arc,
        _ => Family::Other,
    }
}

/// The arguments that open `url` in a window of its own, after the
/// browser's own command, or `None` when that browser is not started that
/// way (Safari, Arc, unknown).
fn window_args(family: Family, url: &str) -> Option<Vec<OsString>> {
    let (w, h) = SIGN_IN_WINDOW_SIZE;
    match family {
        Family::Chromium => Some(vec![format!("--app={url}").into(), format!("--window-size={w},{h}").into()]),
        Family::Firefox => Some(vec!["-new-window".into(), url.into()]),
        Family::Epiphany => Some(vec!["--new-window".into(), url.into()]),
        Family::Falkon => Some(vec![format!("--open-window={url}").into()]),
        Family::Safari | Family::Arc | Family::Other => None,
    }
}

/// Safari's new window: a new document at the URL, sized for a sign-in and
/// brought to the front. The URL and the browser's bundle id arrive as
/// arguments (`on run argv`), never as script text, so no link can change
/// what the script does. `using terms from` keeps Safari's vocabulary while
/// `application id` reaches the exact app that is the default (Safari or
/// Safari Technology Preview). `launch` first: a Safari that a `make` starts
/// opens its own start window after ours, and the resize lands on that one.
/// Only the `make` can fail the script: once the window is open, a size
/// Safari refuses must not send the caller on to open the link again.
const SAFARI_WINDOW_SCRIPT: &str = r#"on run argv
	set {theURL, appId, w, h} to argv
	using terms from application "Safari"
		tell application id appId
			launch
			make new document with properties {URL:theURL}
			try
				set {l, t} to bounds of front window
				set bounds of front window to {l, t, l + (w as integer), t + (h as integer)}
			end try
			activate
		end tell
	end using terms from
end run"#;

/// The `osascript` arguments for Safari's new window.
fn safari_args(bundle_id: &str, url: &str) -> Vec<OsString> {
    let (w, h) = SIGN_IN_WINDOW_SIZE;
    vec!["-e".into(), SAFARI_WINDOW_SCRIPT.into(), url.into(), bundle_id.into(), w.to_string().into(), h.to_string().into()]
}

/// Run Safari's new window and wait for it: `false` when macOS refused (the
/// person said no to "Nebo wants to control Safari") or it failed.
fn open_safari_window(browser: &DefaultBrowser, url: &str) -> bool {
    let out = command::new::<std::process::Command>("osascript", command::Console::Hidden)
        .args(safari_args(&browser.id, url))
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            info!(browser = browser.name, "opened a window in the default browser");
            true
        }
        Ok(o) => {
            warn!(browser = browser.name, error = %String::from_utf8_lossy(&o.stderr).trim(), "Safari did not open a window");
            false
        }
        Err(e) => {
            warn!(browser = browser.name, error = %e, "osascript failed to start");
            false
        }
    }
}

/// Open `url` in a window of its own in the default browser — the person's
/// real browser and profile, so a provider that refuses embedded web views
/// (Google) still signs them in, and closing the window closes the sign-in,
/// not a tab among theirs.
///
/// `false` when that can't be done here (not an http(s) link, no default
/// browser found, a browser with no such window, or it failed to start): the
/// caller opens the link the ordinary way — for Arc, that is Little Arc.
///
/// Blocking (Safari's waits on AppleScript, and on the person the first
/// time macOS asks them): call it off an async runtime's workers.
pub fn open_window(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "https" | "http") => {}
        _ => return false,
    }
    let Some(browser) = detect() else {
        return false;
    };
    let family = browser.family();
    if family == Family::Safari {
        return open_safari_window(&browser, url);
    }
    let Some(ours) = window_args(family, url) else {
        if family == Family::Arc {
            info!(browser = browser.name, "Arc opens links from other apps in Little Arc; opening the link plainly");
        } else {
            info!(browser = browser.name, id = %browser.id, "default browser has no window switch; opening the link plainly");
        }
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
        id: bundle_id,
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
    // The executable and the two folders above it: Opera's executable is
    // `launcher.exe` in `Opera GX`, Yandex's is `browser.exe` in
    // `YandexBrowser\Application`. Never higher — the user folder's name is
    // the person's, not the browser's.
    let parts: Vec<String> = exe.with_extension("").iter().map(|c| c.to_string_lossy().into_owned()).collect();
    let id = parts[parts.len().saturating_sub(3)..].join(" ");
    Some(DefaultBrowser { name: identify(&id), id, program: exe.into_os_string(), args: Vec::new() })
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
    let id = format!("{desktop_id} {base}");
    Some(DefaultBrowser {
        name: identify(&id),
        id,
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
        assert_eq!(identify("company.thebrowser.dia"), "other");
        assert_eq!(identify("com.apple.SafariTechnologyPreview"), "safari");
        assert_eq!(identify("app.zen-browser.zen"), "zen");
        assert_eq!(identify("org.mozilla.librewolf"), "librewolf");
        assert_eq!(identify("org.torproject.torbrowser"), "firefox");
        assert_eq!(identify("com.kagi.kagimacOS"), "other");
        // Windows: the executable and the two folders above it
        assert_eq!(identify("Chrome Application chrome"), "chrome");
        assert_eq!(identify("Edge Application msedge"), "edge");
        assert_eq!(identify("Brave-Browser Application brave"), "brave");
        assert_eq!(identify("Programs Opera launcher"), "opera");
        assert_eq!(identify("Programs Opera GX launcher"), "opera");
        assert_eq!(identify("Vivaldi Application vivaldi"), "vivaldi");
        assert_eq!(identify("YandexBrowser Application browser"), "yandex");
        assert_eq!(identify("Thorium Application thorium"), "thorium");
        assert_eq!(identify("Programs Mozilla Firefox firefox"), "firefox");
        assert_eq!(identify("Programs LibreWolf librewolf"), "librewolf");
        assert_eq!(identify("WindowsApps TheBrowserCompany.Arc_1.0.0.0_x64__ttt1ap7aakyb4 Arc"), "arc");
        // Linux: desktop id + program
        assert_eq!(identify("google-chrome.desktop google-chrome-stable"), "chrome");
        assert_eq!(identify("microsoft-edge.desktop microsoft-edge-stable"), "edge");
        assert_eq!(identify("firefox_firefox.desktop env"), "firefox");
        assert_eq!(identify("org.gnome.Epiphany.desktop epiphany"), "epiphany");
        assert_eq!(identify("org.kde.falkon.desktop falkon"), "falkon");
        assert_eq!(identify("waterfox.desktop waterfox"), "waterfox");
        assert_eq!(identify("one.ablaze.floorp.desktop flatpak"), "floorp");
        assert_eq!(identify("org.qutebrowser.qutebrowser.desktop qutebrowser"), "other");
    }

    #[test]
    fn only_browsers_with_a_window_of_their_own_get_one() {
        let url = "https://accounts.google.com/o/oauth2/auth?a=1&b=2";
        assert_eq!(
            window_args(family_of("edge"), url).unwrap(),
            vec![OsString::from(format!("--app={url}")), OsString::from("--window-size=520,720")]
        );
        assert_eq!(window_args(family_of("yandex"), url).unwrap()[0], OsString::from(format!("--app={url}")));
        assert_eq!(window_args(family_of("firefox"), url).unwrap(), vec![OsString::from("-new-window"), OsString::from(url)]);
        assert_eq!(window_args(family_of("zen"), url).unwrap(), vec![OsString::from("-new-window"), OsString::from(url)]);
        assert_eq!(window_args(family_of("epiphany"), url).unwrap(), vec![OsString::from("--new-window"), OsString::from(url)]);
        assert_eq!(window_args(family_of("falkon"), url).unwrap(), vec![OsString::from(format!("--open-window={url}"))]);
        // Safari's window is AppleScript's, Arc's is Little Arc.
        assert!(window_args(family_of("safari"), url).is_none());
        assert!(window_args(family_of("arc"), url).is_none());
        assert!(window_args(family_of("other"), url).is_none());
    }

    #[test]
    fn safari_gets_the_link_as_an_argument_never_as_script() {
        let url = r#"https://x.test/?q="end tell" & do shell script "id""#;
        let args = safari_args("com.apple.Safari", url);
        assert_eq!(args[0], OsString::from("-e"));
        assert_eq!(args[1], OsString::from(SAFARI_WINDOW_SCRIPT));
        assert_eq!(&args[2..], &[url, "com.apple.Safari", "520", "720"].map(OsString::from));
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
