//! Installing an org on one Nebo (implementation PRD R12): employees into the
//! user agents directory, packs into `packs/`, teams into the local teams
//! table, then every seat reads the packs (R15). Idempotent: installing twice
//! leaves one of everything.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use tracing::info;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOrgRequest {
    /// Absolute path of an org folder: `employees/`, `teams/`, `industry/`, `company/`.
    pub path: String,
}

/// POST /org/install
pub async fn install_org(
    State(state): State<AppState>,
    Json(body): Json<InstallOrgRequest>,
) -> HandlerResult<serde_json::Value> {
    let root = PathBuf::from(body.path.trim());
    if !root.join("employees").is_dir() {
        return Err(to_error_response(types::NeboError::Validation(format!(
            "{} has no employees/ folder",
            root.display()
        ))));
    }

    // 1. Employees → the user agents directory. The directory watcher creates
    //    the rows; this handler only puts the files where the loader looks.
    let user_dir = state.agent_loader.user_dir().to_path_buf();
    let mut employees_copied = Vec::new();
    let mut names_by_qualified: std::collections::HashMap<String, String> = Default::default();
    for entry in std::fs::read_dir(root.join("employees")).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let src = entry.path();
        if !src.is_dir() || !src.join("AGENT.md").is_file() {
            continue;
        }
        let slug = entry.file_name().to_string_lossy().to_string();
        if let Ok(manifest) = std::fs::read_to_string(src.join("manifest.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&manifest) {
                if let (Some(q), Some(n)) = (
                    v.get("qualifiedName").and_then(|x| x.as_str()),
                    v.get("name").and_then(|x| x.as_str()),
                ) {
                    names_by_qualified.insert(q.to_string(), n.to_string());
                }
            }
        }
        copy_dir(&src, &user_dir.join(&slug)).map_err(io_err)?;
        employees_copied.push(slug);
    }

    // 2. Packs → packs/. One directory per layer; the marker names the layer.
    let packs_dir = config::packs_dir().map_err(to_error_response)?;
    let mut packs_copied = Vec::new();
    for layer_dir in ["industry", "franchise", "company"] {
        let src = root.join(layer_dir);
        // A layer folder is a pack only when it carries its marker.
        let has_marker = ["INDUSTRY.md", "FRANCHISE.md", "COMPANY.md"]
            .iter()
            .any(|m| src.join(m).is_file());
        if !src.is_dir() || !has_marker {
            continue;
        }
        let slug = pack_slug(&src).unwrap_or_else(|| layer_dir.to_string());
        copy_dir(&src, &packs_dir.join(&slug)).map_err(io_err)?;
        packs_copied.push(slug);
    }

    // 3. Wait for the watcher to have created the employees' rows (coalesced
    //    debounce, then a rescan), so teams can name their members.
    let expected: Vec<String> = names_by_qualified.values().cloned().collect();
    for _ in 0..40 {
        let missing = expected
            .iter()
            .filter(|n| state.store.get_agent_by_name(n).ok().flatten().is_none())
            .count();
        if missing == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // 4. Teams, by name, idempotent: create or bring members up to date.
    let mut teams_written = Vec::new();
    let mut teams_skipped = Vec::new();
    if root.join("teams").is_dir() {
        for entry in std::fs::read_dir(root.join("teams")).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let team_md = entry.path().join("TEAM.md");
            if !team_md.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&team_md).map_err(io_err)?;
            let (fm, body) = frontmatter(&text);
            let name = fm.get("name").cloned().unwrap_or_else(|| {
                entry.file_name().to_string_lossy().to_string()
            });
            let mission = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .next()
                .unwrap_or("")
                .to_string();
            let member_qualified: Vec<String> = fm
                .get("members")
                .and_then(|m| serde_json::from_str::<Vec<String>>(m).ok())
                .unwrap_or_default();
            let mut member_ids = Vec::new();
            let mut unresolved = Vec::new();
            for q in &member_qualified {
                let label = names_by_qualified
                    .get(q)
                    .cloned()
                    .unwrap_or_else(|| q.rsplit('/').next().unwrap_or(q).to_string());
                match tools::team::resolve_agent(&state.store, &label) {
                    Some(a) => {
                        if !member_ids.contains(&a.id) {
                            member_ids.push(a.id);
                        }
                    }
                    None => unresolved.push(q.clone()),
                }
            }
            let organizer_id = fm
                .get("organizer")
                .map(|q| q.trim_matches('"').to_string())
                .and_then(|q| {
                    let label = names_by_qualified
                        .get(&q)
                        .cloned()
                        .unwrap_or_else(|| q.rsplit('/').next().unwrap_or(&q).to_string());
                    tools::team::resolve_agent(&state.store, &label).map(|a| a.id)
                })
                .unwrap_or_default();
            if member_ids.len() < 2 {
                teams_skipped.push(serde_json::json!({ "team": name, "reason": "fewer than two resolvable members", "unresolved": unresolved }));
                continue;
            }
            let result = match state.store.get_team_by_name(&name).ok().flatten() {
                Some(existing) => tools::team::update(
                    &state.store,
                    &existing.id,
                    None,
                    Some(&mission),
                    Some(&member_ids),
                    Some(&organizer_id),
                )
                .map(|t| t.id),
                None => tools::team::create(
                    None,
                    &state.store,
                    &name,
                    &mission,
                    &member_ids,
                    &organizer_id,
                )
                .await
                .map(|t| t.id),
            };
            match result {
                Ok(id) => teams_written.push(serde_json::json!({ "team": name, "id": id, "members": member_ids.len(), "unresolved": unresolved })),
                Err(e) => teams_skipped.push(serde_json::json!({ "team": name, "reason": e })),
            }
        }
    }

    // 5. Every seat reads the packs. The watcher on packs/ raises the change
    //    for later edits; the install raises it now so nobody waits on a
    //    debounce. `diff_and_raise` also reads the company layer's own policy
    //    — its purpose, its unattended bounds, the operations it reserves to
    //    the owner — because there is no artifact above the company layer to
    //    read it from.
    let current = napp::scan_packs(&packs_dir);
    {
        let mut previous = state.packs.write().await;
        crate::layers_update::diff_and_raise(&state, &mut previous, current);
    }

    info!(employees = employees_copied.len(), packs = packs_copied.len(), teams = teams_written.len(), "org installed");
    Ok(Json(serde_json::json!({
        "employees": employees_copied,
        "packs": packs_copied,
        "teams": teams_written,
        "teamsSkipped": teams_skipped,
    })))
}

fn io_err(e: std::io::Error) -> (reqwest::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(types::NeboError::Internal(e.to_string()))
}

/// The pack's slug: the marker's frontmatter `industry:`/`franchise:`/`company:`
/// value or the marker's own `slug:`, else the directory name.
fn pack_slug(dir: &Path) -> Option<String> {
    for marker in ["INDUSTRY.md", "FRANCHISE.md", "COMPANY.md"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(marker)) {
            let (fm, _) = frontmatter(&text);
            for key in ["slug", "industry", "franchise", "company"] {
                if let Some(v) = fm.get(key) {
                    let v = v.trim_matches('"').trim_start_matches('@');
                    let v = v.rsplit('/').next().unwrap_or(v);
                    if !v.is_empty() && !v.contains(' ') {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Hand-written frontmatter: `key: value` lines between `---` fences. Values
/// are kept raw (arrays as their JSON text) because org files carry bare
/// `@org/...` values that strict YAML rejects.
fn frontmatter(text: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut map = std::collections::HashMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (map, text.to_string());
    }
    let mut body_start = 0;
    for (i, line) in text.lines().enumerate().skip(1) {
        if line.trim() == "---" {
            body_start = i + 1;
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let body = text.lines().skip(body_start).collect::<Vec<_>>().join("\n");
    (map, body)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            let same = std::fs::read(&from)
                .ok()
                .zip(std::fs::read(&to).ok())
                .is_some_and(|(a, b)| a == b);
            if !same {
                std::fs::copy(&from, &to)?;
            }
        }
    }
    Ok(())
}
