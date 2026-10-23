//! The computer going to sleep and waking, as the OS tells this process,
//! for the two connections to NeboAI: the comms watcher and the tunnel
//! watcher (`lib.rs`) each hold a [`PowerWatch`].
//!
//! A sleeping Mac wakes in the dark about every 15 minutes (DarkWake) for
//! 15-30 s. Nebo redialed during each one, the Mac slept again with both
//! connections open, and the hub learned it a minute later from a missed
//! keepalive, showing the bot online meanwhile (gateway logs and
//! `pmset -g log`, 2026-10-22/23). Now the OS says so before it sleeps: each
//! watcher closes its connection with 1001 "sleep"
//! (`comm::reconnect::Disconnect::Sleep`) and dials nothing until the
//! computer wakes, then redials at once.
//!
//! - macOS: IOKit's system power messages (`IORegisterForSystemPower`),
//!   which come for a DarkWake too. The sleep is allowed once the closes
//!   are out, or after [`CLOSE_LIMIT`].
//! - Windows: `PowerRegisterSuspendResumeNotification`, a callback with no
//!   window. The suspend waits the same way.
//! - Linux: systemd-logind's `PrepareForSleep`, held off by a delay
//!   inhibitor until the closes are out.
//!
//! Where the OS says nothing (no logind: a container, a cloud bot) nothing
//! is heard, and the clock-gap checks (`types::stall`, the comms watcher's
//! drift check) still notice a sleep after the fact.

use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

// The longest the OS's sleep is held for the closes (`comm::reconnect`).
use comm::reconnect::SLEEP_CLOSE_LIMIT as CLOSE_LIMIT;

/// The longest a watcher waits for a wake the OS never sent (logind
/// restarted mid-sleep): a missed notice must not keep the bot offline.
/// On macOS and Linux the clock behind it stops while the computer sleeps;
/// on Windows it keeps counting, so it may run out during a long suspend,
/// and the redial then comes right after the resume, as the notice would
/// have made it.
const WAKE_LIMIT: Duration = Duration::from_secs(60);

/// The power state every watcher follows.
pub(crate) struct Power {
    /// True from the sleep notice until the wake.
    asleep: watch::Sender<bool>,
    /// Watchers yet to close their connection for the sleep under way.
    closing: Mutex<usize>,
    closed: Condvar,
}

/// This process's power state, fed by the OS once [`start`] has run.
pub(crate) fn process() -> &'static Power {
    static POWER: LazyLock<Power> = LazyLock::new(Power::new);
    &POWER
}

/// Listen for the OS's sleep and wake notices, once per process.
pub(crate) fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        #[cfg(target_os = "macos")]
        mac::listen();
        #[cfg(windows)]
        win::listen();
        #[cfg(target_os = "linux")]
        linux::listen();
    });
}

impl Power {
    fn new() -> Self {
        Self {
            asleep: watch::Sender::new(false),
            closing: Mutex::new(0),
            closed: Condvar::new(),
        }
    }

    /// A watcher of the power state, held by one connection's watcher for
    /// its life: each sleep waits for every watcher alive to close.
    pub(crate) fn watch(&'static self) -> PowerWatch {
        PowerWatch {
            power: self,
            rx: self.asleep.subscribe(),
        }
    }

    /// The computer is about to sleep: tell every watcher, and return once
    /// each has closed its connection, or after [`CLOSE_LIMIT`]. Blocks: it
    /// runs on the OS's notification thread, which the sleep waits on.
    pub(crate) fn will_sleep(&self) {
        let mut closing = self.closing.lock().unwrap_or_else(|e| e.into_inner());
        *closing = self.asleep.receiver_count();
        self.asleep.send_replace(true);
        info!(
            connections = *closing,
            "power: the computer is going to sleep; closing the connections to NeboAI"
        );
        let (mut closing, waited) = self
            .closed
            .wait_timeout_while(closing, CLOSE_LIMIT, |left| *left > 0)
            .unwrap_or_else(|e| e.into_inner());
        if waited.timed_out() {
            warn!(
                left = *closing,
                "power: sleeping before every connection to NeboAI closed"
            );
        }
        *closing = 0;
    }

    /// The computer is awake: every watcher redials.
    pub(crate) fn did_wake(&self) {
        if self.asleep.send_replace(false) {
            info!("power: the computer woke; redialing NeboAI");
        }
    }
}

/// One connection's view of the power state.
pub(crate) struct PowerWatch {
    power: &'static Power,
    rx: watch::Receiver<bool>,
}

impl PowerWatch {
    /// Resolves once the computer is about to sleep: at once while it is.
    pub(crate) async fn sleeping(&mut self) {
        let _ = self.rx.wait_for(|asleep| *asleep).await;
    }

    /// Whether the computer is about to sleep, or asleep: no dial now.
    pub(crate) fn is_asleep(&self) -> bool {
        *self.rx.borrow()
    }

    /// Wait on `wait` while this watcher holds no connection. A sleep that
    /// comes meanwhile has nothing of this watcher's to wait for: it goes
    /// ahead at once, and `wait` resumes after the wake.
    pub(crate) async fn idle<T>(&mut self, wait: impl std::future::Future<Output = T>) -> T {
        let mut wait = std::pin::pin!(wait);
        loop {
            tokio::select! {
                out = &mut wait => return out,
                () = self.sleeping() => self.sleep_through().await,
            }
        }
    }

    /// This watcher's connection is closed (or was not open): let the sleep
    /// go ahead, and resolve once the computer is awake again.
    pub(crate) async fn sleep_through(&mut self) {
        {
            let mut closing = self.power.closing.lock().unwrap_or_else(|e| e.into_inner());
            *closing = closing.saturating_sub(1);
            self.power.closed.notify_all();
        }
        let woke = tokio::time::timeout(WAKE_LIMIT, self.rx.wait_for(|asleep| !*asleep)).await;
        if woke.is_err() {
            warn!("power: no wake notice from the OS; taking the computer as awake");
            self.power.did_wake();
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tracing::warn;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            refcon: *mut c_void,
            port: *mut *mut c_void,
            callback: extern "C" fn(*mut c_void, u32, u32, *mut c_void),
            notifier: *mut u32,
        ) -> u32;
        fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> *mut c_void;
        fn IOAllowPowerChange(kernel_port: u32, notification_id: isize) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopGetCurrent() -> *mut c_void;
        fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
        fn CFRunLoopRun();
    }

    /// `kIOMessageCanSystemSleep`: idle sleep asks first; always allowed.
    const CAN_SYSTEM_SLEEP: u32 = 0xe000_0270;
    /// `kIOMessageSystemWillSleep`: the sleep is coming, once allowed.
    const SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;
    /// `kIOMessageSystemHasPoweredOn`: awake, a DarkWake included.
    const SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;

    /// The root power domain's port, which answers each sleep message.
    static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

    extern "C" fn on_power(
        _refcon: *mut c_void,
        _service: u32,
        message: u32,
        argument: *mut c_void,
    ) {
        let allow = || {
            // SAFETY: answers the message IOKit just sent on the port it gave us.
            unsafe { IOAllowPowerChange(ROOT_PORT.load(Ordering::Relaxed), argument as isize) };
        };
        match message {
            CAN_SYSTEM_SLEEP => allow(),
            SYSTEM_WILL_SLEEP => {
                super::process().will_sleep();
                allow();
            }
            SYSTEM_HAS_POWERED_ON => super::process().did_wake(),
            _ => {}
        }
    }

    /// Register for the system's power messages on a thread of their own,
    /// running its run loop for the life of the process.
    pub(super) fn listen() {
        let spawned = std::thread::Builder::new().name("nebo-power".into()).spawn(|| {
            let mut port = std::ptr::null_mut();
            let mut notifier = 0;
            // SAFETY: plain IOKit/CoreFoundation calls with valid out-pointers;
            // the port and its run loop source live as long as this thread.
            unsafe {
                let root = IORegisterForSystemPower(std::ptr::null_mut(), &mut port, on_power, &mut notifier);
                if root == 0 {
                    warn!("power: could not register for sleep notices; sleep is noticed after the fact");
                    return;
                }
                ROOT_PORT.store(root, Ordering::Relaxed);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), IONotificationPortGetRunLoopSource(port), kCFRunLoopDefaultMode);
                CFRunLoopRun();
            }
        });
        if let Err(e) = spawned {
            warn!(error = %e, "power: could not start the sleep notice thread");
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use tracing::warn;
    use windows_sys::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    };

    unsafe extern "system" fn on_power(
        _context: *const c_void,
        kind: u32,
        _setting: *const c_void,
    ) -> u32 {
        match kind {
            PBT_APMSUSPEND => super::process().will_sleep(),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => super::process().did_wake(),
            _ => {}
        }
        0
    }

    /// Register the suspend and resume callback for the life of the process.
    pub(super) fn listen() {
        // The registration keeps pointing at these for as long as it lasts:
        // the life of the process.
        let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
            Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(on_power),
                Context: std::ptr::null_mut(),
            }));
        let mut registration = std::ptr::null_mut();
        // SAFETY: `params` is valid for the life of the process, and
        // `registration` is a valid out-pointer.
        let err = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                (params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(),
                &mut registration,
            )
        };
        if err != 0 {
            warn!(
                err,
                "power: could not register for sleep notices; sleep is noticed after the fact"
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use futures::StreamExt;
    use tracing::debug;

    /// Follow logind's `PrepareForSleep` on the system bus, if there is one.
    pub(super) fn listen() {
        tokio::spawn(async {
            if let Err(e) = follow_logind().await {
                debug!(error = %e, "power: no logind; sleep is noticed after the fact");
            }
        });
    }

    async fn follow_logind() -> zbus::Result<()> {
        let bus = zbus::Connection::system().await?;
        let logind = zbus::Proxy::new(
            &bus,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await?;
        let mut notices = logind.receive_signal("PrepareForSleep").await?;
        let mut held = delay_sleep(&logind).await;
        while let Some(notice) = notices.next().await {
            let Ok(going) = notice.body().deserialize::<bool>() else {
                continue;
            };
            if going {
                let _ = tokio::task::spawn_blocking(|| super::process().will_sleep()).await;
                // The closes are out: let the system sleep.
                drop(held.take());
            } else {
                super::process().did_wake();
                held = delay_sleep(&logind).await;
            }
        }
        Ok(())
    }

    /// A delay inhibitor: logind holds the next sleep (up to its
    /// `InhibitDelayMaxSec`) until it is dropped. Without one (not
    /// permitted, as for some system services) the notice still comes,
    /// and the closes race the sleep.
    async fn delay_sleep(logind: &zbus::Proxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
        let why = (
            "sleep",
            "Nebo",
            "Closing its connections to NeboAI",
            "delay",
        );
        match logind.call("Inhibit", &why).await {
            Ok(fd) => Some(fd),
            Err(e) => {
                debug!(error = %e, "power: logind would not hold the sleep for Nebo");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power() -> &'static Power {
        Box::leak(Box::new(Power::new()))
    }

    /// The sleep notice reaches every watcher and waits for each to close
    /// its connection; the wake reaches them all.
    #[tokio::test]
    async fn a_sleep_waits_for_every_watcher_to_close_then_the_wake_releases_them() {
        let power = power();
        let watchers = [power.watch(), power.watch()];
        let notice = tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            power.will_sleep();
            started.elapsed()
        });
        let mut slept = Vec::new();
        for mut watch in watchers {
            slept.push(tokio::spawn(async move {
                watch.sleeping().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                watch.sleep_through().await;
            }));
        }
        let waited = notice.await.unwrap();
        assert!(
            waited >= Duration::from_millis(50) && waited < CLOSE_LIMIT,
            "{waited:?}"
        );
        assert!(
            slept.iter().all(|w| !w.is_finished()),
            "asleep until the wake"
        );
        power.did_wake();
        for w in slept {
            tokio::time::timeout(Duration::from_secs(1), w)
                .await
                .expect("woke")
                .unwrap();
        }
    }

    /// A watcher waiting with no connection open lets a sleep go ahead at
    /// once, and its wait goes on after the wake.
    #[tokio::test]
    async fn an_idle_watcher_does_not_hold_the_sleep() {
        let power = power();
        let mut watch = power.watch();
        let (done_tx, done) = tokio::sync::oneshot::channel::<u8>();
        let waiting = tokio::spawn(async move { watch.idle(done).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let started = std::time::Instant::now();
        tokio::task::spawn_blocking(move || power.will_sleep())
            .await
            .unwrap();
        assert!(
            started.elapsed() < CLOSE_LIMIT / 2,
            "{:?}",
            started.elapsed()
        );
        power.did_wake();
        done_tx.send(7).unwrap();
        let out = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("the wait went on")
            .unwrap();
        assert_eq!(out.unwrap(), 7);
    }

    /// A watcher that never closes holds the sleep only [`CLOSE_LIMIT`].
    #[tokio::test]
    async fn a_sleep_is_held_at_most_the_close_limit() {
        let power = power();
        let _stuck = power.watch();
        let started = std::time::Instant::now();
        tokio::task::spawn_blocking(move || power.will_sleep())
            .await
            .unwrap();
        let waited = started.elapsed();
        assert!(
            waited >= CLOSE_LIMIT && waited < CLOSE_LIMIT * 2,
            "{waited:?}"
        );
    }
}
