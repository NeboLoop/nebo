use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::UpdateError;

static PRE_APPLY_HOOK: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

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
    let output = std::process::Command::new(binary_path)
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

/// Apply the update: detect install method and use the appropriate strategy.
///
/// - `app_bundle`: downloaded file is a DMG/MSI/AppImage — mount, replace .app, relaunch
/// - `direct`: downloaded file is the raw binary — replace in-place, execve
pub fn apply(new_path: &Path) -> Result<(), UpdateError> {
    let method = crate::detect_install_method();
    match method {
        "app_bundle" => apply_app_bundle(new_path),
        _ => apply_direct(new_path),
    }
}

// ── App Bundle Update (DMG / MSI / AppImage) ────────────────────────

/// macOS: mount DMG, copy Nebo.app to /Applications, relaunch.
#[cfg(target_os = "macos")]
fn apply_app_bundle(dmg_path: &Path) -> Result<(), UpdateError> {
    use std::process::Command;

    // 1. Mount the DMG
    let mount_output = Command::new("hdiutil")
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

    // Parse mount point from hdiutil output (last column of last line)
    let stdout = String::from_utf8_lossy(&mount_output.stdout);
    let mount_point = stdout
        .lines()
        .last()
        .and_then(|line| line.split('\t').last())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| UpdateError::Other("failed to parse mount point".into()))?;

    // 2. Find Nebo.app in mounted DMG
    let source_app = std::path::PathBuf::from(&mount_point).join("Nebo.app");
    if !source_app.is_dir() {
        let _ = Command::new("hdiutil").args(["detach", &mount_point]).output();
        return Err(UpdateError::Other(format!(
            "Nebo.app not found in DMG at {}",
            source_app.display()
        )));
    }

    // 3. Determine destination — where the current .app lives
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let dest_app = current_exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/Applications/Nebo.app"));

    // 4. Remove old .app and copy new one
    run_pre_apply();

    let _ = std::fs::remove_dir_all(&dest_app);
    let cp_output = Command::new("cp")
        .args(["-R"])
        .arg(&source_app)
        .arg(&dest_app)
        .output()
        .map_err(|e| UpdateError::Other(format!("cp -R: {}", e)))?;

    if !cp_output.status.success() {
        let _ = Command::new("hdiutil").args(["detach", &mount_point]).output();
        return Err(UpdateError::Other(format!(
            "failed to copy Nebo.app: {}",
            String::from_utf8_lossy(&cp_output.stderr)
        )));
    }

    // 5. Unmount DMG
    let _ = Command::new("hdiutil").args(["detach", &mount_point]).output();

    // 6. Clean temp
    let _ = std::fs::remove_file(dmg_path);

    // 7. Relaunch the app after this process exits.
    // We spawn a background shell that waits for our PID to die, then opens the new app.
    // This avoids the race where `open -n` fires while the old process is still alive.
    let pid = std::process::id();
    let app_path = dest_app.to_string_lossy().to_string();
    let _ = Command::new("sh")
        .args([
            "-c",
            &format!(
                "while kill -0 {} 2>/dev/null; do sleep 0.2; done; open {:?}",
                pid, app_path
            ),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    std::process::exit(0);
}

/// Windows: run the MSI installer silently and exit.
#[cfg(target_os = "windows")]
fn apply_app_bundle(msi_path: &Path) -> Result<(), UpdateError> {
    use std::process::Command;

    run_pre_apply();

    // msiexec /i Nebo.msi /quiet /norestart, then relaunch after install completes.
    // We use cmd /c to chain: run msiexec (wait), then start the new exe.
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let exe_path = current_exe.to_string_lossy().to_string();
    let msi_str = msi_path.to_string_lossy().to_string();

    Command::new("cmd")
        .args([
            "/C",
            &format!(
                "msiexec /i \"{}\" /quiet /norestart && start \"\" \"{}\"",
                msi_str, exe_path
            ),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| UpdateError::Other(format!("msiexec+relaunch: {}", e)))?;

    std::process::exit(0);
}

/// Linux: replace AppImage, make executable, relaunch.
#[cfg(target_os = "linux")]
fn apply_app_bundle(appimage_path: &Path) -> Result<(), UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let real_path = std::fs::canonicalize(&current_exe)
        .map_err(|e| UpdateError::Other(format!("resolve symlinks: {}", e)))?;

    // Backup
    let backup = real_path.with_extension("old");
    copy_file(&real_path, &backup)?;

    // Replace
    if let Err(e) = copy_file(appimage_path, &real_path) {
        let _ = copy_file(&backup, &real_path);
        return Err(UpdateError::Other(format!("replace AppImage: {}", e)));
    }

    // Make executable
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&real_path, std::fs::Permissions::from_mode(0o755))?;

    let _ = std::fs::remove_file(appimage_path);

    run_pre_apply();

    // Relaunch
    std::process::Command::new(&real_path)
        .spawn()
        .map_err(|e| UpdateError::Other(format!("relaunch: {}", e)))?;

    std::process::exit(0);
}

// ── Direct Binary Update ────────────────────────────────────────────

/// Unix: replace binary in-place and execve into the new process.
#[cfg(unix)]
fn apply_direct(new_binary_path: &Path) -> Result<(), UpdateError> {
    use std::ffi::CString;

    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::Other(format!("resolve executable: {}", e)))?;
    let real_path = std::fs::canonicalize(&current_exe)
        .map_err(|e| UpdateError::Other(format!("resolve symlinks: {}", e)))?;

    health_check(new_binary_path)?;

    // Backup current binary
    let backup = real_path.with_extension("old");
    copy_file(&real_path, &backup)?;

    // Replace with new binary
    if let Err(e) = copy_file(new_binary_path, &real_path) {
        // Rollback
        let _ = copy_file(&backup, &real_path);
        return Err(UpdateError::Other(format!("replace binary: {}", e)));
    }

    // Clean temp
    let _ = std::fs::remove_file(new_binary_path);

    // Release resources
    run_pre_apply();

    // Exec into new binary (replaces this process in-place)
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
fn apply_direct(new_binary_path: &Path) -> Result<(), UpdateError> {
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

    run_pre_apply();

    // Spawn new process and exit
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(&current_exe)
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
