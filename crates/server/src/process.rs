//! This process as the desktop app's engine: how it stops, what its exit
//! code tells whoever started it, and what it asks of the OS so it keeps
//! full speed with no window.
//!
//! The desktop app runs the server as its own process (`nebo --engine`); the
//! window process (the shell) attaches to it over this API. Whoever started
//! the engine reads its exit code to decide whether to start it again:
//!
//! | code | meaning | supervisor |
//! |---|---|---|
//! | 0 | stopped on purpose (Quit, an update, a signal) | leaves it down |
//! | [`EXIT_STALL`] | the stall watchdog fired (`liveness`) | starts it again |
//! | [`EXIT_PORT_HELD`] | another process holds the port | starts it again after a pause |
//! | anything else | a crash | starts it again |

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use std::sync::LazyLock;
use tokio::sync::Notify;
use tracing::info;

use types::NeboError;

/// `EX_SOFTWARE`: the stall watchdog stopped a supervised engine.
pub const EXIT_STALL: i32 = 70;
/// `EX_TEMPFAIL`: the port is held by another process (another Nebo engine,
/// or something else); a supervisor tries again once it is free.
pub const EXIT_PORT_HELD: i32 = 75;

/// The exit code for how `server::run` ended.
pub fn exit_code(result: &Result<(), NeboError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(NeboError::PortInUse(_)) => EXIT_PORT_HELD,
        Err(_) => 1,
    }
}

/// Who starts this engine again when it exits non-zero, from
/// `NEBO_SUPERVISED`: the desktop shell that started it as its child
/// (`shell`), or the OS's service manager. None for a server nothing
/// restarts (`nebo-cli serve`, a cloud pod, where Kubernetes probes `/health`).
pub fn supervisor() -> Option<String> {
    std::env::var("NEBO_SUPERVISED").ok().filter(|s| !s.is_empty())
}

static STOP: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Stop this server the graceful way a shutdown signal does: drain, close,
/// exit 0. Callable from any thread, before or after the server serves.
pub fn stop(reason: &str) {
    info!(reason, "engine asked to stop");
    STOP.notify_one();
}

/// Resolves once [`stop`] is called.
pub(crate) async fn stopped() {
    STOP.notified().await;
}

/// Whether a request is the desktop shell's own call: the install key as its
/// bearer token, and no page behind it. A page in an app window reaches this
/// server through the shell's `neboapp://` proxy, which also carries the key
/// but always names the page's `Origin`; a signed-in browser holds a session,
/// never the key.
pub(crate) fn from_the_shell(headers: &HeaderMap) -> bool {
    let Some(key) = config::read_install_key().filter(|k| !k.is_empty()) else { return false };
    headers.get(header::ORIGIN).is_none()
        && crate::middleware::bearer(headers).is_some_and(|t| crate::handlers::ws::constant_time_eq(t, &key))
}

/// POST /api/v1/engine/quit: the shell's "Quit Nebo". The engine stops the
/// graceful way and exits 0, so no supervisor starts it again. The one stop
/// path on every OS: Windows has no SIGTERM.
pub async fn quit(headers: HeaderMap) -> Response {
    if !from_the_shell(&headers) {
        return (StatusCode::UNAUTHORIZED, "the install key is required").into_response();
    }
    stop("Quit Nebo");
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "stopping": true }))).into_response()
}

/// Keep the OS from slowing this process down while no window of it is in
/// front: it serves the local API and runs agents with no window at all.
/// Each opt-out logs a line saying it took. Neither keeps the machine awake.
pub(crate) fn hold_full_speed() {
    #[cfg(target_os = "macos")]
    hold_app_nap_opt_out();
    #[cfg(windows)]
    opt_out_of_power_throttling();
}

/// Keep macOS from napping Nebo. App Nap moved the whole process to
/// background priority 4 with throttled I/O and coalesced timers; under load
/// it got no CPU for minutes: no answer on :27895, even /health, and the
/// NeboAI write loop exited on "sleep drift" (60-182 s, 2026-10-07..09).
/// Held for the life of the process. `UserInitiatedAllowingIdleSystemSleep`
/// lets the Mac idle-sleep as before: no power assertion.
#[cfg(target_os = "macos")]
fn hold_app_nap_opt_out() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str("Nebo serves the local API and runs agents in the background"),
    );
    // Never ended: the activity lasts as long as the process.
    std::mem::forget(activity);
    info!("App Nap opt-out held (idle system sleep still allowed)");
}

/// Keep Windows 11 from putting Nebo in EcoQoS (efficiency mode) or
/// coarsening its timers while it has no window in front: the same
/// starvation App Nap caused on macOS. A state mask of 0 over both controls
/// turns both throttles off for this process. No power request is taken: the
/// PC still sleeps.
#[cfg(windows)]
fn opt_out_of_power_throttling() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
        SetProcessInformation,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: the pseudo-handle of this process and a correctly sized,
    // initialized struct that outlives the call.
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if ok != 0 {
        info!("power throttling opt-out held (EcoQoS and timer coarsening off; the PC still sleeps)");
    } else {
        tracing::warn!(error = %std::io::Error::last_os_error(), "power throttling opt-out not taken");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_tell_the_supervisor_what_happened() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(NeboError::PortInUse(27895))), EXIT_PORT_HELD);
        assert_eq!(exit_code(&Err(NeboError::Server("boom".into()))), 1);
        assert_ne!(EXIT_STALL, EXIT_PORT_HELD);
    }
}
