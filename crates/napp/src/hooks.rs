use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// Valid hook names.
pub const VALID_HOOKS: &[&str] = &[
    "tool.pre_execute",
    "tool.post_execute",
    "message.pre_send",
    "message.post_receive",
    "memory.pre_store",
    "memory.pre_recall",
    "session.message_append",
    "prompt.system_sections",
    "steering.generate",
    "response.stream",
    "agent.turn",
    "agent.should_continue",
];

/// Hook type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    /// Fire-and-forget, results discarded.
    Action,
    /// Chain payload through, can modify or handle.
    Filter,
}

/// Abstraction over how to call an app's hook handler.
/// Implemented by gRPC clients, in-process handlers, etc.
#[async_trait::async_trait]
pub trait HookCaller: Send + Sync {
    /// Call a filter hook. Returns (modified_payload, handled).
    async fn call_filter(&self, hook: &str, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String>;
    /// Call an action hook (fire-and-forget).
    async fn call_action(&self, hook: &str, payload: Vec<u8>) -> Result<(), String>;
}

/// A registered hook subscription.
struct HookSubscription {
    app_id: String,
    hook_type: HookType,
    priority: i32,
    consecutive_failures: u32,
    disabled: bool,
    disabled_at: Option<Instant>,
    caller: Arc<dyn HookCaller>,
}

/// Duration after which a disabled hook is automatically re-enabled.
const CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Hook dispatcher manages app hook subscriptions.
pub struct HookDispatcher {
    hooks: RwLock<HashMap<String, Vec<HookSubscription>>>,
    timeout: Duration,
    max_failures: u32,
}

impl HookDispatcher {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            timeout: Duration::from_millis(500),
            max_failures: 3,
        }
    }

    /// Register a hook subscription.
    pub fn register(
        &self,
        hook_name: &str,
        app_id: &str,
        hook_type: HookType,
        priority: i32,
        caller: Arc<dyn HookCaller>,
    ) {
        if !VALID_HOOKS.contains(&hook_name) {
            warn!(hook = hook_name, "unknown hook name");
            return;
        }

        let mut hooks = self.hooks.write().unwrap();
        let subs = hooks.entry(hook_name.to_string()).or_default();

        // Remove existing subscription from same app
        subs.retain(|s| s.app_id != app_id);

        subs.push(HookSubscription {
            app_id: app_id.to_string(),
            hook_type,
            priority,
            consecutive_failures: 0,
            disabled: false,
            disabled_at: None,
            caller,
        });

        // Sort by priority (lower = first)
        subs.sort_by_key(|s| s.priority);
    }

    /// Unregister all hooks for an app.
    pub fn unregister_app(&self, app_id: &str) {
        let mut hooks = self.hooks.write().unwrap();
        for subs in hooks.values_mut() {
            subs.retain(|s| s.app_id != app_id);
        }
    }

    /// Record a hook failure. Returns true if app should be disabled.
    pub fn record_failure(&self, hook_name: &str, app_id: &str) -> bool {
        let mut hooks = self.hooks.write().unwrap();
        if let Some(subs) = hooks.get_mut(hook_name) {
            for sub in subs.iter_mut() {
                if sub.app_id == app_id {
                    sub.consecutive_failures += 1;
                    if sub.consecutive_failures >= self.max_failures {
                        sub.disabled = true;
                        sub.disabled_at = Some(Instant::now());
                        warn!(
                            app = app_id,
                            hook = hook_name,
                            "hook disabled after {} consecutive failures (recovery in 5m)",
                            self.max_failures
                        );
                        return true;
                    }
                    return false;
                }
            }
        }
        false
    }

    /// Record a hook success (resets failure counter).
    pub fn record_success(&self, hook_name: &str, app_id: &str) {
        let mut hooks = self.hooks.write().unwrap();
        if let Some(subs) = hooks.get_mut(hook_name) {
            for sub in subs.iter_mut() {
                if sub.app_id == app_id {
                    sub.consecutive_failures = 0;
                    return;
                }
            }
        }
    }

    /// Re-enable hooks that have exceeded the 5-minute cooldown.
    fn recover_disabled(&self) {
        let mut hooks = self.hooks.write().unwrap();
        let now = Instant::now();
        for subs in hooks.values_mut() {
            for sub in subs.iter_mut() {
                if sub.disabled {
                    if let Some(at) = sub.disabled_at {
                        if now.duration_since(at) >= CIRCUIT_BREAKER_COOLDOWN {
                            sub.disabled = false;
                            sub.consecutive_failures = 0;
                            sub.disabled_at = None;
                            info!(
                                app = %sub.app_id,
                                "hook recovered after cooldown"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Get active subscribers for a hook (excluding disabled).
    pub fn subscribers(&self, hook_name: &str) -> Vec<(String, HookType)> {
        self.recover_disabled();
        let hooks = self.hooks.read().unwrap();
        hooks
            .get(hook_name)
            .map(|subs| {
                subs.iter()
                    .filter(|s| !s.disabled)
                    .map(|s| (s.app_id.clone(), s.hook_type))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the hook timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Check if any apps have hooks registered for a name.
    pub fn has_subscribers(&self, hook_name: &str) -> bool {
        self.recover_disabled();
        let hooks = self.hooks.read().unwrap();
        hooks
            .get(hook_name)
            .map(|subs| subs.iter().any(|s| !s.disabled))
            .unwrap_or(false)
    }

    /// Snapshot non-disabled subscribers for a hook+type. Releases lock before returning.
    fn active_subscribers(
        &self,
        hook_name: &str,
        hook_type: HookType,
    ) -> Vec<(String, Arc<dyn HookCaller>)> {
        self.recover_disabled();
        let hooks = self.hooks.read().unwrap();
        hooks
            .get(hook_name)
            .map(|subs| {
                subs.iter()
                    .filter(|s| !s.disabled && s.hook_type == hook_type)
                    .map(|s| (s.app_id.clone(), s.caller.clone()))
                    .collect()
            })
            .unwrap_or_default()
        // lock dropped here — before any async work
    }

    /// Apply a filter hook. Chains payload through all filter subscribers in priority order.
    /// Returns (final_payload, was_handled). On error, original payload preserved.
    pub async fn apply_filter(&self, hook_name: &str, payload: Vec<u8>) -> (Vec<u8>, bool) {
        let subs = self.active_subscribers(hook_name, HookType::Filter);
        if subs.is_empty() {
            return (payload, false);
        }

        let mut current = payload;
        for (app_id, caller) in subs {
            match tokio::time::timeout(self.timeout, caller.call_filter(hook_name, current.clone()))
                .await
            {
                Ok(Ok((new_payload, handled))) => {
                    self.record_success(hook_name, &app_id);
                    if handled {
                        return (new_payload, true);
                    }
                    current = new_payload;
                }
                Ok(Err(e)) => {
                    warn!(app = %app_id, hook = hook_name, error = %e, "hook filter error");
                    self.record_failure(hook_name, &app_id);
                    // On error, keep current payload (don't corrupt chain)
                }
                Err(_) => {
                    warn!(
                        app = %app_id,
                        hook = hook_name,
                        "hook filter timed out ({}ms)",
                        self.timeout.as_millis()
                    );
                    self.record_failure(hook_name, &app_id);
                }
            }
        }
        (current, false)
    }

    /// Fire an action hook to all action subscribers. Errors logged, not propagated.
    pub async fn do_action(&self, hook_name: &str, payload: Vec<u8>) {
        let subs = self.active_subscribers(hook_name, HookType::Action);
        for (app_id, caller) in subs {
            match tokio::time::timeout(self.timeout, caller.call_action(hook_name, payload.clone()))
                .await
            {
                Ok(Ok(())) => self.record_success(hook_name, &app_id),
                Ok(Err(e)) => {
                    warn!(app = %app_id, hook = hook_name, error = %e, "hook action error");
                    self.record_failure(hook_name, &app_id);
                }
                Err(_) => {
                    warn!(
                        app = %app_id,
                        hook = hook_name,
                        "hook action timed out ({}ms)",
                        self.timeout.as_millis()
                    );
                    self.record_failure(hook_name, &app_id);
                }
            }
        }
    }
}

impl Default for HookDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock caller for testing.
    struct NoopCaller;

    #[async_trait::async_trait]
    impl HookCaller for NoopCaller {
        async fn call_filter(
            &self,
            _hook: &str,
            payload: Vec<u8>,
        ) -> Result<(Vec<u8>, bool), String> {
            Ok((payload, false))
        }
        async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
            Ok(())
        }
    }

    fn noop() -> Arc<dyn HookCaller> {
        Arc::new(NoopCaller)
    }

    #[test]
    fn test_register_and_list() {
        let d = HookDispatcher::new();
        d.register("tool.pre_execute", "app-1", HookType::Filter, 10, noop());
        d.register("tool.pre_execute", "app-2", HookType::Action, 20, noop());

        let subs = d.subscribers("tool.pre_execute");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].0, "app-1"); // lower priority first
    }

    #[test]
    fn test_circuit_breaker() {
        let d = HookDispatcher::new();
        d.register("tool.pre_execute", "app-1", HookType::Filter, 10, noop());

        assert!(!d.record_failure("tool.pre_execute", "app-1"));
        assert!(!d.record_failure("tool.pre_execute", "app-1"));
        assert!(d.record_failure("tool.pre_execute", "app-1")); // 3rd failure

        // Now disabled
        assert!(d.subscribers("tool.pre_execute").is_empty());
    }

    #[test]
    fn test_success_resets_failures() {
        let d = HookDispatcher::new();
        d.register("tool.pre_execute", "app-1", HookType::Filter, 10, noop());

        d.record_failure("tool.pre_execute", "app-1");
        d.record_failure("tool.pre_execute", "app-1");
        d.record_success("tool.pre_execute", "app-1");

        // Not disabled after success reset
        assert!(!d.subscribers("tool.pre_execute").is_empty());
    }

    /// Mock caller that appends " modified" to the payload.
    struct ModifyCaller;

    #[async_trait::async_trait]
    impl HookCaller for ModifyCaller {
        async fn call_filter(
            &self,
            _hook: &str,
            mut payload: Vec<u8>,
        ) -> Result<(Vec<u8>, bool), String> {
            payload.extend_from_slice(b" modified");
            Ok((payload, false))
        }
        async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
            Ok(())
        }
    }

    /// Mock caller that returns handled=true.
    struct HandledCaller;

    #[async_trait::async_trait]
    impl HookCaller for HandledCaller {
        async fn call_filter(
            &self,
            _hook: &str,
            _payload: Vec<u8>,
        ) -> Result<(Vec<u8>, bool), String> {
            Ok((b"handled".to_vec(), true))
        }
        async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
            Ok(())
        }
    }

    /// Mock caller that always errors.
    struct ErrorCaller;

    #[async_trait::async_trait]
    impl HookCaller for ErrorCaller {
        async fn call_filter(
            &self,
            _hook: &str,
            _payload: Vec<u8>,
        ) -> Result<(Vec<u8>, bool), String> {
            Err("boom".to_string())
        }
        async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
            Err("boom".to_string())
        }
    }

    /// Mock caller that tracks calls via shared counter.
    struct CountCaller {
        count: Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait::async_trait]
    impl HookCaller for CountCaller {
        async fn call_filter(
            &self,
            _hook: &str,
            payload: Vec<u8>,
        ) -> Result<(Vec<u8>, bool), String> {
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok((payload, false))
        }
        async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_apply_filter_modifies_payload() {
        let d = HookDispatcher::new();
        d.register(
            "message.pre_send",
            "app-1",
            HookType::Filter,
            10,
            Arc::new(ModifyCaller),
        );

        let (result, handled) = d.apply_filter("message.pre_send", b"hello".to_vec()).await;
        assert!(!handled);
        assert_eq!(result, b"hello modified");
    }

    #[tokio::test]
    async fn test_apply_filter_handled_stops_chain() {
        let d = HookDispatcher::new();
        d.register(
            "message.pre_send",
            "app-1",
            HookType::Filter,
            10,
            Arc::new(HandledCaller),
        );
        d.register(
            "message.pre_send",
            "app-2",
            HookType::Filter,
            20,
            Arc::new(ModifyCaller),
        );

        let (result, handled) = d.apply_filter("message.pre_send", b"hello".to_vec()).await;
        assert!(handled);
        assert_eq!(result, b"handled"); // app-2 never called
    }

    #[tokio::test]
    async fn test_apply_filter_error_preserves_payload() {
        let d = HookDispatcher::new();
        d.register(
            "message.pre_send",
            "app-1",
            HookType::Filter,
            10,
            Arc::new(ErrorCaller),
        );

        let (result, handled) = d.apply_filter("message.pre_send", b"original".to_vec()).await;
        assert!(!handled);
        assert_eq!(result, b"original");
    }

    #[tokio::test]
    async fn test_apply_filter_error_triggers_circuit_breaker() {
        let d = HookDispatcher::new();
        d.register(
            "message.pre_send",
            "app-1",
            HookType::Filter,
            10,
            Arc::new(ErrorCaller),
        );

        // 3 failures disable the hook
        d.apply_filter("message.pre_send", b"a".to_vec()).await;
        d.apply_filter("message.pre_send", b"b".to_vec()).await;
        d.apply_filter("message.pre_send", b"c".to_vec()).await;

        assert!(!d.has_subscribers("message.pre_send"));
    }

    #[tokio::test]
    async fn test_do_action_calls_all_subscribers() {
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let d = HookDispatcher::new();
        d.register(
            "session.message_append",
            "app-1",
            HookType::Action,
            10,
            Arc::new(CountCaller {
                count: count.clone(),
            }),
        );
        d.register(
            "session.message_append",
            "app-2",
            HookType::Action,
            20,
            Arc::new(CountCaller {
                count: count.clone(),
            }),
        );

        d.do_action("session.message_append", b"payload".to_vec())
            .await;
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_do_action_ignores_errors() {
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let d = HookDispatcher::new();
        d.register(
            "session.message_append",
            "app-1",
            HookType::Action,
            10,
            Arc::new(ErrorCaller),
        );
        d.register(
            "session.message_append",
            "app-2",
            HookType::Action,
            20,
            Arc::new(CountCaller {
                count: count.clone(),
            }),
        );

        d.do_action("session.message_append", b"payload".to_vec())
            .await;
        // app-2 still called despite app-1 error
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
