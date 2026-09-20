//! The ONE writer for a user-owned employee's package on disk.
//!
//! `user/agents/<dir>/` holds `agent.json`, `manifest.json`, `AGENT.md` and,
//! for an app, `ui/`. Every door that makes or reshapes such a package — the
//! registry tool's create and update, the REST create and duplicate — goes
//! through [`write_user_agent`], so the composition rules (what marks an app,
//! what its permissions default to, which file is written last) live once.
//! Before this there were two private writers, and the REST one did not know
//! apps: duplicating an app produced a plain employee (2026-09-19).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::NappError;
use crate::agent::parse_agent;
use crate::manifest::{AppWindowConfig, validate_permissions};

/// The one permission an app gets when nobody names any: its own key-value
/// store, what nearly every app page touches first.
pub const APP_DEFAULT_PERMISSIONS: &[&str] = &["storage:readwrite"];

/// What makes an employee an app: the manifest marker, and the window and
/// permissions its page runs with. `None` for a field means "leave what the
/// manifest already says" (an update's `app: {}` keeps the permissions on
/// disk); a fresh manifest gets the defaults.
#[derive(Debug, Clone, Default)]
pub struct AppFields {
    pub window: Option<AppWindowConfig>,
    pub permissions: Option<Vec<String>>,
}

impl AppFields {
    /// From a tool call's `app` block, checked with the loader's own rules
    /// (`AppWindowConfig`, `validate_permissions`) before anything is written.
    pub fn from_json(app: &Value) -> Result<Self, NappError> {
        let Some(obj) = app.as_object() else {
            return Err(NappError::Other(
                "`app` must be an object: {\"window\": {\"title\": \"...\", \"width\": 900, \"height\": 700}, \"permissions\": [\"storage:readwrite\"]} (both fields optional; {} is fine). Nothing was written."
                    .into(),
            ));
        };
        let window = match obj.get("window") {
            None => None,
            Some(window) if !window.is_object() => {
                return Err(NappError::Other(
                    "`app.window` must be an object: {\"title\", \"width\", \"height\", \"resizable\"}. Nothing was written."
                        .into(),
                ));
            }
            Some(window) => Some(
                serde_json::from_value::<AppWindowConfig>(window.clone())
                    .map_err(|e| NappError::Other(format!("`app.window` is invalid and was not saved: {e}")))?,
            ),
        };
        let permissions = match obj.get("permissions") {
            None => None,
            Some(perms) => {
                let permissions = perms
                    .as_array()
                    .ok_or_else(|| {
                        NappError::Other(
                            "`app.permissions` must be an array of strings, e.g. [\"storage:readwrite\", \"network:outbound\"]. Nothing was written."
                                .into(),
                        )
                    })?
                    .iter()
                    .map(|p| {
                        p.as_str().map(String::from).ok_or_else(|| {
                            NappError::Other(format!(
                                "`app.permissions` entry {p} must be a string like \"storage:readwrite\". Nothing was written."
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                validate_permissions(&permissions)
                    .map_err(|e| NappError::Other(format!("`app.permissions` refused and nothing was written: {e}")))?;
                Some(permissions)
            }
        };
        Ok(Self { window, permissions })
    }

    /// From a manifest already on disk: `Some` when it marks an app, carrying
    /// the window and permissions the loader would read from it. What a
    /// duplicate copies.
    pub fn from_manifest(manifest: &Value) -> Option<Self> {
        let obj = manifest.as_object()?;
        if !manifest_is_app(obj) {
            return None;
        }
        let window = obj
            .get("window")
            .and_then(|w| serde_json::from_value::<AppWindowConfig>(w.clone()).ok());
        let permissions = obj.get("permissions").and_then(|p| p.as_array()).map(|a| {
            a.iter().filter_map(|p| p.as_str().map(String::from)).collect::<Vec<_>>()
        });
        Some(Self { window, permissions })
    }
}

/// True when a manifest already marks its employee as an app, in either
/// spelling the loader reads (`type`, the NeboLoop one; `artifact_type`, the
/// older one).
pub fn manifest_is_app(manifest: &Map<String, Value>) -> bool {
    ["artifact_type", "type"]
        .iter()
        .any(|k| manifest.get(*k).and_then(|v| v.as_str()) == Some("app"))
}

/// Everything a user-owned employee's directory holds.
#[derive(Debug, Clone)]
pub struct AgentPackage<'a> {
    /// The DB row's UUID. Written into a manifest that has none, so the
    /// loader keys the directory by it instead of minting a second one.
    pub id: &'a str,
    /// The name the Employees list shows (manifest.json `name`).
    pub name: &'a str,
    pub description: &'a str,
    /// The persona. Checked with the loader's parser before a byte lands.
    pub agent_md: &'a str,
    /// Workflow bindings, triggers, skills — `agent.json` — when there are any.
    pub agent_json: Option<&'a str>,
    /// `Some` makes the employee an app. `None` with `ui` files still means
    /// an app: nobody writes a page for an employee that is never served.
    pub app: Option<AppFields>,
    /// The page: paths relative to `ui/` (see [`ui_relative_path`]) and bytes.
    pub ui: Vec<(PathBuf, Vec<u8>)>,
}

/// A `ui` path, relative to the app's `ui/` directory and staying inside
/// it: no `..`, no root, no drive, no backslashes.
pub fn ui_relative_path(path: &str) -> Result<PathBuf, NappError> {
    use std::path::Component;
    let refuse = |why: &str| {
        Err(NappError::Other(format!(
            "`ui` path \"{path}\" refused ({why}); nothing was written. Paths are relative to the app's ui/ directory, e.g. \"index.html\" or \"assets/app.js\"."
        )))
    };
    if path.trim().is_empty() {
        return refuse("empty");
    }
    if path.contains('\\') {
        return refuse("backslashes are not allowed");
    }
    let p = Path::new(path);
    let mut normal = 0;
    for component in p.components() {
        match component {
            Component::Normal(_) => normal += 1,
            Component::CurDir => {}
            Component::ParentDir => return refuse("`..` is not allowed"),
            Component::RootDir | Component::Prefix(_) => return refuse("absolute paths are not allowed"),
        }
    }
    if normal == 0 {
        return refuse("names no file");
    }
    Ok(p.to_path_buf())
}

/// The page files under `<dir>/ui/`, as a package carries them: paths
/// relative to `ui/`, bytes as they are. An employee without a `ui/`
/// directory has none.
pub fn read_ui_files(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, NappError> {
    let ui_dir = dir.join("ui");
    let mut files = Vec::new();
    if !ui_dir.is_dir() {
        return Ok(files);
    }
    let mut pending = vec![ui_dir.clone()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(rel) = path.strip_prefix(&ui_dir) {
                files.push((rel.to_path_buf(), std::fs::read(&path)?));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Write a user-owned employee's package to `dir`, which is created when
/// missing. A manifest already there is kept and layered on (its `id` and
/// `name` stand; a missing `id` is filled in); the app fields go over it.
///
/// Order matters for the filesystem watcher, which finalizes an employee
/// the moment it sees `AGENT.md`: `ui/` first, then `agent.json` and
/// `manifest.json`, `AGENT.md` last. A scan between two writes once logged
/// "agent.json: EOF while parsing" and kept a broken employee (2026-09-05).
pub fn write_user_agent(dir: &Path, pkg: &AgentPackage<'_>) -> Result<(), NappError> {
    // Never write what the loader cannot read back: the employee's duties
    // would run invisibly.
    parse_agent(pkg.agent_md)
        .map_err(|e| NappError::Other(format!("AGENT.md is invalid and was not saved: {e}")))?;
    let manifest = compose_manifest(dir, pkg);
    let manifest_text = serde_json::to_string_pretty(&Value::Object(manifest))?;

    std::fs::create_dir_all(dir)
        .map_err(|e| NappError::Other(format!("Failed to create {}: {e}", dir.display())))?;
    let ui_dir = dir.join("ui");
    for (rel, content) in &pkg.ui {
        let target = ui_dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| NappError::Other(format!("Failed to create ui/{}: {e}", rel.display())))?;
        }
        std::fs::write(&target, content)
            .map_err(|e| NappError::Other(format!("Failed to write ui/{}: {e}", rel.display())))?;
    }
    let mut plan: Vec<(&str, &str)> = Vec::new();
    if let Some(json) = pkg.agent_json {
        plan.push(("agent.json", json));
    }
    plan.push(("manifest.json", &manifest_text));
    plan.push(("AGENT.md", pkg.agent_md));
    for (file, content) in plan {
        std::fs::write(dir.join(file), content)
            .map_err(|e| NappError::Other(format!("Failed to write {file}: {e}")))?;
    }
    Ok(())
}

/// The manifest as it will be written: what is on disk, or a fresh one from
/// the package, with the app fields laid over it.
fn compose_manifest(dir: &Path, pkg: &AgentPackage<'_>) -> Map<String, Value> {
    let mut manifest = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for (key, value) in [
        ("id", Value::from(pkg.id)),
        ("name", Value::from(pkg.name)),
        ("version", Value::from("1.0.0")),
        ("type", Value::from("agent")),
        ("description", Value::from(pkg.description)),
    ] {
        manifest.entry(key).or_insert(value);
    }
    let app = match &pkg.app {
        Some(app) => Some(app.clone()),
        None if !pkg.ui.is_empty() && !manifest_is_app(&manifest) => Some(AppFields::default()),
        None => None,
    };
    if let Some(app) = app {
        manifest.insert("type".into(), Value::from("app"));
        manifest.insert("artifact_type".into(), Value::from("app"));
        if let Some(window) = app.window {
            manifest.insert("window".into(), serde_json::to_value(window).unwrap_or_default());
        }
        if let Some(permissions) = app.permissions {
            manifest.insert("permissions".into(), Value::from(permissions));
        }
        let has_permissions = manifest
            .get("permissions")
            .and_then(|p| p.as_array())
            .is_some_and(|a| !a.is_empty());
        if !has_permissions {
            manifest.insert("permissions".into(), Value::from(APP_DEFAULT_PERMISSIONS));
        }
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AGENT_MD: &str = "---\nname: deals\ndescription: Deals\n---\nYou track deals.";

    fn package<'a>(app: Option<AppFields>, ui: Vec<(PathBuf, Vec<u8>)>) -> AgentPackage<'a> {
        AgentPackage {
            id: "11111111-2222-3333-4444-555555555555",
            name: "Deals",
            description: "Deals",
            agent_md: AGENT_MD,
            agent_json: Some("{\"workflows\":{}}"),
            app,
            ui,
        }
    }

    fn manifest_in(dir: &Path) -> Map<String, Value> {
        serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join("manifest.json")).unwrap())
            .unwrap()
            .as_object()
            .cloned()
            .unwrap()
    }

    /// A plain employee: the three files, an `agent` manifest carrying the
    /// row's id, no `ui/`.
    #[test]
    fn a_plain_employee_is_three_files_and_no_ui() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("deals");
        write_user_agent(&dir, &package(None, Vec::new())).unwrap();
        for file in ["agent.json", "manifest.json", "AGENT.md"] {
            assert!(dir.join(file).is_file(), "{file}");
        }
        let m = manifest_in(&dir);
        assert_eq!(m["id"], "11111111-2222-3333-4444-555555555555");
        assert_eq!(m["name"], "Deals");
        assert_eq!(m["type"], "agent");
        assert!(!manifest_is_app(&m));
        assert!(m.get("permissions").is_none());
        assert!(!dir.join("ui").exists());
        assert_eq!(std::fs::read_to_string(dir.join("AGENT.md")).unwrap(), AGENT_MD);
    }

    /// An `app` block marks the manifest with both spellings the loader
    /// reads, carries the window through as the loader deserializes it, and
    /// defaults the permissions to the app's own storage.
    #[test]
    fn an_app_block_marks_the_manifest_the_way_the_loader_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().join("deals");
        let app = AppFields::from_json(&json!({
            "window": {"title": "Deals", "width": 900, "height": 700, "resizable": false}
        }))
        .unwrap();
        let ui = vec![
            (PathBuf::from("index.html"), b"<p>hi</p>".to_vec()),
            (PathBuf::from("assets/app.js"), b"1".to_vec()),
        ];
        write_user_agent(&dir, &package(Some(app), ui)).unwrap();
        let m = manifest_in(&dir);
        assert_eq!(m["artifact_type"], "app");
        assert_eq!(m["type"], "app");
        assert_eq!(m["window"]["title"], "Deals");
        assert_eq!(m["window"]["resizable"], false);
        assert_eq!(m["permissions"], json!(["storage:readwrite"]));
        assert!(manifest_is_app(&m));
        let window: AppWindowConfig = serde_json::from_value(m["window"].clone()).unwrap();
        assert_eq!((window.width, window.height, window.resizable), (900, 700, false));
        assert_eq!(std::fs::read_to_string(dir.join("ui/index.html")).unwrap(), "<p>hi</p>");
        assert_eq!(std::fs::read_to_string(dir.join("ui/assets/app.js")).unwrap(), "1");

        // Named permissions are kept, not replaced by the default.
        let app = AppFields::from_json(&json!({"permissions": ["storage:read", "network:outbound"]})).unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        write_user_agent(dir2.path(), &package(Some(app), Vec::new())).unwrap();
        let m = manifest_in(dir2.path());
        assert_eq!(m["permissions"], json!(["storage:read", "network:outbound"]));
        assert!(m.get("window").is_none());
    }

    /// A page without an `app` block still makes an app.
    #[test]
    fn ui_files_without_an_app_block_still_make_an_app() {
        let dir = tempfile::tempdir().unwrap();
        let ui = vec![(PathBuf::from("index.html"), b"<p>hi</p>".to_vec())];
        write_user_agent(dir.path(), &package(None, ui)).unwrap();
        let m = manifest_in(dir.path());
        assert!(manifest_is_app(&m));
        assert_eq!(m["permissions"], json!(["storage:readwrite"]));
    }

    /// Writing over an existing directory keeps its manifest: id and name
    /// stand, an `app: {}` keeps the permissions already on disk, and new
    /// fields layer over the rest.
    #[test]
    fn an_existing_manifest_is_kept_and_layered_on() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"id":"existing-id","name":"Kept Name","version":"2.3.0","type":"app","permissions":["network:outbound"],"code":"AGNT-AAAA-BBBB"}"#,
        )
        .unwrap();
        write_user_agent(dir.path(), &package(Some(AppFields::default()), Vec::new())).unwrap();
        let m = manifest_in(dir.path());
        assert_eq!(m["id"], "existing-id");
        assert_eq!(m["name"], "Kept Name");
        assert_eq!(m["version"], "2.3.0");
        assert_eq!(m["code"], "AGNT-AAAA-BBBB");
        assert_eq!(m["permissions"], json!(["network:outbound"]));
        assert_eq!(m["artifact_type"], "app");

        // A manifest with no id gets the row's.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), r#"{"name":"Old","version":"1.0.0"}"#).unwrap();
        write_user_agent(dir.path(), &package(None, Vec::new())).unwrap();
        let m = manifest_in(dir.path());
        assert_eq!(m["id"], "11111111-2222-3333-4444-555555555555");
        assert_eq!(m["name"], "Old");
    }

    /// What a duplicate reads from the source: the app fields as the loader
    /// would, and every page file with its bytes.
    #[test]
    fn a_package_reads_back_from_disk_for_duplication() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppFields::from_json(&json!({
            "window": {"title": "Deals", "width": 640, "height": 480},
            "permissions": ["network:outbound"]
        }))
        .unwrap();
        // Sorted by path, which is how they read back.
        let ui = vec![
            (PathBuf::from("img/logo.png"), vec![0x89, 0x50, 0x4e, 0x47, 0x00, 0xff]),
            (PathBuf::from("index.html"), b"<p>hi</p>".to_vec()),
        ];
        write_user_agent(dir.path(), &package(Some(app), ui.clone())).unwrap();

        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("manifest.json")).unwrap()).unwrap();
        let copied = AppFields::from_manifest(&manifest).expect("an app");
        let window = copied.window.as_ref().expect("window");
        assert_eq!((window.width, window.height, window.title.as_deref()), (640, 480, Some("Deals")));
        assert_eq!(copied.permissions, Some(vec!["network:outbound".to_string()]));
        assert_eq!(read_ui_files(dir.path()).unwrap(), ui);

        // Written elsewhere, the copy is the same app.
        let copy = tempfile::tempdir().unwrap();
        let pkg = AgentPackage { id: "copy-id", name: "Deals (Copy)", ..package(Some(copied), read_ui_files(dir.path()).unwrap()) };
        write_user_agent(copy.path(), &pkg).unwrap();
        let m = manifest_in(copy.path());
        assert_eq!(m["id"], "copy-id");
        assert_eq!(m["name"], "Deals (Copy)");
        assert_eq!(m["window"]["width"], 640);
        assert_eq!(m["permissions"], json!(["network:outbound"]));
        assert_eq!(std::fs::read(copy.path().join("ui/img/logo.png")).unwrap(), ui[0].1);

        // A plain employee's manifest reads as no app, and no ui/ is no files.
        assert!(AppFields::from_manifest(&json!({"type": "agent"})).is_none());
        let plain = tempfile::tempdir().unwrap();
        assert!(read_ui_files(plain.path()).unwrap().is_empty());
    }

    /// Bad app fields are refused with the loader's own words, before
    /// anything is written.
    #[test]
    fn bad_app_fields_are_refused_before_a_write() {
        let err = AppFields::from_json(&json!({"permissions": ["bogus:thing"]})).unwrap_err().to_string();
        assert!(err.contains("unknown permission: bogus:thing"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        let err = AppFields::from_json(&json!({"window": {"width": "wide"}})).unwrap_err().to_string();
        assert!(err.contains("`app.window` is invalid"), "{err}");
        let err = AppFields::from_json(&json!({"window": 5})).unwrap_err().to_string();
        assert!(err.contains("`app.window` must be an object"), "{err}");
        let err = AppFields::from_json(&json!("app")).unwrap_err().to_string();
        assert!(err.contains("`app` must be an object"), "{err}");
        let err = AppFields::from_json(&json!({"permissions": "storage:readwrite"})).unwrap_err().to_string();
        assert!(err.contains("`app.permissions` must be an array"), "{err}");
        let err = AppFields::from_json(&json!({"permissions": [1]})).unwrap_err().to_string();
        assert!(err.contains("must be a string"), "{err}");

        // An AGENT.md the loader cannot parse never lands.
        let dir = tempfile::tempdir().unwrap();
        let pkg = AgentPackage { agent_md: "---\nname: [broken\n---\nHi", ..package(None, Vec::new()) };
        let err = write_user_agent(dir.path(), &pkg).unwrap_err().to_string();
        assert!(err.contains("AGENT.md is invalid and was not saved"), "{err}");
        assert!(!dir.path().join("AGENT.md").exists());
        assert!(!dir.path().join("manifest.json").exists());
    }

    /// A `ui` path stays inside the app's ui/ directory: `..`, a root, a
    /// drive, or a backslash is refused before anything is written.
    #[test]
    fn ui_paths_stay_inside_the_ui_directory() {
        for ok in ["index.html", "assets/app.js", "./app.js", "css/theme/dark.css"] {
            let p = ui_relative_path(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
            assert!(p.is_relative(), "{ok}");
        }
        for (bad, why) in [
            ("../x", "`..`"),
            ("a/../../x", "`..`"),
            ("/etc/x", "absolute"),
            ("C:\\x", "backslash"),
            ("..\\x", "backslash"),
            ("", "empty"),
            ("./", "names no file"),
        ] {
            let err = ui_relative_path(bad).unwrap_err().to_string();
            assert!(err.contains(why), "{bad}: {err}");
            assert!(err.contains("nothing was written"), "{bad}: {err}");
        }
    }

    /// AGENT.md is written last: a watcher scan that lands between writes
    /// sees ui/, agent.json and manifest.json whole, never an employee
    /// without them. A directory in AGENT.md's place makes its write fail;
    /// everything that must precede it is already there.
    #[test]
    fn agent_md_is_written_last() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("AGENT.md")).unwrap();
        let ui = vec![(PathBuf::from("index.html"), b"<p>hi</p>".to_vec())];
        let err = write_user_agent(dir.path(), &package(None, ui)).unwrap_err().to_string();
        assert!(err.contains("Failed to write AGENT.md"), "{err}");
        assert!(dir.path().join("agent.json").is_file());
        assert!(dir.path().join("manifest.json").is_file());
        assert!(dir.path().join("ui/index.html").is_file());
    }
}
