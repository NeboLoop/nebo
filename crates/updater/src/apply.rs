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
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const HEALTH_GATE: std::time::Duration = std::time::Duration::from_secs(90);

/// The port the engine serves on: `NEBO_PORT`, else the default.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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
fn spawn_detached_sh(script: &str) -> Result<std::path::PathBuf, UpdateError> {
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
    // The script removes itself when it is done.
    Ok(path)
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
///
/// When launchd runs the engine as the OS's service (`NEBO_SUPERVISED=launchd`;
/// off by default) the helper also waits for the app's window process, starts
/// the new engine (`launchctl kickstart`), and keeps the new app only once its
/// `/health` reports the new app's version within [`HEALTH_GATE`]; otherwise
/// it puts the previous app back, starts its engine and writes the rollback
/// marker. Without the service it swaps and reopens the app, as before.
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
    let label = launchd_label();
    let updating = data_dir.join(crate::UPDATING_MARKER);
    let script = build_macos_helper(&MacHelper {
        pid: std::process::id(),
        dest: &dest_app.to_string_lossy(),
        staged: &staged_app.to_string_lossy(),
        staging: &staging_dir.to_string_lossy(),
        old: &format!("{}.old-{}", dest_app.to_string_lossy(), uuid::Uuid::new_v4()),
        marker: &marker_path(data_dir).to_string_lossy(),
        log: &log_path(data_dir).to_string_lossy(),
        updating: &updating.to_string_lossy(),
        label: label.as_deref(),
        health: &format!("http://127.0.0.1:{}/health", engine_port()),
    });
    // The window process of a service-run engine waits while this is here:
    // the update starts the engine, never the app.
    if label.is_some() {
        let _ = std::fs::write(&updating, "");
    }
    if let Err(e) = spawn_detached_sh(&script) {
        // Couldn't even spawn the helper — clean up and report while still alive.
        let _ = std::fs::remove_dir_all(&staging_dir);
        let _ = std::fs::remove_file(&updating);
        return Err(e);
    }

    std::process::exit(0);
}

/// The launchd job running this engine, when launchd runs it as the OS's
/// service (`NEBO_SUPERVISED=launchd`): launchd names it in `XPC_SERVICE_NAME`.
#[cfg(target_os = "macos")]
fn launchd_label() -> Option<String> {
    if !std::env::var("NEBO_SUPERVISED").is_ok_and(|s| s == "launchd") {
        return None;
    }
    std::env::var("XPC_SERVICE_NAME").ok().filter(|s| !s.is_empty() && s != "0")
}

/// What the macOS helper script is built from.
#[cfg(target_os = "macos")]
struct MacHelper<'a> {
    /// The engine, which exits right after spawning the helper.
    pid: u32,
    dest: &'a str,
    staged: &'a str,
    staging: &'a str,
    old: &'a str,
    marker: &'a str,
    log: &'a str,
    /// [`crate::UPDATING_MARKER`] in the data dir.
    updating: &'a str,
    /// The engine's launchd job: None when the app runs it as its child.
    label: Option<&'a str>,
    /// The engine's `/health`.
    health: &'a str,
}

/// The helper script. Without a service (the app runs the engine as its
/// child), it swaps the app and opens it. With launchd's service it also:
///
/// - waits for the app's window process (`nebo` of this app, not the engine
///   nor a browser relay) to close, which it does once it sees the engine
///   gone and the update marker; stops it after 20 s;
/// - after the swap, `launchctl kickstart -k`s the engine (the registration
///   is the app's identity and plist name, so a same-path swap keeps it) and
///   reopens the app only if it was open;
/// - health gate: keeps the new app only once `/health` reports the new
///   app's own version (its Info.plist `CFBundleShortVersionString`) within
///   [`HEALTH_GATE`]; otherwise puts the previous app back, starts its engine
///   and writes `UPDATE_FAILED.json`.
#[cfg(target_os = "macos")]
fn build_macos_helper(h: &MacHelper) -> String {
    const TEMPLATE: &str = r#"#!/bin/sh
DEST="__DEST__"
STAGED="__STAGED__"
STAGING="__STAGING__"
OLD="__OLD__"
MARKER="__MARKER__"
LOG="__LOG__"
UPDATING="__UPDATING__"
PID=__PID__
LABEL="__LABEL__"
HEALTH="__HEALTH__"
GATE=__GATE__
EXE="$DEST/Contents/MacOS/nebo"
mkdir -p "$(dirname "$LOG")" 2>/dev/null
log() { echo "[$(date '+%Y-%m-%dT%H:%M:%S')] $1" >> "$LOG" 2>/dev/null; }
# The app's window processes: this app's nebo, not its engine, not a relay.
shells() {
  ps -axo pid=,command= | awk -v exe="$EXE" '{ pid = $1; $1 = ""; sub(/^ /, ""); if (index($0, exe) == 1 && $0 !~ /--engine/ && $0 !~ /chrome-extension:/) print pid }'
}
stop_shells() {
  n=0
  while [ -n "$(shells)" ] && [ $n -lt $1 ]; do sleep 1; n=$((n + 1)); done
  for p in $(shells); do kill -TERM "$p" 2>/dev/null; done
  sleep 2
  for p in $(shells); do kill -KILL "$p" 2>/dev/null; done
}
start_engine() {
  [ -n "$LABEL" ] && launchctl kickstart -k "gui/$(id -u)/$LABEL" >> "$LOG" 2>&1
}
reopen() {
  if [ -z "$LABEL" ] || [ "$HAD_SHELL" = 1 ]; then open "$DEST" 2>/dev/null; fi
}
fail() {
  log "FAILED: $1 — rolling back"
  [ -n "$LABEL" ] && stop_shells 0
  if [ -e "$OLD" ]; then
    rm -rf "$DEST" 2>/dev/null
    mv "$OLD" "$DEST" 2>/dev/null
  fi
  mkdir -p "$(dirname "$MARKER")" 2>/dev/null
  printf '%s' '__MARKER_JSON__' > "$MARKER" 2>/dev/null
  rm -f "$UPDATING" 2>/dev/null
  start_engine
  reopen
  rm -rf "$STAGING" 2>/dev/null
  rm -f "$0" 2>/dev/null
  exit 1
}
log "waiting for pid $PID to exit"
while kill -0 "$PID" 2>/dev/null; do sleep 0.2; done
HAD_SHELL=0
if [ -n "$LABEL" ]; then
  [ -n "$(shells)" ] && HAD_SHELL=1
  log "pid $PID gone; waiting for the app to close"
  stop_shells 20
fi
log "swapping"
if [ -e "$DEST" ]; then
  mv "$DEST" "$OLD" || fail "move-aside current app"
fi
mv "$STAGED" "$DEST" || fail "move staged app into place"
if ! codesign --verify --deep --strict "$DEST" >> "$LOG" 2>&1; then
  fail "codesign verification"
fi
if [ -n "$LABEL" ]; then
  NEW=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$DEST/Contents/Info.plist" 2>/dev/null)
  [ -n "$NEW" ] || fail "the new app names no version"
  log "swap OK, starting the new engine ($NEW)"
  start_engine
  n=0
  until curl -fsS --max-time 2 "$HEALTH" 2>/dev/null | grep -qF "\"version\":\"$NEW\""; do
    n=$((n + 1))
    [ $n -ge $GATE ] && fail "the new engine did not report $NEW within ${GATE}s"
    sleep 1
  done
  log "the new engine reports $NEW"
  rm -f "$UPDATING" 2>/dev/null
  reopen
else
  log "swap OK, relaunching"
  open "$DEST" 2>/dev/null
fi
rm -rf "$OLD" "$STAGING" 2>/dev/null
log "update complete"
rm -f "$0" 2>/dev/null
exit 0
"#;
    TEMPLATE
        .replace("__DEST__", h.dest)
        .replace("__STAGED__", h.staged)
        .replace("__STAGING__", h.staging)
        .replace("__OLD__", h.old)
        .replace("__MARKER__", h.marker)
        .replace("__LOG__", h.log)
        .replace("__UPDATING__", h.updating)
        .replace("__PID__", &h.pid.to_string())
        .replace("__LABEL__", h.label.unwrap_or(""))
        .replace("__HEALTH__", h.health)
        .replace("__GATE__", &HEALTH_GATE.as_secs().to_string())
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
        self_task: if gated { UPDATE_TASK.to_string() } else { String::new() },
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
    // An engine the task runs ends with its task, and Task Scheduler takes
    // what the task's processes started with it (seen on the house runner:
    // children die when the task ends, breakaway or not). So that engine's
    // helper runs as a task of its own.
    if gated {
        match start_helper_task(UPDATE_TASK, &script) {
            Ok(()) => std::process::exit(0),
            Err(e) => tracing::warn!(error = %e, "update helper task did not start; starting the helper directly"),
        }
    }
    spawn_detached_powershell(&script)?;

    std::process::exit(0);
}

/// The update helper's own task, for an engine the Windows task runs.
#[cfg(target_os = "windows")]
const UPDATE_TASK: &str = r"\NeboAI\Nebo Update";

/// The helper as `powershell -EncodedCommand`'s argument: UTF-16, base64.
#[cfg(target_os = "windows")]
fn encode_powershell(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// Run the helper as the one-shot task `name` in the user's session, apart
/// from the task the engine ran in. `conhost --headless` gives PowerShell a
/// console no one sees: a task's console program would open a window.
#[cfg(target_os = "windows")]
pub fn start_helper_task(name: &str, script: &str) -> Result<(), UpdateError> {
    let conhost = std::path::Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("System32")
        .join("conhost.exe");
    let arguments = format!("--headless powershell.exe -NoProfile -NonInteractive -EncodedCommand {}", encode_powershell(script));
    let task = command::task::Task {
        description: "Installs a Nebo update, then removes itself.",
        command: &conhost.to_string_lossy(),
        arguments: &arguments,
        at_logon: false,
    };
    command::task::register(name, &task).map_err(UpdateError::Other)?;
    command::task::run(name).map_err(UpdateError::Other)
}

/// The engine's Windows task (`src-tauri/src/service/windows.rs`).
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TASK: &str = r"\NeboAI\Nebo Engine";

/// What the Windows update helper works on.
#[cfg(any(target_os = "windows", test))]
pub struct WindowsHelper {
    /// The helper's own task, deleted when it is done (empty: started
    /// directly).
    pub self_task: String,
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
function Done($code) {
  if ('__SELF_TASK__' -ne '') { schtasks /Delete /TN '__SELF_TASK__' /F 2>&1 | Out-Null }
  exit $code
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
  Done 0
}
if ($code -eq 0) {
  Start-Nebo
  if (Healthy { param($v) $v -ne '__OLD__' }) {
    New-Item -ItemType Directory -Force (Split-Path '__PREVIOUS__') | Out-Null
    Move-Item -Force '__SETUP__' '__PREVIOUS__'
    Log "update complete"
    Done 0
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
Done 1
"#;
    // Inside single-quoted PowerShell strings a quote is doubled.
    let q = |s: &str| s.replace('\'', "''");
    TEMPLATE
        .replace("__SELF_TASK__", &q(&h.self_task))
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
    command::new::<std::process::Command>("powershell", command::Console::Detached)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encode_powershell(script)])
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
    let old = format!("{}.old-{}", dest.to_string_lossy(), uuid::Uuid::new_v4());
    let (marker, log) = (marker_path(data_dir), log_path(data_dir));
    let spawned = match linux_service_unit() {
        // The engine runs as a `systemd --user` unit (the engine service,
        // off by default): the helper restarts the unit and holds the new
        // version to a health gate.
        Some(unit) => {
            let updating = data_dir.join(crate::UPDATING_MARKER);
            let _ = std::fs::write(&updating, std::process::id().to_string());
            let script = build_linux_service_helper(&LinuxServiceSwap {
                pid: std::process::id(),
                dest: &dest.to_string_lossy(),
                staged: &staged.to_string_lossy(),
                old: &old,
                marker: &marker.to_string_lossy(),
                log: &log.to_string_lossy(),
                unit: &unit,
                port: &engine_port().to_string(),
                updating: &updating.to_string_lossy(),
            });
            let spawned = spawn_outside_unit(&script);
            if spawned.is_err() {
                let _ = std::fs::remove_file(&updating);
            }
            spawned
        }
        None => spawn_detached_sh(&build_linux_appimage_helper(
            std::process::id(),
            &dest.to_string_lossy(),
            &staged.to_string_lossy(),
            &old,
            &marker.to_string_lossy(),
            &log.to_string_lossy(),
        ))
        .map(drop),
    };
    if let Err(e) = spawned {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    std::process::exit(0);
}

/// The `systemd --user` unit this engine runs as (`NEBO_SERVICE_UNIT`, set by
/// the unit the desktop app writes), if it runs as one.
#[cfg(target_os = "linux")]
fn linux_service_unit() -> Option<String> {
    (std::env::var("NEBO_SUPERVISED").ok()? == "systemd")
        .then(|| std::env::var("NEBO_SERVICE_UNIT").ok())
        .flatten()
        .filter(|u| !u.is_empty())
}

/// Start the helper as a transient unit of its own: when the engine exits,
/// systemd stops every process in the engine's unit, a `setsid` helper
/// included. `KillMode=process`: the app the helper opens again outlives
/// it. Falls back to the detached helper if `systemd-run` won't.
#[cfg(target_os = "linux")]
fn spawn_outside_unit(script: &str) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let id = uuid::Uuid::new_v4();
    let path = std::env::temp_dir().join(format!("nebo-update-helper-{id}.sh"));
    std::fs::write(&path, script)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    let ran = command::new::<std::process::Command>("systemd-run", command::Console::Hidden)
        .args(["--user", "--collect", "--quiet", "--property=KillMode=process", "--description=Nebo update"])
        .arg(format!("--unit=nebo-update-{id}"))
        .arg("sh")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .output();
    match ran {
        Ok(out) if out.status.success() => Ok(()),
        other => {
            tracing::warn!(result = ?other.map(|o| o.status), "systemd-run would not start the update helper; detaching it");
            let _ = std::fs::remove_file(&path);
            spawn_detached_sh(script).map(drop)
        }
    }
}

/// What the service helper swaps, and how it checks the new engine.
#[cfg(target_os = "linux")]
struct LinuxServiceSwap<'a> {
    /// The engine, which exits right after spawning the helper.
    pid: u32,
    dest: &'a str,
    staged: &'a str,
    old: &'a str,
    marker: &'a str,
    log: &'a str,
    /// The engine's unit.
    unit: &'a str,
    /// The engine's port, for the health gate.
    port: &'a str,
    /// [`crate::UPDATING_MARKER`] in the data dir, removed when done.
    updating: &'a str,
}

/// The AppImage swap for an engine that runs as a `systemd --user` unit, the
/// macOS service helper's flow:
///
/// - waits for the engine, then for the app's window processes (this
///   AppImage's `nebo`, not its engine nor a browser relay), which close on
///   their own once they see the engine gone and the update marker; stopped
///   after 20 s;
/// - swaps (move-aside, move-in), then `systemctl --user restart`s the unit
///   (its `ExecStart` is the AppImage's path, so a same-path swap keeps it)
///   and reopens the app only if it was open;
/// - health gate: keeps the new AppImage only once `/health` reports the
///   new one's own version within [`HEALTH_GATE`] (asked by the new
///   executable, `--engine-service health`, so one that can't run fails
///   too); otherwise puts the previous AppImage back, starts its engine and
///   writes `UPDATE_FAILED.json`.
#[cfg(target_os = "linux")]
fn build_linux_service_helper(s: &LinuxServiceSwap) -> String {
    const TEMPLATE: &str = r#"#!/bin/sh
DEST="__DEST__"
STAGED="__STAGED__"
OLD="__OLD__"
MARKER="__MARKER__"
LOG="__LOG__"
UNIT="__UNIT__"
UPDATING="__UPDATING__"
PID=__PID__
GATE=__GATE__
mkdir -p "$(dirname "$LOG")" 2>/dev/null
log() { echo "[$(date '+%Y-%m-%dT%H:%M:%S')] $1" >> "$LOG" 2>/dev/null; }
# The app's window processes: this AppImage's nebo, not its engine, not a relay.
shells() {
  for p in $(pgrep -u "$(id -u)" -x nebo); do
    case "$(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null)" in *--engine*|*chrome-extension:*) continue ;; esac
    tr '\0' '\n' < /proc/$p/environ 2>/dev/null | grep -qxF "APPIMAGE=$DEST" && echo "$p"
  done
}
stop_shells() {
  n=0
  while [ -n "$(shells)" ] && [ $n -lt $1 ]; do sleep 1; n=$((n + 1)); done
  for p in $(shells); do kill -TERM "$p" 2>/dev/null; done
  sleep 2
  for p in $(shells); do kill -KILL "$p" 2>/dev/null; done
}
reopen() {
  [ "$HAD_SHELL" = 1 ] && ( "$DEST" >/dev/null 2>&1 & )
}
fail() {
  log "FAILED: $1 — rolling back"
  stop_shells 0
  systemctl --user stop "$UNIT" >> "$LOG" 2>&1
  if [ -e "$OLD" ]; then
    rm -f "$DEST" 2>/dev/null
    mv "$OLD" "$DEST" 2>/dev/null
  fi
  chmod +x "$DEST" 2>/dev/null
  mkdir -p "$(dirname "$MARKER")" 2>/dev/null
  printf '%s' '__MARKER_JSON__' > "$MARKER" 2>/dev/null
  rm -f "$UPDATING" 2>/dev/null
  systemctl --user reset-failed "$UNIT" 2>/dev/null
  systemctl --user start "$UNIT" >> "$LOG" 2>&1
  log "rolled back; the previous engine started"
  reopen
  rm -f "$0" 2>/dev/null
  exit 1
}
log "waiting for engine pid $PID to exit"
while kill -0 "$PID" 2>/dev/null; do sleep 0.2; done
HAD_SHELL=0
[ -n "$(shells)" ] && HAD_SHELL=1
log "pid $PID gone; waiting for the app to close"
stop_shells 20
log "swapping"
if [ -e "$DEST" ]; then
  mv "$DEST" "$OLD" || fail "move-aside current AppImage"
fi
mv "$STAGED" "$DEST" || fail "move staged AppImage into place"
chmod +x "$DEST" || fail "chmod new AppImage"
[ -s "$DEST" ] || fail "staged AppImage is empty"
systemctl --user daemon-reload >> "$LOG" 2>&1
systemctl --user reset-failed "$UNIT" 2>/dev/null
systemctl --user restart "$UNIT" >> "$LOG" 2>&1 || fail "start $UNIT"
log "swap OK, waiting for the new engine"
"$DEST" --engine-service health --port "__PORT__" --within "$GATE" >> "$LOG" 2>&1 || fail "the new engine did not report its version within ${GATE}s"
log "the new engine reports its version; update complete"
rm -f "$UPDATING" 2>/dev/null
rm -f "$OLD" 2>/dev/null
reopen
rm -f "$0" 2>/dev/null
exit 0
"#;
    TEMPLATE
        .replace("__DEST__", s.dest)
        .replace("__STAGED__", s.staged)
        .replace("__OLD__", s.old)
        .replace("__MARKER__", s.marker)
        .replace("__LOG__", s.log)
        .replace("__UNIT__", s.unit)
        .replace("__UPDATING__", s.updating)
        .replace("__PORT__", s.port)
        .replace("__GATE__", &HEALTH_GATE.as_secs().to_string())
        .replace("__PID__", &s.pid.to_string())
        .replace("__MARKER_JSON__", ROLLBACK_MARKER_JSON)
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
    fn mac_helper(label: Option<&str>) -> String {
        build_macos_helper(&MacHelper {
            pid: 4242,
            dest: "/Applications/Nebo.app",
            staged: "/Applications/.nebo-update-x/Nebo.app",
            staging: "/Applications/.nebo-update-x",
            old: "/Applications/Nebo.app.old-y",
            marker: "/data/UPDATE_FAILED.json",
            log: "/data/logs/update.log",
            updating: "/data/UPDATING",
            label,
            health: "http://127.0.0.1:27895/health",
        })
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_helper_substitutes_all_placeholders() {
        for label in [None, Some("dev.neboai.nebo.engine")] {
            let script = mac_helper(label);
            assert!(!script.contains("__"), "unsubstituted placeholder: {script}");
            assert!(script.contains("PID=4242"));
            assert!(script.contains("codesign --verify"));
            assert!(script.contains("/Applications/Nebo.app"));
            assert!(script.contains(ROLLBACK_MARKER_JSON));
            assert!(script.contains("GATE=90"));
            assert!(script.contains(&format!("LABEL=\"{}\"", label.unwrap_or(""))));
        }
    }

    /// The script parses, and the app's own window process is told apart
    /// from its engine, a browser relay and other apps.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_helper_is_valid_sh_and_finds_only_the_window_process() {
        let script = mac_helper(Some("dev.neboai.nebo.engine"));
        let dir = std::env::temp_dir().join(format!("nebo-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("helper.sh");
        std::fs::write(&path, &script).unwrap();
        let ok = command::new::<std::process::Command>("sh", command::Console::Hidden).arg("-n").arg(&path).status().unwrap();
        assert!(ok.success(), "sh -n rejects the helper");
        // The awk filter of `shells`, over a fixed process list.
        let filter = script.lines().find(|l| l.contains("ps -axo")).unwrap().split_once("| ").unwrap().1;
        let list = "  11 /Applications/Nebo.app/Contents/MacOS/nebo\n  12 Contents/MacOS/nebo --engine\n  13 /Applications/Nebo.app/Contents/MacOS/nebo --engine\n  14 /Applications/Nebo.app/Contents/MacOS/nebo chrome-extension://abc/\n  15 /Applications/Other.app/Contents/MacOS/nebo\n";
        let out = command::new::<std::process::Command>("sh", command::Console::Hidden)
            .arg("-c")
            .arg(format!("EXE=/Applications/Nebo.app/Contents/MacOS/nebo; printf '{list}' | {filter}"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "11\n");
        std::fs::remove_dir_all(&dir).unwrap();
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_service_helper_restarts_the_unit_and_gates_on_health() {
        let script = build_linux_service_helper(&LinuxServiceSwap {
            pid: 7,
            dest: "/home/a/Nebo.AppImage",
            staged: "/home/a/.nebo-update-x.AppImage",
            old: "/home/a/Nebo.AppImage.old-y",
            marker: "/data/UPDATE_FAILED.json",
            log: "/data/logs/update.log",
            unit: "nebo-engine.service",
            port: "27895",
            updating: "/data/UPDATING",
        });
        assert!(!script.contains("__"), "unsubstituted placeholder: {script}");
        assert!(script.contains("PID=7"));
        assert!(script.contains("GATE=90"));
        assert!(script.contains("systemctl --user restart \"$UNIT\""));
        assert!(script.contains("--engine-service health --port \"27895\" --within \"$GATE\""));
        assert!(script.contains(ROLLBACK_MARKER_JSON));
        assert!(script.contains("rm -f \"$UPDATING\""));
        // The script parses.
        let path = std::env::temp_dir().join(format!("nebo-linux-helper-{}.sh", std::process::id()));
        std::fs::write(&path, &script).unwrap();
        let ok = command::new::<std::process::Command>("sh", command::Console::Hidden).arg("-n").arg(&path).status().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(ok.success(), "sh -n rejects the helper");
    }

    fn windows_helper(gate_secs: u64) -> String {
        build_windows_helper(&WindowsHelper {
            self_task: r"\NeboAI\Nebo Update".into(),
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
        assert!(script.contains(r"schtasks /Delete /TN '\NeboAI\Nebo Update' /F"));
    }

    #[test]
    fn windows_helper_gates_only_when_asked() {
        assert!(windows_helper(0).contains("if (0 -eq 0)"));
        assert!(windows_helper(90).contains("if (90 -eq 0)"));
        assert!(windows_helper(90).contains("AddSeconds(90)"));
    }

    /// An app update under launchd, end to end, on a registered TEST agent
    /// (`scripts/test-engine-service-macos.sh` registers one and sets the
    /// env): a good update swaps the app, starts the new engine and passes
    /// the health gate; one whose engine never reports the version, and one
    /// whose signature is broken, are rolled back with the failure marker,
    /// and the previous engine serves again. No window opens (none was open).
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a registered test LaunchAgent: scripts/test-engine-service-macos.sh"]
    fn macos_service_update_under_launchd() {
        use std::os::unix::fs::MetadataExt;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
        let app = PathBuf::from(env("NEBO_TEST_APP"));
        let label = env("NEBO_TEST_LABEL");
        // A copy of the app whose Info.plist names a version its engine never
        // reports (re-signed): the health gate's failure.
        let never = PathBuf::from(env("NEBO_TEST_APP_NEVER"));
        let port = env("NEBO_TEST_PORT");
        let key = env("NEBO_TEST_KEY");
        let version = env("NEBO_TEST_VERSION");
        let data = PathBuf::from(env("NEBO_TEST_HOME"));
        let health_url = format!("http://127.0.0.1:{port}/health");
        let exe = app.join("Contents/MacOS/nebo");

        let health = || -> Option<serde_json::Value> {
            let out = command::new::<std::process::Command>("curl", command::Console::Hidden).args(["-fsS", "--max-time", "2", &health_url]).output().ok()?;
            serde_json::from_slice(&out.stdout).ok()
        };
        let serving = |within: Duration| {
            let end = Instant::now() + within;
            while Instant::now() < end {
                if health().is_some_and(|h| h["version"] == version.as_str()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            false
        };

        // One update: stage a copy (broken if asked), hand it to the helper,
        // stop the engine as `apply_app_bundle` does, wait for the helper.
        let update = |source: &Path, corrupt: bool| {
            assert!(serving(Duration::from_secs(60)), "the engine is not serving before the update");
            let pid = health().unwrap()["pid"].as_u64().unwrap() as u32;
            let staging = app.parent().unwrap().join(format!(".nebo-update-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&staging).unwrap();
            let staged = staging.join(app.file_name().unwrap());
            assert!(command::new::<std::process::Command>("cp", command::Console::Hidden).arg("-R").arg(source).arg(&staged).status().unwrap().success());
            if corrupt {
                let mut f = std::fs::OpenOptions::new().append(true).open(staged.join("Contents/MacOS/nebo")).unwrap();
                f.write_all(b"not signed").unwrap();
            }
            let marker = marker_path(&data);
            let _ = std::fs::remove_file(&marker);
            let updating = data.join(crate::UPDATING_MARKER);
            std::fs::write(&updating, "").unwrap();
            let script = build_macos_helper(&MacHelper {
                pid,
                dest: &app.to_string_lossy(),
                staged: &staged.to_string_lossy(),
                staging: &staging.to_string_lossy(),
                old: &format!("{}.old-{}", app.to_string_lossy(), uuid::Uuid::new_v4()),
                marker: &marker.to_string_lossy(),
                log: &log_path(&data).to_string_lossy(),
                updating: &updating.to_string_lossy(),
                label: Some(&label),
                health: &health_url,
            });
            let helper = spawn_detached_sh(&script).unwrap();
            // The engine stops the graceful way: exit 0, launchd leaves it down.
            let quit = command::new::<std::process::Command>("curl", command::Console::Hidden)
                .args(["-fsS", "-X", "POST", "-H", &format!("Authorization: Bearer {key}")])
                .arg(format!("http://127.0.0.1:{port}/api/v1/engine/quit"))
                .status()
                .unwrap();
            assert!(quit.success(), "the engine did not take Quit");
            let end = Instant::now() + Duration::from_secs(200);
            while helper.exists() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(500));
            }
            assert!(!helper.exists(), "the helper did not finish within 200 s");
            assert!(!updating.exists(), "the helper left the updating marker");
            assert!(!staging.exists(), "the helper left the staging folder");
            marker.exists()
        };

        // A good update.
        let before = std::fs::metadata(&exe).unwrap().ino();
        assert!(!update(&app, false), "a good update wrote the failure marker");
        assert_ne!(std::fs::metadata(&exe).unwrap().ino(), before, "the app was not swapped");
        assert!(serving(Duration::from_secs(5)), "the new engine is not serving");

        // The new engine never reports the new version: rolled back.
        let before = std::fs::metadata(&exe).unwrap().ino();
        assert!(update(&never, false), "no failure marker after the health gate failed");
        assert_eq!(std::fs::metadata(&exe).unwrap().ino(), before, "the previous app is not back");
        assert!(serving(Duration::from_secs(60)), "the previous engine is not serving after the rollback");

        // A broken signature: rolled back before any engine starts.
        let before = std::fs::metadata(&exe).unwrap().ino();
        assert!(update(&app, true), "no failure marker after the signature check failed");
        assert_eq!(std::fs::metadata(&exe).unwrap().ino(), before, "the previous app is not back");
        assert!(serving(Duration::from_secs(60)), "the previous engine is not serving after the rollback");

        let leftovers: Vec<_> = std::fs::read_dir(app.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".old-") || n.starts_with(".nebo-update-"))
            .collect();
        assert!(leftovers.is_empty(), "left beside the app: {leftovers:?}");
    }
}
