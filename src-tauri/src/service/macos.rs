//! macOS: the engine as a per-user LaunchAgent. `SMAppService` on macOS 13
//! and later (the plist inside the app); the legacy `~/Library/LaunchAgents`
//! plist plus `launchctl bootstrap` on 10.15-12, kept for one release.

use super::*;
use objc2_foundation::{NSProcessInfo, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// macOS 13 or later: `SMAppService` exists.
fn modern() -> bool {
    NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 13
}

fn agent(target: &Target) -> objc2::rc::Retained<SMAppService> {
    // SAFETY: a plain class constructor taking an NSString.
    unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(&target.plist_name())) }
}

fn domain() -> String {
    // SAFETY: getuid never fails.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn service_target(target: &Target) -> String {
    format!("{}/{}", domain(), target.label)
}

fn launchctl(args: &[&str]) -> Result<String, String> {
    let out = command::new::<std::process::Command>("/bin/launchctl", command::Console::Hidden)
        .args(args)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_string()) }
}

/// The legacy plist (macOS 10.15-12).
fn legacy_plist(target: &Target) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(target.plist_name()))
}

/// The legacy plist for this executable, with the root and port this
/// process was given (a test's), so the engine serves where asked.
fn legacy_body(target: &Target) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("this executable: {e}"))?;
    let port = target.port.to_string();
    let home = std::env::var("NEBO_HOME").ok();
    let mut env: Vec<(&str, &str)> = Vec::new();
    if target.port != 27895 {
        env.push(("NEBO_PORT", &port));
    }
    if let Some(home) = home.as_deref() {
        env.push(("NEBO_HOME", home));
    }
    Ok(render_plist(&target.label, &Program::Path(exe), &env))
}

pub fn status(target: &Target) -> Status {
    if !modern() {
        return match legacy_plist(target) {
            Some(p) if p.exists() => Status::Enabled,
            _ => Status::NotRegistered,
        };
    }
    // SAFETY: reads the registration's status.
    match unsafe { agent(target).status() } {
        SMAppServiceStatus::Enabled => Status::Enabled,
        SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
        SMAppServiceStatus::NotFound => Status::NotFound,
        _ => Status::NotRegistered,
    }
}

/// Register the engine (idempotent). macOS posts its one "Background
/// Items Added" notice the first time.
pub fn install(target: &Target) -> Result<Status, String> {
    if !modern() {
        let path = legacy_plist(target).ok_or("no home folder")?;
        let body = legacy_body(target)?;
        // Rewritten when the app moved; loaded again either way.
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
            let _ = launchctl(&["bootout", &service_target(target)]);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if job(target) == Job::Absent {
            launchctl(&["bootstrap", &domain(), &path.to_string_lossy()])?;
        }
        return Ok(status(target));
    }
    // SAFETY: registers this app's own bundled agent.
    if let Err(e) = unsafe { agent(target).registerAndReturnError() } {
        let status = status(target);
        // Registered, waiting for the owner's approval: not an error here.
        if status != Status::RequiresApproval {
            return Err(format!("register {}: {}", target.label, e.localizedDescription()));
        }
    }
    Ok(status(target))
}

/// Remove the registration. launchd stops a running engine (SIGTERM, its
/// graceful path).
pub fn unregister(target: &Target) -> Result<(), String> {
    if !modern() {
        let _ = launchctl(&["bootout", &service_target(target)]);
        if let Some(p) = legacy_plist(target)
            && p.exists()
        {
            std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        return Ok(());
    }
    if status(target) == Status::NotRegistered {
        return Ok(());
    }
    // SAFETY: unregisters this app's own bundled agent.
    unsafe { agent(target).unregisterAndReturnError() }
        .map_err(|e| format!("unregister {}: {}", target.label, e.localizedDescription()))
}

/// Start the engine now; with `restart`, stop a running one first
/// (SIGTERM: the graceful path).
pub fn kickstart(target: &Target, restart: bool) -> Result<(), String> {
    let service = service_target(target);
    let mut args = vec!["kickstart"];
    if restart {
        args.push("-k");
    }
    args.push(&service);
    launchctl(&args).map(|_| ())
}

pub fn job(target: &Target) -> Job {
    match launchctl(&["print", &service_target(target)]) {
        Ok(out) => parse_print(&out),
        Err(_) => Job::Absent,
    }
}

/// System Settings → General → Login Items.
pub fn open_login_items() {
    if modern() {
        // SAFETY: opens a System Settings pane.
        unsafe { SMAppService::openSystemSettingsLoginItems() };
    } else {
        let _ = open::that("x-apple.systempreferences:com.apple.preferences.users");
    }
}
