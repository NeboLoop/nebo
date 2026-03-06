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

/// Apply the update: replace binary and restart.
#[cfg(unix)]
pub fn apply(new_binary_path: &Path) -> Result<(), UpdateError> {
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

#[cfg(unix)]
fn nix_execve(
    path: &std::ffi::CString,
    args: &[std::ffi::CString],
    env: &[std::ffi::CString],
) -> Result<(), UpdateError> {
    // Use libc::execve directly
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

/// Apply the update on Windows: rename current → .old, copy new → current, spawn new process.
#[cfg(windows)]
pub fn apply(new_binary_path: &Path) -> Result<(), UpdateError> {
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
