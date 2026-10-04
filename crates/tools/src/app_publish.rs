//! Publishing an app from a conversation.
//!
//! The owner taps Publish on an app's chat; the app's employee drafts the
//! marketplace listing from the app's manifest and code, takes screenshots
//! of the app as it is served (`app_screenshot`), and shows the listing.
//! The owner shapes it by talking ("shorter description", "use the second
//! screenshot first"). Nothing reaches the marketplace until the owner says
//! yes on the card the submission puts in front of him; then the listing
//! goes through the NeboLoop MCP, the one publish path: create or update
//! the artifact, upload the app's bundle (manifest, persona, config and the
//! page under `ui/`), set the long description and the screenshots, and
//! submit the version for review. The review's outcome comes back as the
//! hub's `artifact_reviewed` notification and is said in the chat the
//! listing was submitted from.
//!
//! The tools (`app_screenshot`, `app_listing`, `app_submit`) belong to the
//! developer pack (`app_dev::TOOLS`): the owner's own app always publishes
//! itself, and App Developer mode opens them to an app's teammates
//! (`app_dev::withheld`, `app_dev::may_work_on`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const APP_SCREENSHOT: &str = "app_screenshot";
pub const APP_LISTING: &str = "app_listing";
pub const APP_SUBMIT: &str = "app_submit";

/// Where an owner without a publisher profile sets one up.
pub const PUBLISHER_SETUP_URL: &str = "https://neboai.com/publisher/register";

/// The owner's answer on the submission card that sends it.
pub const SUBMIT_ANSWER: &str = "Submit for review";
const NOT_YET_ANSWER: &str = "Not yet";

/// The most screenshots a listing holds (the hub's cap).
const MAX_SCREENSHOTS: usize = 10;
/// The hub's limits on one bundle file and on the whole bundle.
const MAX_BUNDLE_FILE: u64 = 10 << 20;
const MAX_BUNDLE_TOTAL: u64 = 50 << 20;

fn is_app(agent: &db::models::Agent) -> bool {
    agent.is_app.unwrap_or(0) != 0
}

/// The app a pack tool works on ([`resolve_app`]), when the caller may
/// work on it (`app_dev::may_work_on`: itself, when it is one of the
/// owner's own apps, or any app under App Developer mode).
pub(crate) fn permitted_app(
    store: &db::Store,
    ctx: &ToolContext,
    named: Option<&str>,
) -> Result<db::models::Agent, String> {
    let app = resolve_app(store, ctx, named)?;
    let caller = types::keyparser::extract_agent_id(&ctx.session_key);
    if crate::app_dev::may_work_on(store, &caller, &app) {
        Ok(app)
    } else {
        Err(crate::app_dev::not_yours(&app))
    }
}

/// The app a call is about: the one it names (by name or id), else the
/// employee running it when that employee is an app.
fn resolve_app(
    store: &db::Store,
    ctx: &ToolContext,
    named: Option<&str>,
) -> Result<db::models::Agent, String> {
    let named = named.map(str::trim).filter(|n| !n.is_empty());
    let found = match named {
        Some(n) => store
            .get_agent(n)
            .ok()
            .flatten()
            .or_else(|| store.get_agent_by_name(n).ok().flatten()),
        None => {
            let me = types::keyparser::extract_agent_id(&ctx.session_key);
            store.get_agent(&me).ok().flatten()
        }
    };
    match found {
        Some(a) if is_app(&a) => Ok(a),
        Some(a) => Err(format!(
            "{} is not an app. Name the app (app: \"<name>\").",
            a.name
        )),
        None => Err(match named {
            Some(n) => format!("There is no app named {n} on this bot."),
            None => "Name the app (app: \"<name>\").".to_string(),
        }),
    }
}

/// The folder the app is served from (`agents.app_ui_path`), else its
/// package's `ui/`.
fn ui_dir(app: &db::models::Agent) -> Option<PathBuf> {
    app.app_ui_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| app.napp_path.as_deref().map(|p| Path::new(p).join("ui")))
        .filter(|p| p.is_dir())
}

/// The app's package folder (manifest.json, AGENT.md, agent.json).
fn package_dir(app: &db::models::Agent) -> Option<PathBuf> {
    app.napp_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// The bot's own HTTP port, by the one rule every local caller uses
/// (`napp::plugin::local_port`).
fn local_port() -> u16 {
    napp::plugin::local_port()
}

// ── The listing ─────────────────────────────────────────────────────

/// One screenshot of the listing: the file the one upload path stored,
/// and the copy on this bot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingShot {
    pub file_id: String,
    /// The copy in the workspace, relative to the files folder.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub label: String,
}

/// An app's marketplace listing as the owner shapes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListingDraft {
    pub name: String,
    pub short_description: String,
    pub long_description: String,
    #[serde(default)]
    pub category: Option<String>,
    pub version: String,
    /// public, unlisted or private (the hub's listing rules).
    pub visibility: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub window: Option<Value>,
    #[serde(default)]
    pub screenshots: Vec<ListingShot>,
}

/// What a draft is built from: the app as it is on disk, and what is known
/// of its listing so far.
#[derive(Debug)]
pub struct DraftSource<'a> {
    /// The employee's display name.
    pub display_name: &'a str,
    /// The employee's one-line description.
    pub description: &'a str,
    /// manifest.json.
    pub manifest: &'a Value,
    /// AGENT.md.
    pub agent_md: &'a str,
    /// The page's entry HTML, when there is one.
    pub index_html: Option<&'a str>,
    /// The listing as last shaped, when there is one.
    pub previous: Option<&'a ListingDraft>,
    /// The version last submitted, when the app was published before.
    pub published_version: Option<&'a str>,
    /// The marketplace's categories, when known.
    pub categories: &'a [String],
}

/// Draft the listing: what the owner shaped before stays; everything else
/// comes from the app itself. A version already submitted moves on a patch.
pub fn build_draft(src: &DraftSource) -> ListingDraft {
    let prev = src.previous;
    let (front, body) = split_frontmatter(src.agent_md);
    let manifest_str = |k: &str| {
        src.manifest
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let window = src
        .manifest
        .get("window")
        .filter(|w| w.is_object())
        .cloned();
    let window_title = window
        .as_ref()
        .and_then(|w| w.get("title"))
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let html_title = src.index_html.and_then(html_title);
    let html_description = src.index_html.and_then(html_meta_description);

    let name = prev
        .map(|p| p.name.clone())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| window_title.map(str::to_string))
        .or_else(|| Some(src.display_name.trim().to_string()).filter(|n| !n.is_empty()))
        .or_else(|| html_title.clone())
        .unwrap_or_else(|| "My App".to_string());

    let short_description = prev
        .map(|p| p.short_description.clone())
        .filter(|d| !d.trim().is_empty())
        .or_else(|| manifest_str("description").map(str::to_string))
        .or_else(|| frontmatter_field(front, "description"))
        .or_else(|| Some(src.description.trim().to_string()).filter(|d| !d.is_empty()))
        .or_else(|| html_description.clone())
        .map(|d| cap_chars(&one_line(&d), 500))
        .unwrap_or_default();

    let long_description = prev
        .map(|p| p.long_description.clone())
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| {
            let prose = prose_of(body);
            if prose.is_empty() {
                short_description.clone()
            } else {
                cap_chars(&prose, 4000)
            }
        });

    let category = prev
        .and_then(|p| p.category.clone())
        .or_else(|| manifest_str("category").map(str::to_string))
        .or_else(|| {
            let text = format!(
                "{name} {short_description} {} {} {}",
                src.description,
                src.manifest
                    .get("tags")
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                long_description
            )
            .to_lowercase();
            // A category the app's own words name ("game" names Games).
            src.categories
                .iter()
                .find(|c| {
                    let c = c.to_lowercase();
                    let singular = c.strip_suffix('s').unwrap_or(&c);
                    text.contains(&c) || (singular.len() >= 3 && text.contains(singular))
                })
                .cloned()
        });

    let manifest_version = manifest_str("version")
        .filter(|v| is_semver(v))
        .unwrap_or("1.0.0")
        .to_string();
    let mut version = prev
        .map(|p| p.version.clone())
        .filter(|v| is_semver(v))
        .unwrap_or(manifest_version);
    if let Some(published) = src.published_version.filter(|p| is_semver(p)) {
        if !semver_gt(&version, published) {
            version = bump_patch(published);
        }
    }

    let permissions = src
        .manifest
        .get("permissions")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    ListingDraft {
        name,
        short_description,
        long_description,
        category,
        version,
        visibility: prev
            .map(|p| p.visibility.clone())
            .filter(|v| is_visibility(v))
            .unwrap_or_else(|| "public".into()),
        permissions,
        window,
        screenshots: prev.map(|p| p.screenshots.clone()).unwrap_or_default(),
    }
}

/// Apply the owner's changes, as the employee passes them on. `screenshots`
/// names the files to keep, in the order to show them.
pub fn apply_edits(draft: &mut ListingDraft, edits: &Value) -> Result<(), String> {
    let text = |k: &str| {
        edits
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(v) = text("name") {
        draft.name = v.to_string();
    }
    if let Some(v) = text("short_description") {
        draft.short_description = one_line(v);
    }
    if let Some(v) = text("long_description") {
        draft.long_description = v.to_string();
    }
    if let Some(v) = text("category") {
        draft.category = Some(v.to_string());
    }
    if let Some(v) = text("version") {
        if !is_semver(v) {
            return Err(format!("{v} is not a version like 1.2.0."));
        }
        draft.version = v.to_string();
    }
    if let Some(v) = text("visibility") {
        let v = v.to_lowercase();
        if !is_visibility(&v) {
            return Err("Visibility is public, unlisted or private.".to_string());
        }
        draft.visibility = v;
    }
    if let Some(order) = edits.get("screenshots").and_then(|v| v.as_array()) {
        let mut kept = Vec::new();
        for id in order.iter().filter_map(|v| v.as_str()) {
            match draft.screenshots.iter().find(|s| s.file_id == id) {
                Some(shot) => kept.push(shot.clone()),
                None => return Err(format!("{id} is not one of this listing's screenshots.")),
            }
        }
        draft.screenshots = kept;
    }
    Ok(())
}

/// What keeps the listing from being submitted, in words the employee can
/// act on. Empty when it is ready.
pub fn problems(draft: &ListingDraft, categories: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if draft.name.trim().is_empty() {
        out.push("The listing needs a name.".to_string());
    }
    let short = draft.short_description.chars().count();
    if short < 10 {
        out.push("The short description needs at least 10 characters.".to_string());
    } else if short > 500 {
        out.push("The short description is over 500 characters.".to_string());
    }
    if !is_semver(&draft.version) {
        out.push(format!("{} is not a version like 1.2.0.", draft.version));
    }
    if !is_visibility(&draft.visibility) {
        out.push("Visibility is public, unlisted or private.".to_string());
    }
    if draft.screenshots.is_empty() {
        out.push("The listing has no screenshots yet: take 3 to 5 with app_screenshot (for_listing: true).".to_string());
    } else if draft.screenshots.len() > MAX_SCREENSHOTS {
        out.push(format!(
            "A listing shows at most {MAX_SCREENSHOTS} screenshots."
        ));
    }
    if draft.category.is_none() && !categories.is_empty() {
        out.push(format!("Pick a category: {}.", categories.join(", ")));
    }
    out
}

/// The listing as the owner reads it in the chat.
pub fn render_listing(draft: &ListingDraft, status: &str) -> String {
    let mut out = format!(
        "**{}** · v{} · {}\n",
        draft.name,
        draft.version,
        visibility_label(&draft.visibility)
    );
    if let Some(c) = &draft.category {
        out.push_str(&format!("Category: {c}\n"));
    }
    if !status.is_empty() && status != "draft" {
        out.push_str(&format!("Status: {}\n", status_label(status)));
    }
    out.push_str(&format!("\n> {}\n\n", draft.short_description));
    out.push_str(&format!("**What it does**\n{}\n\n", draft.long_description));
    if !draft.permissions.is_empty() {
        out.push_str(&format!("Permissions: {}\n", draft.permissions.join(", ")));
    }
    if draft.screenshots.is_empty() {
        out.push_str("Screenshots: none yet\n");
    } else {
        out.push_str("Screenshots (the first is the card image):\n");
        for (i, s) in draft.screenshots.iter().enumerate() {
            let label = if s.label.is_empty() {
                s.path.as_str()
            } else {
                s.label.as_str()
            };
            out.push_str(&format!("{}. {} (file {})\n", i + 1, label, s.file_id));
        }
    }
    out
}

/// The structured card the app renders beside the text.
pub fn listing_payload(
    app_id: &str,
    artifact_id: &str,
    draft: &ListingDraft,
    status: &str,
    notes: &str,
) -> Value {
    json!({
        "kind": "app_listing",
        "appId": app_id,
        "artifactId": artifact_id,
        "name": draft.name,
        "shortDescription": draft.short_description,
        "version": draft.version,
        "visibility": draft.visibility,
        "category": draft.category,
        "status": status,
        "notes": notes,
        "screenshots": draft.screenshots.iter().map(|s| json!({"fileId": s.file_id, "path": s.path, "label": s.label})).collect::<Vec<_>>(),
    })
}

pub fn visibility_label(v: &str) -> &'static str {
    match v {
        "unlisted" => "Unlisted (installs by link or code)",
        "private" => "Private (only you)",
        _ => "Public",
    }
}

pub fn status_label(s: &str) -> &'static str {
    match s {
        "approved" => "Approved",
        "rejected" => "Changes requested",
        "in_review" | "submitted" => "In review",
        _ => "Draft",
    }
}

/// The hub's submission status as the listing keeps it.
fn listing_status(hub_status: &str) -> &'static str {
    match hub_status {
        "approved" | "auto_approved" | "active" => "approved",
        "rejected" | "draft" => "rejected",
        _ => "in_review",
    }
}

fn is_visibility(v: &str) -> bool {
    matches!(v, "public" | "unlisted" | "private")
}

fn is_semver(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn semver_parts(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn semver_gt(a: &str, b: &str) -> bool {
    semver_parts(a) > semver_parts(b)
}

fn bump_patch(v: &str) -> String {
    let (a, b, c) = semver_parts(v);
    format!("{a}.{b}.{}", c + 1)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, cut at a word when that keeps most of it.
fn cap_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let body = match cut.rfind(char::is_whitespace) {
        Some(i) if i >= cut.len() / 2 => &cut[..i],
        _ => cut.as_str(),
    };
    format!("{}…", body.trim_end())
}

/// AGENT.md's frontmatter block and its body.
fn split_frontmatter(md: &str) -> (&str, &str) {
    let t = md.trim_start_matches('\u{feff}');
    if let Some(rest) = t
        .strip_prefix("---\n")
        .or_else(|| t.strip_prefix("---\r\n"))
    {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            let after = after
                .strip_prefix('\n')
                .or_else(|| after.strip_prefix("\r\n"))
                .unwrap_or(after);
            return (&rest[..end], after);
        }
    }
    ("", t)
}

fn frontmatter_field(front: &str, key: &str) -> Option<String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(front).ok()?;
    doc.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The persona's prose: headings and lists' markers dropped, paragraphs
/// kept.
fn prose_of(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .split("\n\n")
        .map(one_line)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title>")? + 7;
    let end = lower[start..].find("</title>")? + start;
    Some(one_line(&html[start..end])).filter(|t| !t.is_empty())
}

fn html_meta_description(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let at = lower.find("name=\"description\"")?;
    let tag_start = lower[..at].rfind('<')?;
    let tag_end = lower[at..].find('>')? + at;
    let tag = &html[tag_start..tag_end];
    let c = tag.to_lowercase().find("content=\"")? + 9;
    let rest = &tag[c..];
    Some(one_line(&rest[..rest.find('"')?])).filter(|d| !d.is_empty())
}

/// AGENT.md with `artifact_type: app` in its frontmatter: the hub types an
/// artifact from it when the artifact is created.
pub fn with_app_frontmatter(agent_md: &str) -> String {
    let (front, body) = split_frontmatter(agent_md);
    let declares = front.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("artifact_type:")
            .is_some_and(|v| v.trim().trim_matches('"') == "app")
    });
    if declares {
        return agent_md.to_string();
    }
    if front.is_empty() && !agent_md.trim_start().starts_with("---") {
        return format!("---\nartifact_type: app\n---\n{agent_md}");
    }
    format!("---\n{}\nartifact_type: app\n---\n{body}", front.trim_end())
}

// ── The bundle ──────────────────────────────────────────────────────

/// The app's bundle for the hub: AGENT.md (typed as an app), agent.json,
/// manifest.json, the employee's own skills under `skills/` ([`own_skills`]),
/// and every file of the page under `ui/`, as a .zip.
/// Returns the bytes and how many page files it carries. Dot files and
/// folders never ship; a file or a bundle past the hub's limits is refused
/// here, in words, rather than cut there.
///
/// `user_skills` is the bot's `user/skills/` folder, where a skill the
/// employee's agent.json names by plain name lives when it is not in the
/// package.
pub fn build_bundle(
    agent_md: &str,
    package: Option<&Path>,
    ui: &Path,
    user_skills: Option<&Path>,
) -> Result<(Vec<u8>, usize), String> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut total: u64 = 0;
    let mut add = |zip: &mut zip::ZipWriter<&mut std::io::Cursor<Vec<u8>>>,
                   name: &str,
                   data: &[u8]|
     -> Result<(), String> {
        let len = data.len() as u64;
        if len > MAX_BUNDLE_FILE {
            return Err(format!(
                "{name} is {} MB; one file may be at most 10 MB.",
                len >> 20
            ));
        }
        total += len;
        if total > MAX_BUNDLE_TOTAL {
            return Err(
                "The app's files come to more than 50 MB, the most one listing may carry."
                    .to_string(),
            );
        }
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(data).map_err(|e| e.to_string())
    };
    add(
        &mut zip,
        "AGENT.md",
        with_app_frontmatter(agent_md).as_bytes(),
    )?;
    if let Some(pkg) = package {
        for name in ["agent.json", "manifest.json"] {
            if let Ok(data) = std::fs::read(pkg.join(name)) {
                add(&mut zip, name, &data)?;
            }
        }
    }
    for (name, path) in own_skills(package, user_skills) {
        let data = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        add(&mut zip, &name, &data)?;
    }
    let mut pages = 0usize;
    let mut pending = vec![ui.to_path_buf()];
    let mut files: Vec<PathBuf> = Vec::new();
    while let Some(dir) = pending.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if hidden || meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if entry.file_name() != "node_modules" {
                    pending.push(path);
                }
            } else if meta.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let rel = path.strip_prefix(ui).map_err(|e| e.to_string())?;
        let name = format!("ui/{}", rel.to_string_lossy().replace('\\', "/"));
        let data = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        add(&mut zip, &name, &data)?;
        pages += 1;
    }
    zip.finish().map_err(|e| e.to_string())?;
    if pages == 0 {
        return Err("The app's page folder is empty: there is nothing to publish yet.".to_string());
    }
    Ok((buf.into_inner(), pages))
}

/// Folders the hub drops from a bundle wherever they appear (neboloop
/// `skill_bundle.go`, `bundleSkipDirs`); every dot folder goes too.
const SKILL_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "out",
    "dist",
    "__pycache__",
    "venv",
];
/// Names the system owns, never a skill's own file (neboloop
/// `skill_files.go`, `reservedSkillFiles`); a skill's own SKILL.md is the
/// one exception.
const SKILL_RESERVED_FILES: &[&str] = &[
    "SKILL.md",
    "AGENT.md",
    "PLUGIN.md",
    "agent.json",
    "plugin.json",
    "manifest.json",
    "signatures.json",
];
/// The file types a skill may carry (neboloop `skill_files.go`,
/// `allowedSkillFileExts`); files under its `scripts/` or `bin/` may be
/// any type.
const SKILL_FILE_TYPES: &[&str] = &[
    "sh", "ps1", "py", "js", "ts", "md", "json", "yaml", "yml", "txt", "csv", "toml", "cfg", "html",
    "css", "sql", "rb", "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp", "pdf", "woff",
    "woff2", "ttf", "otf",
];

/// Whether the hub keeps a skill's file, by its path inside the skill's
/// folder (`SKILL.md`, `scripts/run`, `references/notes.md`).
fn skill_file_kept(rest: &str) -> bool {
    let parts: Vec<&str> = rest.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || p.starts_with('.') || SKILL_SKIP_DIRS.contains(p))
    {
        return false;
    }
    let base = parts[parts.len() - 1];
    if rest == "SKILL.md" {
        return true;
    }
    if base == "Thumbs.db" || SKILL_RESERVED_FILES.contains(&base) {
        return false;
    }
    if parts.len() > 1 && matches!(parts[0], "scripts" | "bin") {
        return true;
    }
    Path::new(base)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SKILL_FILE_TYPES.contains(&e.to_ascii_lowercase().as_str()))
}

/// A skill's name as a folder: one plain path segment.
fn plain_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('@')
        && !name.contains(['/', '\\'])
        && !name.contains("..")
}

/// The employee's own skills, as `(path in the bundle, file on disk)` under
/// `skills/<name>/`: every skill folder in its package's `skills/` (what an
/// employee package ships and the loader keys to that employee), then each
/// skill its agent.json names by plain name that lives in the bot's
/// `user/skills/` instead. A marketplace reference (`@org/skills/name`) is a
/// dependency the hub installs on its own and is never copied; what the
/// employee learned on this bot stays here. Files the hub would not keep
/// ([`skill_file_kept`]) and links are left out; sizes are checked by the
/// bundle like every other file.
pub fn own_skills(package: Option<&Path>, user_skills: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mut skills: Vec<(String, PathBuf)> = Vec::new();
    if let Some(dir) = package.map(|p| p.join("skills")) {
        let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let path = e.path();
                let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
                (is_dir && plain_skill_name(&name) && path.join("SKILL.md").is_file()).then_some((name, path))
            })
            .collect();
        found.sort();
        skills.extend(found);
    }
    let named: Vec<String> = package
        .and_then(|p| std::fs::read_to_string(p.join("agent.json")).ok())
        .and_then(|j| serde_json::from_str::<Value>(&j).ok())
        .and_then(|v| v.get("skills").and_then(|s| s.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().map(str::trim).map(String::from))
        .collect();
    if let Some(user) = user_skills {
        for name in named {
            if !plain_skill_name(&name) || skills.iter().any(|(n, _)| *n == name) {
                continue;
            }
            let dir = user.join(&name);
            if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) && dir.join("SKILL.md").is_file() {
                skills.push((name, dir));
            }
        }
    }
    let mut out = Vec::new();
    for (name, dir) in skills {
        let mut files: Vec<(String, PathBuf)> = walkdir::WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                let rest = e.path().strip_prefix(&dir).ok()?.to_string_lossy().replace('\\', "/");
                skill_file_kept(&rest).then(|| (format!("skills/{name}/{rest}"), e.path().to_path_buf()))
            })
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

// ── The hub ─────────────────────────────────────────────────────────

/// The publish path, as this module uses it: the hub's MCP tools and the
/// bundle upload the MCP's token opens. A trait so the confirmation and the
/// order of calls are tested without a hub.
#[async_trait::async_trait]
pub trait PublishHub: Send + Sync {
    /// The marketplace's category names.
    async fn categories(&self) -> Vec<String>;
    /// agent(action: create | update | bundle-token | submit | get).
    async fn agent(&self, args: Value) -> Result<Value, String>;
    /// Upload the bundle with a bundle-token's token.
    async fn upload_bundle(
        &self,
        artifact_id: &str,
        token: &str,
        zip: Vec<u8>,
    ) -> Result<Value, String>;
    /// Store a file through the one upload path; its file id.
    async fn upload_file(
        &self,
        filename: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> Result<String, String>;
}

/// The hub over this bot's NeboAI connection.
pub struct HubClient(pub comm::api::NeboAIApi);

#[async_trait::async_trait]
impl PublishHub for HubClient {
    async fn categories(&self) -> Vec<String> {
        let Ok(v) = self
            .0
            .mcp_tool("marketplace", json!({"action": "list_categories"}))
            .await
        else {
            return Vec::new();
        };
        let list = v.get("categories").cloned().unwrap_or(v);
        list.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        c.get("name")
                            .or_else(|| c.get("slug"))
                            .and_then(|n| n.as_str())
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn agent(&self, args: Value) -> Result<Value, String> {
        self.0
            .mcp_tool("agent", args)
            .await
            .map_err(|e| e.to_string())
    }

    async fn upload_bundle(
        &self,
        artifact_id: &str,
        token: &str,
        zip: Vec<u8>,
    ) -> Result<Value, String> {
        self.0
            .upload_bundle(artifact_id, token, zip)
            .await
            .map_err(|e| e.to_string())
    }

    async fn upload_file(
        &self,
        filename: &str,
        mime: &str,
        data: Vec<u8>,
    ) -> Result<String, String> {
        self.0
            .upload_file(filename, mime, data, &[])
            .await
            .map(|a| a.file_id)
            .map_err(|e| e.to_string())
    }
}

/// The hub's refusal, in words the owner can act on.
fn hub_words(err: &str) -> String {
    let lower = err.to_lowercase();
    if lower.contains("developer account") || lower.contains("publisher") {
        return format!(
            "The marketplace needs a publisher profile for this account first. Set one up at {PUBLISHER_SETUP_URL}, then \
             submit again. ({err})"
        );
    }
    if lower.contains("401") || lower.contains("unauthorized") || lower.contains("not paired") {
        return "This bot is not signed in to NeboAI, so it can't reach the marketplace. Connect it in Settings, NeboAI, \
                then submit again."
            .to_string();
    }
    format!("The marketplace refused it: {err}")
}

/// What a submission sent: the artifact, the hub's status for it, and the
/// hub's message.
#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    pub artifact_id: String,
    pub status: String,
    pub message: String,
}

/// The owner's yes, on a card in this conversation. Nothing else submits:
/// not the employee's own judgement, not an unattended run, not a reply
/// that only sounds like yes. The card's answer comes from the owner's tap.
pub async fn owner_says_submit(ctx: &ToolContext, draft: &ListingDraft) -> Result<(), String> {
    if crate::origin::ExecutionMode::from(ctx.origin) != crate::origin::ExecutionMode::Interactive
        || ctx.ask_channels.is_none()
        || ctx.stream_tx.is_none()
    {
        return Err(
            "Submitting needs the owner's yes in this conversation, and nobody is here to give it. Show the owner the \
             listing and submit when they are."
                .to_string(),
        );
    }
    let question = format!(
        "Submit {} v{} to the marketplace? It goes to review, then {}.\n\n{}",
        draft.name,
        draft.version,
        match draft.visibility.as_str() {
            "unlisted" => "anyone with its link or code can install it",
            "private" => "only you can install it",
            _ => "it is listed for everyone",
        },
        draft.short_description
    );
    let widgets = json!([{ "type": "options", "multiSelect": false, "options": [SUBMIT_ANSWER, NOT_YET_ANSWER] }]);
    match ctx.ask_user(&question, widgets).await {
        Some(answer) if answer.trim().eq_ignore_ascii_case(SUBMIT_ANSWER) => Ok(()),
        Some(_) => Err(
            "The owner said not yet. Nothing was sent. Keep shaping the listing with them."
                .to_string(),
        ),
        None => Err(
            "No app is connected to show the owner the submission. Nothing was sent.".to_string(),
        ),
    }
}

/// Send the listing through the publish path, after the owner's yes:
/// create (or update) the artifact, upload the bundle, set the long
/// description, the screenshots and the visibility, and submit the version.
pub async fn publish_to_hub(
    hub: &dyn PublishHub,
    existing_artifact: &str,
    draft: &ListingDraft,
    agent_md: &str,
    bundle: Vec<u8>,
) -> Result<Submitted, String> {
    let manifest = with_app_frontmatter(agent_md);
    let mut base = json!({
        "name": draft.name,
        "description": draft.short_description,
        "version": draft.version,
        "manifestContent": manifest,
    });
    if let Some(c) = &draft.category {
        base["category"] = json!(c);
    }
    let id = if existing_artifact.is_empty() {
        let mut args = base.clone();
        args["action"] = json!("create");
        // Create takes private, loop or public; unlisted is set on update.
        args["visibility"] = json!(if draft.visibility == "public" {
            "public"
        } else {
            "private"
        });
        let out = hub.agent(args).await.map_err(|e| hub_words(&e))?;
        out.get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                "The marketplace created the listing but did not say its id.".to_string()
            })?
    } else {
        let mut args = base.clone();
        args["action"] = json!("update");
        args["id"] = json!(existing_artifact);
        hub.agent(args).await.map_err(|e| hub_words(&e))?;
        existing_artifact.to_string()
    };

    let token = hub
        .agent(json!({"action": "bundle-token", "id": id}))
        .await
        .map_err(|e| hub_words(&e))?
        .get("token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "The marketplace did not hand out an upload token.".to_string())?;
    let stored = hub
        .upload_bundle(&id, &token, bundle)
        .await
        .map_err(|e| hub_words(&e))?;
    if stored
        .get("uiFilesStored")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        == 0
    {
        return Err(
            "The marketplace stored none of the app's page files, so nothing was submitted."
                .to_string(),
        );
    }

    hub.agent(json!({
        "action": "update",
        "id": id,
        "longDescription": draft.long_description,
        "visibility": draft.visibility,
        "screenshots": draft.screenshots.iter().map(|s| s.file_id.clone()).collect::<Vec<_>>(),
    }))
    .await
    .map_err(|e| hub_words(&e))?;

    let out = hub
        .agent(json!({"action": "submit", "id": id, "version": draft.version}))
        .await
        .map_err(|e| hub_words(&e))?;
    Ok(Submitted {
        artifact_id: id,
        status: out
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("submitted")
            .to_string(),
        message: out
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

// ── Reading the app from disk ───────────────────────────────────────

struct AppOnDisk {
    manifest: Value,
    agent_md: String,
    index_html: Option<String>,
    package: Option<PathBuf>,
    ui: Option<PathBuf>,
}

fn read_app(app: &db::models::Agent) -> AppOnDisk {
    let package = package_dir(app);
    let ui = ui_dir(app);
    let manifest = package
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p.join("manifest.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let agent_md = package
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p.join("AGENT.md")).ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| app.agent_md.clone());
    let index_html = ui
        .as_ref()
        .and_then(|u| std::fs::read_to_string(u.join("index.html")).ok());
    AppOnDisk {
        manifest,
        agent_md,
        index_html,
        package,
        ui,
    }
}

fn load_listing(store: &db::Store, app_id: &str) -> (db::AppListing, Option<ListingDraft>) {
    let row = store
        .get_app_listing(app_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| db::AppListing {
            app_id: app_id.to_string(),
            status: "draft".into(),
            draft: "{}".into(),
            ..Default::default()
        });
    let draft = serde_json::from_str::<ListingDraft>(&row.draft)
        .ok()
        .filter(|d| !d.name.is_empty() || !d.screenshots.is_empty());
    (row, draft)
}

/// The version last sent for review, when one was.
fn published_version(row: &db::AppListing) -> Option<&str> {
    (!row.artifact_id.is_empty() && !row.version.is_empty()).then_some(row.version.as_str())
}

// ── app_screenshot ──────────────────────────────────────────────────

/// `app_screenshot(app)`: the app as it is served, in the bot's own
/// headless browser. The picture is saved in the workspace and stored
/// through the one upload path; the employee gets the vision helper's
/// reading of it, never the image.
pub struct AppScreenshotTool {
    store: Arc<db::Store>,
    browser: Option<Arc<browser::Manager>>,
    hub: Option<Arc<dyn PublishHub>>,
}

impl AppScreenshotTool {
    pub fn new(store: Arc<db::Store>, browser: Option<Arc<browser::Manager>>) -> Self {
        Self {
            store,
            browser,
            hub: None,
        }
    }

    #[cfg(test)]
    fn with_hub(mut self, hub: Arc<dyn PublishHub>) -> Self {
        self.hub = Some(hub);
        self
    }

    fn hub(&self) -> Option<Arc<dyn PublishHub>> {
        self.hub.clone().or_else(|| {
            crate::build_neboai_api(&self.store)
                .ok()
                .map(|api| Arc::new(HubClient(api)) as Arc<dyn PublishHub>)
        })
    }

    async fn shoot(&self, ctx: &ToolContext, input: &Value) -> ToolResult {
        let app = match permitted_app(&self.store, ctx, input["app"].as_str()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        let page = match page_path(&app, input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };
        let executor = match headless(self.browser.as_ref(), "take a screenshot of the app") {
            Ok(e) => e,
            Err(e) => return ToolResult::error(e),
        };
        let (width, height) = viewport(&app, input, (390, 844));
        let wait = Duration::from_millis(input["wait_ms"].as_u64().unwrap_or(1500).min(10_000));

        // What the page logs while it loads lands in the app's console ring
        // (its developer script posts it); the shot reports this load's lines.
        let console_mark = crate::app_console::recent(&app.id, None, 1).last().map(|e| e.seq);
        let opened = AppPage::open(&executor, &app, &page, (width, height), &[], Duration::from_secs(120)).await;
        let shot = match &opened {
            Ok(view) => {
                tokio::time::sleep(wait).await;
                match view.failures().await {
                    Ok(failed) if failed.is_empty() => view
                        .call("screenshot", &json!({ "format": "png" }))
                        .await,
                    Ok(failed) => Err(failed_load(&app, &page, &failed)),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e.clone()),
        };
        if let Ok(view) = opened {
            view.close().await;
        }
        let shot = match shot {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::error(format!(
                    "{e}{}",
                    load_console(&crate::app_console::recent(&app.id, console_mark, 200))
                ));
            }
        };
        let Some(bytes) = shot["data"].as_str().and_then(|d| {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, d).ok()
        }) else {
            return ToolResult::error("The browser returned no picture.");
        };

        // The copy in the workspace, where the owner and the employee find it.
        let files = match config::data_dir() {
            Ok(d) => d.join("files"),
            Err(e) => return ToolResult::error(format!("cannot find the workspace: {e}")),
        };
        let folder = Path::new("app-screenshots").join(safe_name(&app.name));
        let file_name = format!(
            "{}-{}x{}.png",
            chrono::Utc::now().format("%Y%m%d-%H%M%S%3f"),
            width,
            height
        );
        let rel = folder.join(&file_name);
        if let Err(e) = std::fs::create_dir_all(files.join(&folder))
            .and_then(|_| std::fs::write(files.join(&rel), &bytes))
        {
            return ToolResult::error(format!("could not save the screenshot: {e}"));
        }
        let local = files.join(&rel);
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        // The one upload path: the file the listing (and anyone it is
        // shared with) can name.
        let uploaded = match self.hub() {
            Some(hub) => hub.upload_file(&file_name, "image/png", bytes).await,
            None => Err("this bot is not signed in to NeboAI".to_string()),
        };
        let label = input["label"]
            .as_str()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or(&page)
            .to_string();
        let mut text = format!(
            "Screenshot of {} at {}x{} ({}), saved as {}.",
            app.name,
            width,
            height,
            page,
            local.display()
        );
        match &uploaded {
            Ok(file_id) => {
                text.push_str(&format!(" Stored for sharing as file {file_id}."));
                if input["for_listing"].as_bool().unwrap_or(false) {
                    let (mut row, draft) = load_listing(&self.store, &app.id);
                    let mut draft = draft.unwrap_or_default();
                    draft.screenshots.push(ListingShot {
                        file_id: file_id.clone(),
                        path: rel_str.clone(),
                        label,
                    });
                    if draft.screenshots.len() > MAX_SCREENSHOTS {
                        let extra = draft.screenshots.len() - MAX_SCREENSHOTS;
                        draft.screenshots.drain(..extra);
                    }
                    row.draft = serde_json::to_string(&draft).unwrap_or_else(|_| "{}".into());
                    if let Err(e) = self.store.put_app_listing(&row) {
                        return ToolResult::error(format!(
                            "could not add the screenshot to the listing: {e}"
                        ));
                    }
                    text.push_str(&format!(
                        " Added to the listing as screenshot {} of {}.",
                        draft.screenshots.len(),
                        draft.screenshots.len()
                    ));
                }
            }
            Err(e) => {
                text.push_str(&format!(
                    " It was not stored for sharing ({e}), so it can't go on a listing."
                ));
            }
        }
        text.push_str(&load_console(&crate::app_console::recent(&app.id, console_mark, 200)));
        ToolResult::ok(text).with_image_url(local.to_string_lossy().to_string())
    }
}

// ── An app's page in the headless browser ───────────────────────────

/// The bot's own headless browser, which draws Nebo's own pages
/// (`ActionExecutor::execute_headless`); `to` says what it is needed for.
pub(crate) fn headless(
    browser: Option<&Arc<browser::Manager>>,
    to: &str,
) -> Result<browser::ActionExecutor, String> {
    browser
        .and_then(|m| m.executor())
        .filter(|e| e.cdp_available())
        .ok_or_else(|| format!("This bot has no built-in browser, so it can't {to}."))
}

/// The viewport a pack tool draws the app at: `width`/`height` from the
/// call, else the app's window, else `fallback`.
pub(crate) fn viewport(app: &db::models::Agent, input: &Value, fallback: (u64, u64)) -> (u64, u64) {
    let window = app
        .app_window_config
        .as_deref()
        .and_then(|w| serde_json::from_str::<Value>(w).ok());
    let dim = |k: &str, fallback: u64| {
        input[k]
            .as_u64()
            .or_else(|| window.as_ref().and_then(|w| w[k].as_u64()))
            .unwrap_or(fallback)
            .clamp(240, 3000)
    };
    (dim("width", fallback.0), dim("height", fallback.1))
}

/// The page a pack tool opens: the call's `path` inside the app's `ui/`
/// (default `index.html`; a query or route after it is kept). A page file
/// the app does not have is refused here: the server would answer with the
/// entry page instead, and the tool would show the wrong page as if it
/// were the one asked for.
pub(crate) fn page_path(app: &db::models::Agent, input: &Value) -> Result<String, String> {
    let Some(ui) = ui_dir(app) else {
        return Err(format!(
            "{} has no page to show yet: its ui folder is empty or missing.",
            app.name
        ));
    };
    let page = input["path"]
        .as_str()
        .map(|p| p.trim().trim_start_matches('/'))
        .filter(|p| !p.is_empty())
        .unwrap_or("index.html");
    if page.contains("..") {
        return Err("path stays inside the app (no ..).".into());
    }
    let file = page.split(['?', '#']).next().unwrap_or("");
    let is_page = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
    if is_page && !ui.join(file).is_file() {
        return Err(format!("{} has no page {file} (its ui folder has no such file).", app.name));
    }
    Ok(page.to_string())
}

/// Watches the page from before its first script: an uncaught error, a
/// rejected promise nobody handled, and a script, style or picture that
/// failed to load. Read back by [`AppPage::failures`].
const PAGE_PROBE: &str = r#"(()=>{if(window.__neboPageErrors)return;const E=window.__neboPageErrors=[];const add=m=>{if(E.length<20)E.push(String(m).slice(0,500));};const short=u=>{u=String(u||'');if(u.indexOf(location.origin)===0)u=u.slice(location.origin.length);return u.replace(/^\/k\/[^\/]+/,'');};window.addEventListener('error',ev=>{const t=ev.target;if(t&&t!==window&&t.nodeType===1){add('Failed to load '+short(t.src||t.href||t.tagName));return;}add((ev.message||'Error')+(ev.filename?' ('+short(ev.filename)+':'+ev.lineno+')':''));},true);window.addEventListener('unhandledrejection',ev=>{const r=ev.reason;add('Unhandled promise rejection: '+(r&&r.message?r.message:String(r)));});})();"#;

/// What the probe saw, and the answer the page itself came with.
const PAGE_FAILURES: &str = r#"(()=>{const n=performance.getEntriesByType('navigation')[0];const s=n&&n.responseStatus||0;if(!window.__neboPageErrors)return JSON.stringify(['The page did not load'+(s?' (it answered '+s+')':'')+'.']);const e=window.__neboPageErrors.slice();if(s>=400)e.unshift('The page answered '+s+'.');return JSON.stringify(e);})()"#;

/// An app's page open in the bot's own headless browser, for a pack tool
/// (`app_screenshot`, `app_record`): one tab of its own, behind a pass that
/// opens this app alone, laid out at the size it is drawn at before its
/// first script runs, and watched for errors from the start
/// ([`PAGE_PROBE`]). Closed with [`AppPage::close`].
pub(crate) struct AppPage<'a> {
    executor: &'a browser::ActionExecutor,
    session: String,
    pass: String,
}

impl<'a> AppPage<'a> {
    /// Open `page` (from [`page_path`]) at `size`. `scripts` run before the
    /// page's own, after the probe; the pass lasts `ttl`.
    pub(crate) async fn open(
        executor: &'a browser::ActionExecutor,
        app: &db::models::Agent,
        page: &str,
        (width, height): (u64, u64),
        scripts: &[&str],
        ttl: Duration,
    ) -> Result<AppPage<'a>, String> {
        let view = AppPage {
            executor,
            session: format!("app-page-{}", uuid::Uuid::new_v4().simple()),
            pass: napp::app_view::grant(&app.id, ttl),
        };
        let url = format!(
            "http://127.0.0.1:{}/k/{}/apps/{}/ui/{page}",
            local_port(),
            view.pass,
            app.id
        );
        let opened = async {
            view.call("viewport", &json!({ "width": width, "height": height })).await?;
            for source in std::iter::once(PAGE_PROBE).chain(scripts.iter().copied()) {
                view.call("init_script", &json!({ "source": source })).await?;
            }
            view.call("navigate", &json!({ "url": url })).await
        }
        .await;
        match opened {
            Ok(_) => Ok(view),
            Err(e) => {
                view.close().await;
                Err(format!("The browser could not open {}: {e}", app.name))
            }
        }
    }

    /// One browser action in this page's tab.
    pub(crate) async fn call(&self, tool: &str, args: &Value) -> Result<Value, String> {
        self.executor
            .execute_headless(tool, args, &self.session)
            .await
            .map_err(|e| e.to_string())
    }

    /// The value of `expression` in the page, as text.
    pub(crate) async fn eval(&self, expression: &str) -> Result<String, String> {
        let v = self.call("evaluate", &json!({ "expression": expression })).await?;
        Ok(v["text"].as_str().unwrap_or_default().to_string())
    }

    /// What has gone wrong with the page since it opened: the answer it
    /// came with, when that was an error, and what [`PAGE_PROBE`] saw.
    /// Empty when nothing did.
    pub(crate) async fn failures(&self) -> Result<Vec<String>, String> {
        let text = self.eval(PAGE_FAILURES).await?;
        serde_json::from_str(&text).map_err(|_| format!("could not read the page's errors: {text}"))
    }

    /// Close the tab and drop the pass.
    pub(crate) async fn close(self) {
        self.executor.close_session(&self.session).await;
        napp::app_view::revoke(&self.pass);
    }
}

/// A page that failed, said as the error the call returns.
pub(crate) fn failed_load(app: &db::models::Agent, page: &str, failed: &[String]) -> String {
    let mut out = format!(
        "{}'s page {page} failed in the browser. Fix it before saying the app works:",
        app.name
    );
    for f in failed {
        out.push_str("\n- ");
        out.push_str(f);
    }
    out
}

/// What a screenshot's own load put in the app's console: its errors and
/// warnings word for word, so a broken page reaches the employee in the
/// call that looked at it, not only when someone opens the console.
pub(crate) fn load_console(entries: &[crate::app_console::Entry]) -> String {
    let worth: Vec<&crate::app_console::Entry> =
        entries.iter().filter(|e| e.level == "error" || e.level == "warn").collect();
    let logged = entries.len() - worth.len();
    if worth.is_empty() {
        return format!(
            "\nConsole during this load: no errors ({logged} other line{}).",
            if logged == 1 { "" } else { "s" }
        );
    }
    let mut out = format!(
        "\nConsole during this load: {} error or warning line{}. Fix them before saying the app works:",
        worth.len(),
        if worth.len() == 1 { "" } else { "s" }
    );
    for e in worth.iter().take(20) {
        out.push_str("\n");
        out.push_str(&crate::app_console::format_entry(e));
    }
    out
}

/// A folder name from an app's name.
pub(crate) fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "app".into() } else { s }
}

impl DynTool for AppScreenshotTool {
    fn name(&self) -> &str {
        APP_SCREENSHOT
    }

    fn description(&self) -> String {
        "Takes a screenshot of an app as it is served, in this bot's own headless browser, and tells you what it shows.\n\
         - `app`: the app's name; leave it out when you are the app.\n\
         - `path`: a page or route inside the app (default index.html, e.g. \"index.html#/scores\").\n\
         - `width`/`height`: the viewport (default the app's window, else a phone at 390x844). Use a phone, a tablet and a desktop size for a listing.\n\
         - `for_listing: true` adds it to the app's marketplace listing (3 to 5 make a good listing; the first is the card image).\n\
         - You get a written reading of the picture; the file is saved in the workspace under app-screenshots/."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app": { "type": "string", "description": "The app's name or id. Leave out when you are the app." },
                "path": { "type": "string", "description": "A page or route inside the app, e.g. index.html#/scores." },
                "width": { "type": "integer", "description": "Viewport width in pixels." },
                "height": { "type": "integer", "description": "Viewport height in pixels." },
                "wait_ms": { "type": "integer", "description": "How long the page settles before the shot (default 1500, at most 10000)." },
                "for_listing": { "type": "boolean", "description": "Add it to the app's marketplace listing." },
                "label": { "type": "string", "description": "A few words on what the shot shows, for the listing." }
            }
        })
    }

    fn search_hint(&self) -> &str {
        "screenshot an app listing picture"
    }

    fn read_only(&self, input: &Value) -> bool {
        !input["for_listing"].as_bool().unwrap_or(false)
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn emits_image(&self, input: &Value) -> bool {
        input["for_listing"].as_bool().unwrap_or(false)
    }

    fn activity(&self, _input: &Value) -> String {
        "taking a screenshot of the app".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Took a screenshot of the app".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.shoot(ctx, &input).await })
    }
}

// ── app_listing and app_submit ──────────────────────────────────────

/// What `app_listing` and `app_submit` share: the store and the publish
/// path.
pub struct Publisher {
    store: Arc<db::Store>,
    hub: Option<Arc<dyn PublishHub>>,
}

impl Publisher {
    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store, hub: None }
    }

    #[cfg(test)]
    fn with_hub(mut self, hub: Arc<dyn PublishHub>) -> Self {
        self.hub = Some(hub);
        self
    }

    fn hub(&self) -> Option<Arc<dyn PublishHub>> {
        self.hub.clone().or_else(|| {
            crate::build_neboai_api(&self.store)
                .ok()
                .map(|api| Arc::new(HubClient(api)) as Arc<dyn PublishHub>)
        })
    }

    async fn categories(&self) -> Vec<String> {
        match self.hub() {
            Some(hub) => hub.categories().await,
            None => Vec::new(),
        }
    }

    /// The app a call is about, with the mode checked first.
    fn app(&self, ctx: &ToolContext, input: &Value) -> Result<db::models::Agent, ToolResult> {
        permitted_app(&self.store, ctx, input["app"].as_str()).map_err(ToolResult::error)
    }

    /// Draft the listing (or redraft it with the owner's changes) and show
    /// it, with where it stands and what keeps it from being submitted.
    async fn draft(&self, app: &db::models::Agent, input: &Value) -> ToolResult {
        let disk = read_app(app);
        let (mut row, previous) = load_listing(&self.store, &app.id);
        let categories = self.categories().await;
        let mut draft = build_draft(&DraftSource {
            display_name: &app.name,
            description: &app.description,
            manifest: &disk.manifest,
            agent_md: &disk.agent_md,
            index_html: disk.index_html.as_deref(),
            previous: previous.as_ref(),
            published_version: published_version(&row),
            categories: &categories,
        });
        let before = draft.clone();
        if let Err(e) = apply_edits(&mut draft, input) {
            return ToolResult::error(e);
        }
        let changed = draft != before || previous.as_ref() != Some(&draft);
        if changed
            && (row.status == "approved" || row.status == "rejected" || row.artifact_id.is_empty())
        {
            // A new round of shaping: what was submitted before stays named
            // by its artifact; the listing is a draft again.
            row.status = "draft".into();
        }
        row.draft = serde_json::to_string(&draft).unwrap_or_else(|_| "{}".into());
        if let Err(e) = self.store.put_app_listing(&row) {
            return ToolResult::error(format!("could not save the listing: {e}"));
        }
        let todo = problems(&draft, &categories);
        let mut text = render_listing(&draft, &row.status);
        if !row.notes.is_empty() {
            text.push_str(&format!(
                "\nThe reviewer's notes on v{}: {}\n",
                row.version, row.notes
            ));
        }
        if app
            .app_binary_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
        {
            text.push_str(
                "\nThis app runs a program of its own beside its page. Publishing from a conversation covers apps that \
                 are a page; tell the owner that part needs the full publishing tools.\n",
            );
        }
        if todo.is_empty() {
            text.push_str(
                "\nShow the owner this listing and ask what to change. When they say to send it, call app_submit: they \
                 confirm on a card before anything leaves this bot.",
            );
        } else {
            text.push_str("\nBefore it can be submitted:\n");
            for p in &todo {
                text.push_str(&format!("- {p}\n"));
            }
        }
        ToolResult::ok(text).with_payload(listing_payload(
            &app.id,
            &row.artifact_id,
            &draft,
            &row.status,
            &row.notes,
        ))
    }

    /// The owner's yes on a card, then the publish path.
    async fn submit(&self, ctx: &ToolContext, app: &db::models::Agent) -> ToolResult {
        let (mut row, draft) = load_listing(&self.store, &app.id);
        let Some(draft) = draft else {
            return ToolResult::error("There is no listing yet. Draft it first with app_listing.");
        };
        let todo = problems(&draft, &self.categories().await);
        if !todo.is_empty() {
            return ToolResult::error(format!("Not ready to submit:\n- {}", todo.join("\n- ")));
        }
        if app
            .app_binary_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
        {
            return ToolResult::error(
                "This app runs a program of its own beside its page; publishing from a conversation covers apps that are \
                 a page. Nothing was sent.",
            );
        }
        let disk = read_app(app);
        let Some(ui) = disk.ui.as_deref() else {
            return ToolResult::error(format!("{} has no page folder to publish.", app.name));
        };
        let user_skills = config::user_dir().ok().map(|d| d.join("skills"));
        let (bundle, _pages) = match build_bundle(&disk.agent_md, disk.package.as_deref(), ui, user_skills.as_deref()) {
            Ok(b) => b,
            Err(e) => return ToolResult::error(e),
        };
        let Some(hub) = self.hub() else {
            return ToolResult::error(
                "This bot is not signed in to NeboAI, so it can't reach the marketplace. Connect it in Settings, NeboAI.",
            );
        };
        // The owner's yes, on a card. Nothing leaves this bot without it.
        if let Err(e) = owner_says_submit(ctx, &draft).await {
            return ToolResult::error(e);
        }
        match publish_to_hub(
            hub.as_ref(),
            &row.artifact_id,
            &draft,
            &disk.agent_md,
            bundle,
        )
        .await
        {
            Ok(sent) => {
                row.artifact_id = sent.artifact_id.clone();
                row.status = listing_status(&sent.status).to_string();
                row.version = draft.version.clone();
                row.notes = String::new();
                row.chat_session = ctx.session_key.clone();
                if let Err(e) = self.store.put_app_listing(&row) {
                    tracing::warn!(app = %app.id, error = %e, "submitted listing not recorded");
                }
                let mut text = if row.status == "approved" {
                    format!(
                        "{} v{} passed review and is live ({}).",
                        draft.name,
                        draft.version,
                        visibility_label(&draft.visibility)
                    )
                } else {
                    format!(
                        "Submitted {} v{} for review ({}). When the review finishes, the outcome is posted in this chat.",
                        draft.name,
                        draft.version,
                        visibility_label(&draft.visibility)
                    )
                };
                if !sent.message.is_empty() {
                    text.push_str(&format!("\n{}", sent.message));
                }
                ToolResult::ok(text).with_payload(listing_payload(
                    &app.id,
                    &row.artifact_id,
                    &draft,
                    &row.status,
                    "",
                ))
            }
            Err(e) => ToolResult::error(e),
        }
    }
}

/// `app_listing`: draft an app's marketplace listing, change it as the
/// owner asks, and show where it stands.
pub struct AppListingTool(pub Arc<Publisher>);

impl DynTool for AppListingTool {
    fn name(&self) -> &str {
        APP_LISTING
    }

    fn description(&self) -> String {
        "Drafts an app's marketplace listing from its manifest and code, changes it as the owner asks, and shows it with where its review stands.\n\
         - Pass any of name, short_description (one line for the card), long_description ('What it does', benefits first), category, version, visibility (public, unlisted, private) to change them.\n\
         - Pass screenshots (their file ids, in order) to drop or reorder them; the first is the card image. New screenshots come from app_screenshot with for_listing: true.\n\
         - Show the owner the listing; send it with app_submit when they say so.\n\
         - `app`: the app's name; leave it out when you are the app."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app": { "type": "string", "description": "The app's name or id. Leave out when you are the app." },
                "name": { "type": "string" },
                "short_description": { "type": "string", "description": "One line for the marketplace card, 10 to 500 characters." },
                "long_description": { "type": "string", "description": "The listing page's 'What it does', benefits first, plain words." },
                "category": { "type": "string" },
                "version": { "type": "string", "description": "Like 1.2.0." },
                "visibility": { "type": "string", "enum": ["public", "unlisted", "private"] },
                "screenshots": { "type": "array", "items": { "type": "string" }, "description": "The listing's screenshot file ids to keep, in order; the first is the card image." }
            }
        })
    }

    fn search_hint(&self) -> &str {
        "draft app marketplace listing"
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        // A draft on this bot: nothing leaves it.
        types::permissions::CallEffects::none()
    }

    fn activity(&self, _input: &Value) -> String {
        "drafting the listing".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Drafted the listing".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.0.app(ctx, &input) {
                Ok(app) => self.0.draft(&app, &input).await,
                Err(refusal) => refusal,
            }
        })
    }
}

/// `app_submit`: send the listing for review, after the owner's yes on a
/// card in this conversation.
pub struct AppSubmitTool(pub Arc<Publisher>);

impl DynTool for AppSubmitTool {
    fn name(&self) -> &str {
        APP_SUBMIT
    }

    fn description(&self) -> String {
        "Sends an app's listing (drafted with app_listing) to the marketplace for review.\n\
         - The owner confirms on a card in this conversation; only their yes sends it, and a 'Not yet' sends nothing.\n\
         - Call it when the owner says to send it, never because you think it is ready, and never in a run nobody is watching.\n\
         - The review's outcome is posted in this chat when it comes.\n\
         - `app`: the app's name; leave it out when you are the app."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app": { "type": "string", "description": "The app's name or id. Leave out when you are the app." }
            }
        })
    }

    fn search_hint(&self) -> &str {
        "publish submit app for marketplace review"
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        // It publishes; the owner's card is the consent.
        types::permissions::CallEffects {
            publishes: types::permissions::Knowable::Yes,
            ..types::permissions::CallEffects::none()
        }
    }

    fn activity(&self, _input: &Value) -> String {
        "submitting the app".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Submitted the app".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.0.app(ctx, &input) {
                Ok(app) => self.0.submit(ctx, &app).await,
                Err(refusal) => refusal,
            }
        })
    }
}

#[cfg(test)]
mod tests;
