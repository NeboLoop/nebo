//! Linux: the engine as a `systemd --user` unit.
//!
//! The app writes `~/.config/systemd/user/nebo-engine.service` itself (the
//! AppImage and the .deb alike). `ExecStart` is the executable where it
//! stays: `$APPIMAGE`, never the `/tmp/.mount_*` the AppImage runtime runs
//! from, or the .deb's fixed path. The unit carries the session's `PATH`
//! (without the AppImage's own directories) so the CLIs agents run resolve
//! as in a terminal. `Restart=on-failure` starts the engine again after a
//! crash, a stall (exit 70) or a held port (75); exit 0 (Quit, an update,
//! logout's SIGTERM) leaves it down. Nothing slows it: no `Nice=`,
//! `CPUQuota=` or idle I/O class. `WantedBy=default.target` starts it when
//! the owner logs in.
//!
//! **Display.** A user unit doesn't inherit the desktop session's
//! environment, and with no `DISPLAY`/`WAYLAND_DISPLAY` the engine counts as
//! headless and refuses desktop tools (`tools::server_mode`). Every start
//! from the app imports the session's display, D-Bus and `XDG_*` variables
//! into the user manager first. At login the unit can start before the
//! session has exported its display, so a login entry
//! (`~/.config/autostart/nebo-engine.desktop`) runs `nebo --engine-service
//! login` inside the session: an engine that came up without a display is
//! started again with it, when it is idle (and the kill switch, when the
//! feed turned the service off, removes the unit there).
//!
//! **Logout.** The user manager stops with the owner's last session, and the
//! engine with it (SIGTERM, exit 0). "Keep running after I log out" turns on
//! `loginctl enable-linger` for the user: only from Settings, never silently.
//!
//! Re-registered when the app moved: a unit for another executable counts
//! as not registered ([`status`]), so the app writes it again at open.

use super::*;

/// The session variables a desktop tool needs, imported into the user
/// manager before the engine starts.
const SESSION_VARS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
];

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = command::new::<std::process::Command>(program, command::Console::Hidden)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("{program} {}: {}", args.join(" "), out.status) } else { err })
    }
}

/// `systemctl --user <args>`.
fn systemctl(args: &[&str]) -> Result<String, String> {
    run("systemctl", &[&["--user"], args].concat())
}

/// A systemd user manager answers this user.
fn available() -> bool {
    systemctl(&["show-environment"]).is_ok()
}

/// The unit's name: the app's is `nebo-engine.service`, a test's follows its
/// label.
pub(super) fn unit(target: &Target) -> String {
    let name = target.label.strip_prefix(LABEL).map(|rest| format!("nebo-engine{rest}")).unwrap_or_else(|| target.label.clone());
    format!("{name}.service")
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn unit_path(target: &Target) -> PathBuf {
    config_home().join("systemd/user").join(unit(target))
}

fn autostart_path(target: &Target) -> PathBuf {
    let unit = unit(target);
    config_home().join("autostart").join(format!("{}.desktop", unit.trim_end_matches(".service")))
}

/// The executable a unit may start: where it stays, never an AppImage's
/// temporary mount.
fn stable_exe() -> Result<PathBuf, String> {
    let exe = server::process::stable_exe().map_err(|e| format!("this executable: {e}"))?;
    if exe.to_string_lossy().contains("/.mount_") {
        return Err(format!("{} is an AppImage's temporary mount ($APPIMAGE is not set)", exe.display()));
    }
    Ok(exe)
}

/// One value for a unit file: double-quoted, `\` and `"` escaped, `%` (a
/// specifier) doubled.
fn unit_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%"))
}

/// The `ExecStart=` line for `exe`.
fn exec_start(exe: &Path) -> String {
    format!("ExecStart={} --engine", unit_quote(&exe.to_string_lossy()).replace('$', "$$"))
}

/// The unit. `path` is the session's `PATH`; `env` adds a test's port and
/// Nebo folder.
pub(super) fn render_unit(exe: &Path, path: &str, target: &Target, env: &[(&str, String)]) -> String {
    let mut vars = vec![
        "NEBO_SUPERVISED=systemd".to_string(),
        format!("NEBO_SERVICE_UNIT={}", unit(target)),
        format!("PATH={path}"),
    ];
    vars.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    let vars: String = vars.iter().map(|e| format!("Environment={}\n", unit_quote(e))).collect();
    format!(
        "# Written by Nebo, and again when the app moves. Start at login in Nebo's Settings removes it.\n\
         [Unit]\n\
         Description=Nebo engine\n\
         After=graphical-session.target\n\
         StartLimitIntervalSec=300\n\
         StartLimitBurst=20\n\
         \n\
         [Service]\n\
         Type=simple\n\
         {exec}\n\
         {vars}\
         Restart=on-failure\n\
         RestartSec=3\n\
         RestartPreventExitStatus=0\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exec = exec_start(exe),
    )
}

/// One argument of a desktop entry's `Exec`: quoted, with `"`, `` ` ``, `$`
/// and `\` escaped (a `\` twice over: the string escape applies first), and
/// `%` doubled.
fn exec_quote(s: &str) -> String {
    let mut quoted = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The login entry: inside the session, once its display is up, it brings
/// the display to the engine (`--engine-service login`). No window.
pub(super) fn render_autostart(exe: &Path, target: &Target) -> String {
    let mut args = String::new();
    if target.label != LABEL {
        args += &format!(" --label {}", exec_quote(&target.label));
    }
    if target.port != 27895 {
        args += &format!(" --port {}", target.port);
    }
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Nebo\n\
         Comment=Keeps Nebo working in the background\n\
         Exec={} --engine-service login{args}\n\
         NoDisplay=true\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quote(&exe.to_string_lossy()),
    )
}

/// The session's `PATH` for the unit, without the directories an AppImage's
/// runtime put in front of it (its `$APPDIR`, a `/tmp/.mount_*` gone once
/// this process exits).
pub(super) fn session_path(path: &str, appdir: Option<&str>) -> String {
    path.split(':')
        .filter(|dir| !dir.is_empty() && !dir.contains("/.mount_"))
        .filter(|dir| !appdir.is_some_and(|a| !a.is_empty() && dir.starts_with(a)))
        .collect::<Vec<_>>()
        .join(":")
}

/// Write `contents` to `path` unless it already holds exactly that. Returns
/// whether it changed.
fn write_if_changed(path: &Path, contents: &str) -> Result<bool, String> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

/// Bring this session's display, D-Bus and `XDG_*` variables to the user
/// manager, so the engine it starts has them. Only the ones set here.
fn import_environment() {
    let vars: Vec<&str> = SESSION_VARS.iter().copied().filter(|v| set(v)).collect();
    if !vars.is_empty()
        && let Err(e) = systemctl(&[&["import-environment"], vars.as_slice()].concat())
    {
        tracing::warn!(error = %e, "could not bring the session's display to the engine");
    }
}

/// Whether an environment (`has(name)`) has a display a desktop tool can use.
fn has_display(has: impl Fn(&str) -> bool) -> bool {
    has("DISPLAY") || has("WAYLAND_DISPLAY")
}

/// Whether the running engine `pid` has a display.
fn engine_has_display(pid: u32) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else { return true };
    let vars: Vec<&[u8]> = environ.split(|b| *b == 0).collect();
    has_display(|name| vars.iter().any(|v| v.len() > name.len() + 1 && v.starts_with(format!("{name}=").as_bytes())))
}

/// The test's port and Nebo folder for the unit, as the macOS legacy plist
/// carries them.
fn unit_env(target: &Target) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if target.port != 27895 {
        env.push(("NEBO_PORT", target.port.to_string()));
    }
    if let Some(home) = std::env::var("NEBO_HOME").ok().filter(|h| !h.is_empty()) {
        env.push(("NEBO_HOME", home));
    }
    env
}

pub fn status(target: &Target) -> Status {
    if !available() {
        return Status::Unavailable;
    }
    let Ok(body) = std::fs::read_to_string(unit_path(target)) else { return Status::NotRegistered };
    // A unit for another executable (the app moved, or another copy wrote
    // it) is this app's to write again.
    let ours = stable_exe().is_ok_and(|exe| body.lines().any(|l| l == exec_start(&exe)));
    match systemctl(&["is-enabled", &unit(target)]).as_deref() {
        // Masked by hand: kept from running, so the engine is the app's child.
        Err(e) if e.starts_with("masked") => Status::Unavailable,
        Ok("masked" | "masked-runtime") => Status::Unavailable,
        Ok("enabled") if ours => Status::Enabled,
        _ => Status::NotRegistered,
    }
}

/// Write the unit (again, when the app moved), enable it, add the login
/// entry, and start it with the session's display.
pub fn install(target: &Target) -> Result<Status, String> {
    if !available() {
        return Ok(Status::Unavailable);
    }
    let exe = stable_exe()?;
    let path = session_path(&std::env::var("PATH").unwrap_or_default(), std::env::var("APPDIR").ok().as_deref());
    if write_if_changed(&unit_path(target), &render_unit(&exe, &path, target, &unit_env(target)))? {
        tracing::info!(unit = %unit_path(target).display(), exe = %exe.display(), "engine unit written");
        systemctl(&["daemon-reload"])?;
    }
    systemctl(&["enable", &unit(target)])?;
    write_if_changed(&autostart_path(target), &render_autostart(&exe, target))?;
    kickstart(target, false)?;
    Ok(status(target))
}

/// Stop the engine (SIGTERM: its graceful path), and remove the unit and
/// the login entry. Lingering Nebo turned on goes too.
pub fn unregister(target: &Target) -> Result<(), String> {
    if !available() {
        return Ok(());
    }
    let _ = systemctl(&["stop", &unit(target)]);
    let _ = systemctl(&["disable", &unit(target)]);
    for path in [unit_path(target), autostart_path(target)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    let _ = systemctl(&["daemon-reload"]);
    let _ = systemctl(&["reset-failed", &unit(target)]);
    if target.label == LABEL && linger_marker().is_some_and(|m| m.exists()) {
        set_keep_after_logout(false)?;
    }
    Ok(())
}

/// Start the engine with the session's display; with `restart`, stop a
/// running one first. Without `restart`, an engine that runs without a
/// display while this session has one is started again, when it is idle.
pub fn kickstart(target: &Target, restart: bool) -> Result<(), String> {
    import_environment();
    let unit = unit(target);
    if restart {
        return systemctl(&["restart", &unit]).map(drop);
    }
    if let Job::Running(pid) = job(target) {
        if has_display(set) && !engine_has_display(pid) {
            if crate::engine::engine_idle(target.port) {
                tracing::info!("the engine started without the desktop's display; starting it again with it");
                return systemctl(&["restart", &unit]).map(drop);
            }
            tracing::warn!("the engine has no display and is working; it gets one at its next start");
        }
        return Ok(());
    }
    // A unit that hit its start limit stays failed until reset.
    let _ = systemctl(&["reset-failed", &unit]);
    systemctl(&["start", &unit]).map(drop)
}

pub fn job(target: &Target) -> Job {
    if !unit_path(target).exists() {
        return Job::Absent;
    }
    let active = systemctl(&["is-active", &unit(target)]);
    let pid = systemctl(&["show", "-p", "MainPID", "--value", &unit(target)]).ok().and_then(|p| p.parse::<u32>().ok());
    match (active.as_deref(), pid) {
        (Ok("active" | "activating" | "reloading"), Some(pid)) if pid != 0 => Job::Running(pid),
        _ => Job::Loaded,
    }
}

/// No Login Items pane on Linux: Start at login lives in Nebo's Settings.
pub fn open_login_items() {}

/// This user, for `loginctl`: its name, else its uid.
fn user() -> String {
    use std::os::unix::fs::MetadataExt;
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::fs::metadata("/proc/self").ok().map(|m| m.uid().to_string()))
        .unwrap_or_default()
}

/// Marks lingering Nebo turned on, so removing Nebo's unit turns it off
/// again; lingering the owner had before stays.
fn linger_marker() -> Option<PathBuf> {
    config::data_dir().ok().map(|d| d.join("linger-by-nebo"))
}

/// Whether the engine keeps running after the owner logs out.
pub fn keep_after_logout() -> Option<bool> {
    available().then(|| run("loginctl", &["show-user", &user(), "-p", "Linger", "--value"]).as_deref() == Ok("yes"))
}

/// "Keep running after I log out": `loginctl enable-linger` for this user.
pub fn set_keep_after_logout(on: bool) -> Result<(), String> {
    if on {
        if keep_after_logout() != Some(true) {
            run("loginctl", &["enable-linger"])?;
            if let Some(marker) = linger_marker() {
                let _ = std::fs::write(marker, "");
            }
        }
    } else {
        run("loginctl", &["disable-linger"])?;
        if let Some(marker) = linger_marker() {
            let _ = std::fs::remove_file(marker);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Target {
        Target { label: LABEL.into(), port: 27895 }
    }

    #[test]
    fn the_unit_restarts_on_failure_and_never_slows_the_engine() {
        let unit = render_unit(Path::new("/home/a/Applications/Nebo.AppImage"), "/usr/bin:/bin", &app(), &[]);
        assert!(unit.contains("ExecStart=\"/home/a/Applications/Nebo.AppImage\" --engine\n"), "{unit}");
        for line in [
            "Restart=on-failure",
            "RestartSec=3",
            "RestartPreventExitStatus=0",
            "WantedBy=default.target",
            "Environment=\"NEBO_SUPERVISED=systemd\"",
            "Environment=\"NEBO_SERVICE_UNIT=nebo-engine.service\"",
            "Environment=\"PATH=/usr/bin:/bin\"",
        ] {
            assert!(unit.lines().any(|l| l == line), "{line} missing from {unit}");
        }
        for slow in ["Nice=", "CPUQuota=", "IOSchedulingClass", "CPUSchedulingPolicy"] {
            assert!(!unit.contains(slow), "{slow} in {unit}");
        }
        // The start limit is a [Unit] setting.
        assert!(unit.split("[Service]").next().unwrap().contains("StartLimitBurst=20"));
    }

    #[test]
    fn a_test_unit_has_its_own_name_port_and_folder() {
        let test = Target { label: format!("{LABEL}.test"), port: 37895 };
        assert_eq!(unit(&test), "nebo-engine.test.service");
        assert_eq!(unit(&app()), "nebo-engine.service");
        let body = render_unit(Path::new("/opt/nebo"), "/usr/bin", &test, &[("NEBO_PORT", "37895".into()), ("NEBO_HOME", "/tmp/a b".into())]);
        assert!(body.contains("Environment=\"NEBO_PORT=37895\"\n"));
        assert!(body.contains("Environment=\"NEBO_HOME=/tmp/a b\"\n"));
        let entry = render_autostart(Path::new("/opt/nebo"), &test);
        assert!(entry.contains("Exec=\"/opt/nebo\" --engine-service login --label \"dev.neboai.nebo.engine.test\" --port 37895\n"), "{entry}");
        assert!(render_autostart(Path::new("/opt/nebo"), &app()).contains("Exec=\"/opt/nebo\" --engine-service login\n"));
    }

    #[test]
    fn odd_paths_are_quoted_for_systemd_and_desktop_entries() {
        let exe = Path::new("/home/a/My Apps/100%/Ne\"bo$.AppImage");
        let unit = render_unit(exe, "/a%b", &app(), &[]);
        assert!(unit.contains("ExecStart=\"/home/a/My Apps/100%%/Ne\\\"bo$$.AppImage\" --engine\n"), "{unit}");
        assert!(unit.contains("Environment=\"PATH=/a%%b\"\n"));
        let entry = render_autostart(exe, &app());
        assert!(entry.contains("Exec=\"/home/a/My Apps/100%%/Ne\\\"bo\\$.AppImage\" --engine-service login\n"), "{entry}");
    }

    #[test]
    fn the_unit_path_leaves_out_the_appimage_mount() {
        let path = "/tmp/.mount_NeboAb/usr/bin/:/tmp/.mount_NeboAb/bin/:/opt/app/usr/bin:/usr/local/bin:/usr/bin::/bin";
        assert_eq!(session_path(path, Some("/opt/app")), "/usr/local/bin:/usr/bin:/bin");
        assert_eq!(session_path("/usr/bin:/bin", None), "/usr/bin:/bin");
    }

    #[test]
    fn a_display_is_x11_or_wayland() {
        assert!(has_display(|v| v == "DISPLAY"));
        assert!(has_display(|v| v == "WAYLAND_DISPLAY"));
        assert!(!has_display(|v| v == "DBUS_SESSION_BUS_ADDRESS"));
        // This process: the reader agrees with its own environment.
        assert_eq!(engine_has_display(std::process::id()), has_display(set));
    }
}
