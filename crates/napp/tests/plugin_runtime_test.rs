use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nebo_napp::plugin::{
    PluginManifest, PluginPermissions, PlatformBinary, PluginStore, current_platform_key,
};
use nebo_napp::PluginRuntime;

fn fake_plugin_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // repo root
    path.push("target");
    path.push("debug");
    path.push("examples");
    path.push("fake_plugin");
    assert!(
        path.exists(),
        "fake_plugin binary not found at {}. Run `cargo build --example fake_plugin -p nebo-napp` first.",
        path.display()
    );
    path
}

fn test_manifest(slug: &str, binary_name: &str) -> PluginManifest {
    let platform = current_platform_key();
    let mut platforms = HashMap::new();
    platforms.insert(
        platform,
        PlatformBinary {
            binary_name: binary_name.to_string(),
            sha256: String::new(),
            signature: String::new(),
            size: 0,
            download_url: String::new(),
        },
    );

    PluginManifest {
        id: "test-id".to_string(),
        slug: slug.to_string(),
        name: "Test Plugin".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        author: String::new(),
        platforms,
        signing_key_id: String::new(),
        env_var: String::new(),
        auth: None,
        events: None,
        dependencies: Vec::new(),
        capabilities: None,
        permissions: None,
        category: String::new(),
        triggers: Vec::new(),
        channel: None,
        setup: None,
    }
}

fn setup_plugin_store(slug: &str, binary: &std::path::Path) -> (tempfile::TempDir, Arc<PluginStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let installed_dir = tmp.path().join("installed");
    let user_dir = tmp.path().join("user");

    let slug_dir = installed_dir.join(slug);
    std::fs::create_dir_all(&slug_dir).unwrap();

    // Copy binary into plugin directory
    let dest = slug_dir.join("fake_plugin");
    std::fs::copy(binary, &dest).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Write manifest
    let manifest = test_manifest(slug, "fake_plugin");
    let manifest_path = slug_dir.join("plugin.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    std::fs::create_dir_all(&user_dir).unwrap();

    let store = Arc::new(PluginStore::new(installed_dir, user_dir, None));
    (tmp, store)
}

fn setup_plugin_store_with_permissions(
    slug: &str,
    binary: &std::path::Path,
    permissions: PluginPermissions,
) -> (tempfile::TempDir, Arc<PluginStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let installed_dir = tmp.path().join("installed");
    let user_dir = tmp.path().join("user");

    let slug_dir = installed_dir.join(slug);
    std::fs::create_dir_all(&slug_dir).unwrap();

    let dest = slug_dir.join("fake_plugin");
    std::fs::copy(binary, &dest).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut manifest = test_manifest(slug, "fake_plugin");
    manifest.permissions = Some(permissions);

    let manifest_path = slug_dir.join("plugin.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    std::fs::create_dir_all(&user_dir).unwrap();

    let store = Arc::new(PluginStore::new(installed_dir, user_dir, None));
    (tmp, store)
}

#[tokio::test]
async fn env_sanitization_strips_dangerous_vars() {
    let binary = fake_plugin_binary();
    let (_tmp, store) = setup_plugin_store("testplugin", &binary);
    let resolved = store.resolve("testplugin", "*").unwrap();

    let runtime = PluginRuntime::new("testplugin", resolved, store);

    unsafe {
        std::env::set_var("LD_PRELOAD", "/evil/lib.so");
        std::env::set_var("DYLD_INSERT_LIBRARIES", "/evil/lib.dylib");
        std::env::set_var("NODE_OPTIONS", "--require /evil.js");
    }

    let mut cmd = runtime.command("echo-env");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success(), "fake_plugin exited non-zero");

    let env: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).unwrap();

    assert!(!env.contains_key("LD_PRELOAD"), "LD_PRELOAD should be stripped");
    assert!(!env.contains_key("DYLD_INSERT_LIBRARIES"), "DYLD_INSERT_LIBRARIES should be stripped");
    assert!(!env.contains_key("NODE_OPTIONS"), "NODE_OPTIONS should be stripped");

    unsafe {
        std::env::remove_var("LD_PRELOAD");
        std::env::remove_var("DYLD_INSERT_LIBRARIES");
        std::env::remove_var("NODE_OPTIONS");
    }
}

#[tokio::test]
async fn env_deny_enforcement() {
    let binary = fake_plugin_binary();
    let permissions = PluginPermissions {
        env_allow: Vec::new(),
        env_deny: vec!["TEST_SECRET".to_string(), "ANOTHER_SECRET".to_string()],
        network: false,
        max_timeout_seconds: 300,
    };
    let (_tmp, store) = setup_plugin_store_with_permissions("denyplugin", &binary, permissions);
    let resolved = store.resolve("denyplugin", "*").unwrap();

    unsafe {
        std::env::set_var("TEST_SECRET", "supersecret");
        std::env::set_var("ANOTHER_SECRET", "alsosecret");
        std::env::set_var("SAFE_VAR", "ok");
    }

    let runtime = PluginRuntime::new("denyplugin", resolved, store).with_permissions();
    let mut cmd = runtime.command("echo-env");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success());

    let env: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).unwrap();

    assert!(!env.contains_key("TEST_SECRET"), "TEST_SECRET should be denied");
    assert!(!env.contains_key("ANOTHER_SECRET"), "ANOTHER_SECRET should be denied");
    assert!(env.contains_key("SAFE_VAR"), "SAFE_VAR should pass through");

    unsafe {
        std::env::remove_var("TEST_SECRET");
        std::env::remove_var("ANOTHER_SECRET");
        std::env::remove_var("SAFE_VAR");
    }
}

#[tokio::test]
async fn env_allow_enforcement() {
    let binary = fake_plugin_binary();
    let permissions = PluginPermissions {
        env_allow: vec!["PATH".to_string(), "HOME".to_string(), "ALLOWED_VAR".to_string()],
        env_deny: Vec::new(),
        network: false,
        max_timeout_seconds: 300,
    };
    let (_tmp, store) = setup_plugin_store_with_permissions("allowplugin", &binary, permissions);
    let resolved = store.resolve("allowplugin", "*").unwrap();

    unsafe {
        std::env::set_var("ALLOWED_VAR", "yes");
        std::env::set_var("BLOCKED_VAR", "no");
    }

    let runtime = PluginRuntime::new("allowplugin", resolved, store).with_permissions();
    let mut cmd = runtime.command("echo-env");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success());

    let env: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).unwrap();

    assert!(env.contains_key("ALLOWED_VAR"), "ALLOWED_VAR should pass");
    assert!(!env.contains_key("BLOCKED_VAR"), "BLOCKED_VAR should be blocked by allow-list");

    unsafe {
        std::env::remove_var("ALLOWED_VAR");
        std::env::remove_var("BLOCKED_VAR");
    }
}

#[test]
fn max_timeout_enforcement() {
    let binary = fake_plugin_binary();
    let permissions = PluginPermissions {
        env_allow: Vec::new(),
        env_deny: Vec::new(),
        network: false,
        max_timeout_seconds: 5,
    };
    let (_tmp, store) = setup_plugin_store_with_permissions("tmpplugin", &binary, permissions);
    let resolved = store.resolve("tmpplugin", "*").unwrap();

    let runtime = PluginRuntime::new("tmpplugin", resolved, store).with_permissions();
    let effective = runtime.effective_timeout(Duration::from_secs(60));
    assert_eq!(effective, Duration::from_secs(5), "should cap at manifest max");
}

#[test]
fn timeout_passthrough_without_permissions() {
    let binary = fake_plugin_binary();
    let (_tmp, store) = setup_plugin_store("noperm", &binary);
    let resolved = store.resolve("noperm", "*").unwrap();

    let runtime = PluginRuntime::new("noperm", resolved, store);
    let effective = runtime.effective_timeout(Duration::from_secs(60));
    assert_eq!(effective, Duration::from_secs(60), "without permissions, should pass through");
}

#[tokio::test]
async fn shlex_arg_parsing() {
    let binary = fake_plugin_binary();
    let (_tmp, store) = setup_plugin_store("argtest", &binary);
    let resolved = store.resolve("argtest", "*").unwrap();

    let runtime = PluginRuntime::new("argtest", resolved, store);
    let mut cmd = runtime.command("echo-args --scope \"read write\" --flag");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success());

    let args: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(args, vec!["--scope", "read write", "--flag"]);
}

#[tokio::test]
async fn plugin_binary_env_var_injected() {
    let binary = fake_plugin_binary();
    let (_tmp, store) = setup_plugin_store("myplugin", &binary);
    let resolved = store.resolve("myplugin", "*").unwrap();

    let runtime = PluginRuntime::new("myplugin", resolved.clone(), store);
    let mut cmd = runtime.command("echo-env");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success());

    let env: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        env.get("MYPLUGIN_BIN"),
        Some(&resolved.to_string_lossy().into_owned()),
        "MYPLUGIN_BIN should point to the binary"
    );
    assert!(env.contains_key("NEBO_PLUGIN_DATA"), "NEBO_PLUGIN_DATA should be set");
    assert!(env.contains_key("MYPLUGIN_DATA"), "MYPLUGIN_DATA should be set");
}

#[tokio::test]
async fn auth_env_injection() {
    let binary = fake_plugin_binary();
    let (_tmp, store) = setup_plugin_store("authtest", &binary);

    store.set_env_var("authtest", "GWS_CLIENT_ID", "test-client-id");
    store.set_env_var("authtest", "GWS_CLIENT_SECRET", "test-secret");

    let resolved = store.resolve("authtest", "*").unwrap();
    let runtime = PluginRuntime::new("authtest", resolved, store);
    let mut cmd = runtime.command("echo-env");
    let output = cmd.output().await.unwrap();
    assert!(output.status.success());

    let env: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(env.get("GWS_CLIENT_ID").map(|s| s.as_str()), Some("test-client-id"));
    assert_eq!(env.get("GWS_CLIENT_SECRET").map(|s| s.as_str()), Some("test-secret"));
}
