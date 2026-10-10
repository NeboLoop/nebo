use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::{ApplyMode, UpdateError};

static PRE_APPLY_HOOK: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// JSON written to `<data_dir>/UPDATE_FAILED.json` when the deferred helper rolls back.
/// The (restored, working) app reads this on the next WS client connect and toasts it.
/// NOTE: must contain no single-quote — it is embedded in a single-quoted `printf` in the
/// POSIX helper script.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const ROLLBACK_MARKER_JSON: &str =
    r#"{"error":"Update failed and was rolled back to the previous version."}"#;

/// How long an app-bundle update waits for the new engine to answer
/// `/health` before it rolls back (the helper's health gate).
#[cfg(target_os = "windows")]
const HEALTH_GATE: std::time::Duration = std::time::Duration::from_secs(90);

/// The port the engine serves on: `NEBO_PORT`, else the default.
#[cfg(target_os = "windows")]
fn engine_port() -> u16 {
    std::env::var("NEBO_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(27895)
}

/// Register a function to run before the binary restarts.
pub fn set_pre_apply_hook(f: Box<dyn Fn() + Send>) {
    let mut hook = PRE_APPLY_HOOK.lock().unwrap();
    *hook = Some(f);
}

fn run_pre_apply() {
    let hook = PRE_APPLY_HOOK.lock().unwrap();
    if let Some(ref f) = *hook {
        f();
    }
}

/// Health check: run "nebo --version" on the new binary.
fn health_check(binary_path: &Path) -> Result<(), UpdateError> {
    let output = command::new::<std::process::Command>(binary_path, command::Console::Hidden)
        .arg("--version")
        .output()
        .map_err(|e| UpdateError::Other(format!("health check failed: {}", e)))?;

    if !output.status.success() {
        return Err(UpdateError::Other("health check: non-zero exit".into()));
    }
    Ok(())
}

/// Copy a file preserving permissions.
fn copy_file(src: &Path, dst: &Path) -> Result<(), UpdateError> {
    let metadata = std::fs::metadata(src)?;
    let mut src_file = std::fs::File::open(src)?;
    let mut dst_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)?;

    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst_file.write_all(&buf[..n])?;
    }
    dst_file.set_permissions(metadata.permissions())?;
    Ok(())
}

/// Path to the rollback marker the deferred helper writes on failure.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn marker_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("UPDATE_FAILED.json")
}

/// Path to the update log the deferred helper appends to.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn log_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("logs").join("update.log")
}

/// Probe whether we can create files in `dir` — used to fail with a clean error
/// **while the app is still running**, before any destructive work is deferred.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".nebo-write-test-{}", uuid::Uuid::new_v4()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Write a POSIX helper script to a temp file and spawn it in its OWN SESSION so
/// it survives this process exiting.
///
/// `setsid()` (via `pre_exec`) is load-bearing, not cosmetic: macOS launchd
/// terminates the remaining child processes of a GUI app's job when that app
/// exits. A plain `spawn()` child is still part of our job, so it is killed the
/// instant the caller `exit(0)`s — before it can swap the bundle or relaunch,
/// producing the "app quits on update but never comes back" bug. Making the
/// helper a session leader detaches it from our job so it runs to completion.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn spawn_detached_sh(script: &str) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let path = std::env::temp_dir().join(format!("nebo-update-helper-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&path, script)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    let mut cmd = command::new::<std::process::Command>("sh", command::Console::Detached);
    cmd.arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid() is async-signal-safe and the only call made in the child
    // between fork and exec. It places the helper in a new session (new process
    // group, no controlling terminal), detaching it from this app's launchd job.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| UpdateError::Other(format!("spawn update helper: {}", e)))?;
    Ok(())
}

/// Apply the update: detect install method and use the appropriate strategy.
///
/// - `app_bundle`: downloaded file is a DMG / NSIS installer / AppImage — swap the
///   installed bundle via a detached helper that runs after this process exits.
/// - `direct`: downloaded file is the raw binary — replace, then `execve` when
///   `mode` is [`ApplyMode::Restart`] or return when it is [`ApplyMode::ReplaceOnly`].
///
/// `data_dir` is where a rollback writes `UPDATE_FAILED.json` (see [`marker_path`]).
pub fn apply(new_path: &Path, data_dir: &Path, mode: ApplyMode) -> Result<(), UpdateError> {
    let method = crate::detect_install_method();
    match (method, mode) {
        ("app_bundle", ApplyMode::Restart) => apply_app_bundle(new_path, data_dir),
        // The bundle swap only happens after this process exits — there is no
        // "replace and keep running" for an app bundle.
        ("app_bundle", ApplyMode::ReplaceOnly) => Err(UpdateError::Other(
            "app bundle updates always restart the app".into(),
        )),
        _ => apply_direct(new_path, mode),
    }
}

/// Platforms with no app-bundle install method (Android: the binary runs from
/// Termux/adb, install method is always `direct`). Reaching this arm means
/// detect_install_method returned something impossible for the platform —
/// fail loudly rather than pretend an update happened.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn apply_app_bundle(_new_path: &Path, _data_dir: &Path) -> Result<(), UpdateError> {
    Err(UpdateError::Other(
        "app_bundle updates are not supported on this platform; use the direct binary".into(),
    ))
}

// ── App Bundle Update (deferred-helper swap) ────────────────────────
//
// Invariant on every platform: the running process performs only non-destructive
// prep (stage the new bundle on the same volume as the target). All destructive work
// happens in a detached helper *after the app exits*, is atomic (same-volume rename),
// and is reversible (move-aside + rollback). On failure the helper restores the
// previous version, writes the rollback marker, and relaunches — the user is never
// left without a working app.

/// macOS: stage Nebo.app out of the DMG next to the target, then a detached helper
/// atomically swaps it in (move-aside → move-in → codesign verify → rollback on fail).
#[cfg(target_os = "macos")]
fn apply_app_bundle(dmg_path: &Path, data_dir: &Path) -> Result<(), UpdateError> {
    use std::process::Command;

    // 1. Mount the DMG.
    let mount_output = command::new::<Command>("hdiutil", command::Console::Hidden)
        .args(["attach", "-nobrowse", "-noverify", "-noautoopen"])
        .arg(dmg_path)
        .output()
        .map_err(|e| UpdateError::Other(format!("hdiutil attach: {}", e)))?;

    if !mount_output.status.success() {
        return Err(UpdateError::Other(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&mount_output.stderr)
        )));
    }

    // Parse mount point from hdiutil output (last column of last line).
    let stdout = String::from_utf8_lossy(&mount_output.stdout);
    let mount_point = stdout
        .lines()
        .last()
        .and_then(|line| line.split('\t').last())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| UpdateError::Other("failed to parse mount point".into()))?;

    let detach = |mp: &str| {
        let _ = command::new::<Command>("hdiutil", command::Console::Hidden).args(["detach", mp]).output();
    };

    // 2. Find Nebo.app in the mounted DMG.
    let source_app = std::path::PathBuf::from(&mount_point).join("Nebo.app");
    if !source_app.is_dir() {
        detach(&mount_point);
        return Err(UpdateError::Other(format!(
            "Nebo.app not found in DMG at {}",
            source_app.display()
        )));
    }

    // 3. Determine destination — where the current .app lives.
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let dest_app = current_exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/Applications/Nebo.app"));
    let dest_parent = dest_app
        .parent()
        .ok_or_else(|| UpdateError::Other("install target has no parent dir".into()))?
        .to_path_buf();

    // Pre-check writability WHILE ALIVE — fail cleanly here rather than after exit.
    if !is_writable(&dest_parent) {
        detach(&mount_point);
        return Err(UpdateError::Other(format!(
            "install directory not writable: {} — move Nebo to /Applications or run with permission",
            dest_parent.display()
        )));
    }

    // 4. Stage the new app on the SAME VOLUME as the target (so the helper's move is
    //    an atomic rename). Non-destructive: the running .app is untouched.
    let staging_dir = dest_parent.join(format!(".nebo-update-{}", uuid::Uuid::new_v4()));
    if let Err(e) = std::fs::create_dir_all(&staging_dir) {
        detach(&mount_point);
        return Err(UpdateError::Other(format!("create staging dir: {}", e)));
    }
    let staged_app = staging_dir.join("Nebo.app");
    let cp_output = command::new::<Command>("cp", command::Console::Hidden)
        .args(["-R"])
        .arg(&source_app)
        .arg(&staged_app)
        .output()
        .map_err(|e| UpdateError::Other(format!("cp -R: {}", e)))?;
    if !cp_output.status.success() {
        let _ = std::fs::remove_dir_all(&staging_dir);
        detach(&mount_point);
        return Err(UpdateError::Other(format!(
            "failed to stage Nebo.app: {}",
            String::from_utf8_lossy(&cp_output.stderr)
        )));
    }

    // 5. Detach the DMG and remove the temp download.
    detach(&mount_point);
    let _ = std::fs::remove_file(dmg_path);

    // 6. Spawn the detached helper, then exit. All destructive work happens after exit.
    run_pre_apply();
    let script = build_macos_helper(
        std::process::id(),
        &dest_app.to_string_lossy(),
        &staged_app.to_string_lossy(),
        &staging_dir.to_string_lossy(),
        &format!("{}.old-{}", dest_app.to_string_lossy(), uuid::Uuid::new_v4()),
        &marker_path(data_dir).to_string_lossy(),
        &log_path(data_dir).to_string_lossy(),
    );
    if let Err(e) = spawn_detached_sh(&script) {
        // Couldn't even spawn the helper — clean up and report while still alive.
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(e);
    }

    std::process::exit(0);
}

#[cfg(target_os = "macos")]
fn build_macos_helper(
    pid: u32,
    dest: &str,
    staged: &str,
    staging: &str,
    old: &str,
    marker: &str,
    log: &str,
) -> String {
    const TEMPLATE: &str = r#"#!/bin/sh
DEST="__DEST__"
STAGED="__STAGED__"
STAGING="__STAGING__"
OLD="__OLD__"
MARKER="__MARKER__"
LOG="__LOG__"
PID=__PID__
mkdir -p "$(dirname "$LOG")" 2>/dev/null
log() { echo "[$(date '+%Y-%m-%dT%H:%M:%S')] $1" >> "$LOG" 2>/dev/null; }
fail() {
  log "FAILED: $1 — rolling back"
  if [ -e "$OLD" ]; then
    rm -rf "$DEST" 2>/dev/null
    mv "$OLD" "$DEST" 2>/dev/null
  fi
  mkdir -p "$(dirname "$MARKER")" 2>/dev/null
  printf '%s' '__MARKER_JSON__' > "$MARKER" 2>/dev/null
  open "$DEST" 2>/dev/null
  rm -rf "$STAGING" 2>/dev/null
  rm -f "$0" 2>/dev/null
  exit 1
}
log "waiting for pid $PID to exit"
while kill -0 "$PID" 2>/dev/null; do sleep 0.2; done
log "pid $PID gone, swapping"
if [ -e "$DEST" ]; then
  mv "$DEST" "$OLD" || fail "move-aside current app"
fi
mv "$STAGED" "$DEST" || fail "move staged app into place"
if ! codesign --verify --deep --strict "$DEST" >> "$LOG" 2>&1; then
  fail "codesign verification"
fi
log "swap OK, relaunching"
open "$DEST" 2>/dev/null
rm -rf "$OLD" "$STAGING" 2>/dev/null
log "update complete"
rm -f "$0" 2>/dev/null
exit 0
"#;
    TEMPLATE
        .replace("__DEST__", dest)
        .replace("__STAGED__", staged)
        .replace("__STAGING__", staging)
        .replace("__OLD__", old)
        .replace("__MARKER__", marker)
        .replace("__LOG__", log)
        .replace("__PID__", &pid.to_string())
        .replace("__MARKER_JSON__", ROLLBACK_MARKER_JSON)
}

/// Windows: a detached helper runs the NSIS installer silently (`/S`) once this
/// process has exited (so the installer can replace in-use files), then starts
/// Nebo again: the engine's task when one is registered, and the app.
///
/// When the engine runs as the OS's service (its Windows task's supervisor,
/// `NEBO_SUPERVISED=taskscheduler`; off by default) the helper also holds the
/// new version to a health gate: the engine must answer `/health` as a version
/// other than this one within [`HEALTH_GATE`]. When it doesn't, or the
/// installer fails, it reinstalls the previous version's installer (kept from
/// the last update that passed the gate) and writes the rollback marker. NSIS
/// is not transactional: the previous installer is the way back.
#[cfg(target_os = "windows")]
fn apply_app_bundle(setup_path: &Path, data_dir: &Path) -> Result<(), UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;

    // The download has no extension; Windows starts an installer only as an .exe.
    let setup = std::env::temp_dir().join(format!("Nebo-update-{}-setup.exe", uuid::Uuid::new_v4()));
    std::fs::rename(setup_path, &setup).or_else(|_| copy_file(setup_path, &setup).map(|()| {
        let _ = std::fs::remove_file(setup_path);
    }))?;

    let gated = std::env::var("NEBO_SUPERVISED").is_ok_and(|s| s == "taskscheduler");
    let previous = data_dir.join("updates").join("previous-setup.exe");
    let script = build_windows_helper(&WindowsHelper {
        pid: std::process::id(),
        setup: setup.to_string_lossy().into_owned(),
        previous: previous.to_string_lossy().into_owned(),
        relaunch: current_exe.to_string_lossy().into_owned(),
        task: WINDOWS_TASK.to_string(),
        port: engine_port(),
        old_version: env!("CARGO_PKG_VERSION").to_string(),
        gate_secs: if gated { HEALTH_GATE.as_secs() } else { 0 },
        marker: marker_path(data_dir).to_string_lossy().into_owned(),
        log: log_path(data_dir).to_string_lossy().into_owned(),
    });
    run_pre_apply();
    spawn_detached_powershell(&script)?;

    std::process::exit(0);
}

/// The engine's Windows task (`src-tauri/src/service/windows.rs`).
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TASK: &str = r"\NeboAI\Nebo Engine";

/// What the Windows update helper works on.
#[cfg(any(target_os = "windows", test))]
pub struct WindowsHelper {
    /// The engine process to wait for.
    pub pid: u32,
    /// The new version's installer.
    pub setup: String,
    /// The installer of the version that last passed the health gate.
    pub previous: String,
    /// The app to start once installed (empty: none).
    pub relaunch: String,
    /// The engine's task, started when registered.
    pub task: String,
    pub port: u16,
    /// The version being replaced: the new engine must answer as another.
    pub old_version: String,
    /// The health gate's length; 0 turns the gate and the rollback off.
    pub gate_secs: u64,
    pub marker: String,
    pub log: String,
}

/// The helper, a PowerShell script. Run as `-EncodedCommand`, which no
/// execution policy blocks (a `.ps1` file would be, and the house box's
/// machine policy is Restricted).
#[cfg(any(target_os = "windows", test))]
pub fn build_windows_helper(h: &WindowsHelper) -> String {
    const TEMPLATE: &str = r#"
$ErrorActionPreference = 'Continue'
$Log = '__LOG__'
function Log($m) { try { New-Item -ItemType Directory -Force (Split-Path $Log) | Out-Null; Add-Content -Path $Log -Value "[$(Get-Date -Format s)] $m" } catch { } }
function Install($setup) {
  try { $p = Start-Process -FilePath $setup -ArgumentList '/S' -Wait -PassThru -ErrorAction Stop; return $p.ExitCode } catch { Log "installer did not run: $_"; return -1 }
}
function Start-Nebo {
  schtasks /Query /TN '__TASK__' 2>&1 | Out-Null
  if ($LASTEXITCODE -eq 0) { schtasks /Run /TN '__TASK__' 2>&1 | Out-Null; Log "started task __TASK__ ($LASTEXITCODE)" }
  if ('__RELAUNCH__' -ne '') { try { Start-Process -FilePath '__RELAUNCH__' } catch { Log "relaunch failed: $_" } }
}
function Healthy($want) {
  $deadline = (Get-Date).AddSeconds(__GATE__)
  while ((Get-Date) -lt $deadline) {
    try {
      $h = Invoke-RestMethod -Uri 'http://127.0.0.1:__PORT__/health' -TimeoutSec 2
      if ($h.status -eq 'ok' -and $h.role -eq 'engine' -and (& $want $h.version)) { Log "engine answers as $($h.version)"; return $true }
    } catch { }
    Start-Sleep -Seconds 1
  }
  return $false
}
function Marker($json) { New-Item -ItemType Directory -Force (Split-Path '__MARKER__') | Out-Null; Set-Content -Encoding ascii -NoNewline -Path '__MARKER__' -Value $json }

Log "waiting for pid __PID__ to exit"
while (Get-Process -Id __PID__ -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 200 }
Log "running installer __SETUP__"
$code = Install '__SETUP__'
Log "installer exited $code"
if (__GATE__ -eq 0) {
  if ($code -ne 0) { Marker '__INSTALL_FAILED_JSON__' }
  Start-Nebo
  Remove-Item -Force '__SETUP__' -ErrorAction SilentlyContinue
  Log "update done (no health gate)"
  exit 0
}
if ($code -eq 0) {
  Start-Nebo
  if (Healthy { param($v) $v -ne '__OLD__' }) {
    New-Item -ItemType Directory -Force (Split-Path '__PREVIOUS__') | Out-Null
    Move-Item -Force '__SETUP__' '__PREVIOUS__'
    Log "update complete"
    exit 0
  }
  Log "FAILED: the new version did not answer within __GATE__ s"
}
if (Test-Path '__PREVIOUS__') {
  Log "rolling back with __PREVIOUS__"
  $back = Install '__PREVIOUS__'
  Log "previous installer exited $back"
  Start-Nebo
  if (Healthy { param($v) $true }) { Log "rolled back" } else { Log "the previous version did not answer either" }
  Marker '__ROLLED_BACK_JSON__'
} else {
  Log "no previous installer to roll back to"
  Start-Nebo
  Marker '__INSTALL_FAILED_JSON__'
}
Remove-Item -Force '__SETUP__' -ErrorAction SilentlyContinue
exit 1
"#;
    // Inside single-quoted PowerShell strings a quote is doubled.
    let q = |s: &str| s.replace('\'', "''");
    TEMPLATE
        .replace("__PID__", &h.pid.to_string())
        .replace("__SETUP__", &q(&h.setup))
        .replace("__PREVIOUS__", &q(&h.previous))
        .replace("__RELAUNCH__", &q(&h.relaunch))
        .replace("__TASK__", &q(&h.task))
        .replace("__PORT__", &h.port.to_string())
        .replace("__OLD__", &q(&h.old_version))
        .replace("__GATE__", &h.gate_secs.to_string())
        .replace("__MARKER__", &q(&h.marker))
        .replace("__LOG__", &q(&h.log))
        .replace("__ROLLED_BACK_JSON__", r#"{"error":"Update failed and was rolled back to the previous version."}"#)
        .replace("__INSTALL_FAILED_JSON__", r#"{"error":"Update failed. Please reinstall Nebo from neboai.com."}"#)
}

/// Start the helper so it outlives this process: its own process group and
/// hidden console (`Console::Detached`).
#[cfg(target_os = "windows")]
fn spawn_detached_powershell(script: &str) -> Result<(), UpdateError> {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    command::new::<std::process::Command>("powershell", command::Console::Detached)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| UpdateError::Other(format!("spawn update helper: {}", e)))?;
    Ok(())
}

/// Linux AppImage: stage the new single-file AppImage next to the target, then a
/// detached helper atomically swaps it in (move-aside → move-in → rollback on fail).
#[cfg(target_os = "linux")]
fn apply_app_bundle(appimage_path: &Path, data_dir: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;

    // Prefer the AppImage runtime's $APPIMAGE path; fall back to the resolved exe.
    let dest = match std::env::var_os("APPIMAGE") {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let current_exe = std::env::current_exe()
                .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
            std::fs::canonicalize(&current_exe)
                .map_err(|e| UpdateError::Other(format!("resolve symlinks: {}", e)))?
        }
    };
    let dest_parent = dest
        .parent()
        .ok_or_else(|| UpdateError::Other("install target has no parent dir".into()))?
        .to_path_buf();

    // Pre-check writability WHILE ALIVE.
    if !is_writable(&dest_parent) {
        return Err(UpdateError::Other(format!(
            "install directory not writable: {}",
            dest_parent.display()
        )));
    }

    // Stage on the same volume as the target.
    let staged = dest_parent.join(format!(".nebo-update-{}.AppImage", uuid::Uuid::new_v4()));
    copy_file(appimage_path, &staged)?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    let _ = std::fs::remove_file(appimage_path);

    run_pre_apply();
    let script = build_linux_appimage_helper(
        std::process::id(),
        &dest.to_string_lossy(),
        &staged.to_string_lossy(),
        &format!("{}.old-{}", dest.to_string_lossy(), uuid::Uuid::new_v4()),
        &marker_path(data_dir).to_string_lossy(),
        &log_path(data_dir).to_string_lossy(),
    );
    if let Err(e) = spawn_detached_sh(&script) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    std::process::exit(0);
}

#[cfg(target_os = "linux")]
fn build_linux_appimage_helper(
    pid: u32,
    dest: &str,
    staged: &str,
    old: &str,
    marker: &str,
    log: &str,
) -> String {
    const TEMPLATE: &str = r#"#!/bin/sh
DEST="__DEST__"
STAGED="__STAGED__"
OLD="__OLD__"
MARKER="__MARKER__"
LOG="__LOG__"
PID=__PID__
mkdir -p "$(dirname "$LOG")" 2>/dev/null
log() { echo "[$(date '+%Y-%m-%dT%H:%M:%S')] $1" >> "$LOG" 2>/dev/null; }
relaunch() { chmod +x "$DEST" 2>/dev/null; ( "$DEST" >/dev/null 2>&1 & ) ; }
fail() {
  log "FAILED: $1 — rolling back"
  if [ -e "$OLD" ]; then
    rm -f "$DEST" 2>/dev/null
    mv "$OLD" "$DEST" 2>/dev/null
  fi
  mkdir -p "$(dirname "$MARKER")" 2>/dev/null
  printf '%s' '__MARKER_JSON__' > "$MARKER" 2>/dev/null
  relaunch
  rm -f "$0" 2>/dev/null
  exit 1
}
log "waiting for pid $PID to exit"
while kill -0 "$PID" 2>/dev/null; do sleep 0.2; done
log "pid $PID gone, swapping"
if [ -e "$DEST" ]; then
  mv "$DEST" "$OLD" || fail "move-aside current AppImage"
fi
mv "$STAGED" "$DEST" || fail "move staged AppImage into place"
chmod +x "$DEST" || fail "chmod new AppImage"
[ -s "$DEST" ] || fail "staged AppImage is empty"
log "swap OK, relaunching"
relaunch
rm -f "$OLD" 2>/dev/null
log "update complete"
rm -f "$0" 2>/dev/null
exit 0
"#;
    TEMPLATE
        .replace("__DEST__", dest)
        .replace("__STAGED__", staged)
        .replace("__OLD__", old)
        .replace("__MARKER__", marker)
        .replace("__LOG__", log)
        .replace("__PID__", &pid.to_string())
        .replace("__MARKER_JSON__", ROLLBACK_MARKER_JSON)
}

// ── Direct Binary Update ────────────────────────────────────────────

/// Unix: rename the running binary aside, write the new binary in its place, then
/// `execve` into it. Renaming first is permitted while the binary is executing and
/// avoids `ETXTBSY` from truncating an in-use executable in place.
#[cfg(unix)]
fn apply_direct(new_binary_path: &Path, mode: ApplyMode) -> Result<(), UpdateError> {
    use std::ffi::CString;

    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let real_path = std::fs::canonicalize(&current_exe)
        .map_err(|e| UpdateError::Other(format!("resolve symlinks: {}", e)))?;

    health_check(new_binary_path)?;

    // Move the running binary aside (atomic, allowed while executing).
    let backup = real_path.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&real_path, &backup)
        .map_err(|e| UpdateError::Other(format!("rename current exe: {}", e)))?;

    // Write the new binary at the original path.
    if let Err(e) = copy_file(new_binary_path, &real_path) {
        // Rollback: restore the original.
        let _ = std::fs::rename(&backup, &real_path);
        return Err(UpdateError::Other(format!("replace binary: {}", e)));
    }

    // Clean temp.
    let _ = std::fs::remove_file(new_binary_path);

    if mode == ApplyMode::ReplaceOnly {
        return Ok(());
    }

    // Release resources.
    run_pre_apply();

    // Exec into the new binary (replaces this process in-place).
    let c_path = CString::new(real_path.to_string_lossy().as_bytes())
        .map_err(|e| UpdateError::Other(format!("CString: {}", e)))?;

    let args: Vec<CString> = std::env::args()
        .map(|a| CString::new(a).unwrap_or_default())
        .collect();

    let env: Vec<CString> = std::env::vars()
        .map(|(k, v)| CString::new(format!("{}={}", k, v)).unwrap_or_default())
        .collect();

    nix_execve(&c_path, &args, &env)
}

/// Windows: rename current → .old, copy new → current, spawn new process.
#[cfg(windows)]
fn apply_direct(new_binary_path: &Path, mode: ApplyMode) -> Result<(), UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;

    health_check(new_binary_path)?;

    // Rename current to .old
    let backup = current_exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&current_exe, &backup)
        .map_err(|e| UpdateError::Other(format!("rename current exe: {}", e)))?;

    // Copy new binary into place
    if let Err(e) = copy_file(new_binary_path, &current_exe) {
        let _ = std::fs::rename(&backup, &current_exe);
        return Err(UpdateError::Other(format!("copy new binary: {}", e)));
    }
    let _ = std::fs::remove_file(new_binary_path);

    if mode == ApplyMode::ReplaceOnly {
        return Ok(());
    }

    run_pre_apply();

    // Spawn new process and exit
    let args: Vec<String> = std::env::args().skip(1).collect();
    command::new::<std::process::Command>(&current_exe, command::Console::Inherit)
        .args(&args)
        .spawn()
        .map_err(|e| UpdateError::Other(format!("start new process: {}", e)))?;

    std::process::exit(0);
}

#[cfg(unix)]
fn nix_execve(
    path: &std::ffi::CString,
    args: &[std::ffi::CString],
    env: &[std::ffi::CString],
) -> Result<(), UpdateError> {
    let c_args: Vec<*const libc::c_char> = args
        .iter()
        .map(|a| a.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let c_env: Vec<*const libc::c_char> = env
        .iter()
        .map(|e| e.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    unsafe {
        libc::execve(path.as_ptr(), c_args.as_ptr(), c_env.as_ptr());
    }

    // If execve returns, it failed
    Err(UpdateError::Other(format!(
        "execve failed: {}",
        std::io::Error::last_os_error()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_helper_substitutes_all_placeholders() {
        let script = build_macos_helper(
            4242,
            "/Applications/Nebo.app",
            "/Applications/.nebo-update-x/Nebo.app",
            "/Applications/.nebo-update-x",
            "/Applications/Nebo.app.old-y",
            "/data/UPDATE_FAILED.json",
            "/data/logs/update.log",
        );
        assert!(!script.contains("__"), "unsubstituted placeholder: {script}");
        assert!(script.contains("PID=4242"));
        assert!(script.contains("codesign --verify"));
        assert!(script.contains("/Applications/Nebo.app"));
        assert!(script.contains(ROLLBACK_MARKER_JSON));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_appimage_helper_substitutes_all_placeholders() {
        let script = build_linux_appimage_helper(
            7,
            "/opt/Nebo.AppImage",
            "/opt/.nebo-update-x.AppImage",
            "/opt/Nebo.AppImage.old-y",
            "/data/UPDATE_FAILED.json",
            "/data/logs/update.log",
        );
        assert!(!script.contains("__"), "unsubstituted placeholder: {script}");
        assert!(script.contains("PID=7"));
        assert!(script.contains(ROLLBACK_MARKER_JSON));
    }

    fn windows_helper(gate_secs: u64) -> String {
        build_windows_helper(&WindowsHelper {
            pid: 99,
            setup: r"C:\Temp\Nebo-update-x-setup.exe".into(),
            previous: r"C:\Users\O'Brien\AppData\Roaming\Nebo\updates\previous-setup.exe".into(),
            relaunch: r"C:\Users\O'Brien\AppData\Local\Nebo\nebo.exe".into(),
            task: WINDOWS_TASK.into(),
            port: 27895,
            old_version: "0.16.15".into(),
            gate_secs,
            marker: r"C:\data\UPDATE_FAILED.json".into(),
            log: r"C:\data\logs\update.log".into(),
        })
    }

    #[test]
    fn windows_helper_substitutes_all_placeholders() {
        let script = windows_helper(90);
        assert!(!script.contains("__"), "unsubstituted placeholder: {script}");
        assert!(script.contains("Get-Process -Id 99"));
        assert!(script.contains("-ArgumentList '/S'"));
        assert!(script.contains(r"schtasks /Run /TN '\NeboAI\Nebo Engine'"));
        assert!(script.contains("http://127.0.0.1:27895/health"));
        assert!(script.contains("$v -ne '0.16.15'"));
        // A quote in a path is doubled inside PowerShell's single quotes.
        assert!(script.contains(r"'C:\Users\O''Brien\AppData\Local\Nebo\nebo.exe'"));
        assert!(script.contains("rolled back to the previous version"));
    }

    #[test]
    fn windows_helper_gates_only_when_asked() {
        assert!(windows_helper(0).contains("if (0 -eq 0)"));
        assert!(windows_helper(90).contains("if (90 -eq 0)"));
        assert!(windows_helper(90).contains("AddSeconds(90)"));
    }
}
