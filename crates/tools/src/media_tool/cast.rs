//! The cast: the people a video swap may put on screen. Each member is a
//! folder under `<data_dir>/files/cast/<name>/` that every employee on the
//! bot reads: a `hero` image (front-facing, full body, good light), extra
//! angles, and `cast.json` with the owner's one-time attestation and the
//! hub file ids the images were stored under.
//!
//! A replace takes its person only from here, and only from a member the
//! owner attested to on a card in his own chat. That is the guardrail: no
//! face image ever reaches a swap any other way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::origin::ToolContext;

/// The file in each member's folder that says who they are.
pub const CAST_JSON: &str = "cast.json";

/// The image formats a cast image may be.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// The attestation card's answers, with the `type` each records.
const ANSWERS: &[(&str, &str)] = &[
    ("Someone who gave consent or a release", "release"),
    ("An AI persona", "ai_persona"),
    ("Me", "owner"),
];
const NO_ANSWER: &str = "No, don't use them";

/// One cast member's `cast.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub name: String,
    /// `release`, `ai_persona` or `owner`; set by the attestation.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub attestation: Option<Attestation>,
    /// Image file name (`hero.png`) to the id the hub stored it under, so
    /// an image goes up once.
    #[serde(default)]
    pub hub_file_ids: BTreeMap<String, String>,
}

/// The owner's one-time confirmation for a member: what he confirmed,
/// when, and the id every swap sends Janus.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Attestation {
    pub id: String,
    pub by: String,
    pub at: String,
    pub statements: Vec<String>,
}

/// The cast folder.
pub struct Cast {
    pub root: PathBuf,
}

/// A member's name as a folder name: letters, digits, spaces, `-`, `_`
/// and `.`, never a path.
pub fn valid_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= 64
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'));
    if ok {
        Ok(name)
    } else {
        Err(format!(
            "`{name}` can't be a cast member's name: use letters, digits, spaces, - or _ (up to 64)."
        ))
    }
}

/// The statements the owner confirms for `name`, as the card shows them
/// and `cast.json` keeps them.
pub fn statements(name: &str) -> Vec<String> {
    vec![
        format!("You have {name}'s consent or a signed release, or {name} is an AI persona, or {name} is you."),
        format!("{name} is not a public figure or a celebrity lookalike."),
        format!("{name} won't be presented as a real customer giving a testimonial unless that is true."),
        format!("Any ad with {name} will be labelled as AI-generated where the law requires it."),
    ]
}

/// The attestation card's question for `name`.
pub fn question(name: &str) -> String {
    let lines: Vec<String> = statements(name).iter().map(|s| format!("- {s}")).collect();
    format!(
        "Before {name} can play a person in a video, confirm all of this is true:\n{}\n\nWho is {name}?",
        lines.join("\n")
    )
}

impl Cast {
    /// The member's folder, matched exactly or else by case.
    pub fn dir(&self, name: &str) -> Result<PathBuf, String> {
        let name = valid_name(name)?;
        let exact = self.root.join(name);
        if exact.is_dir() {
            return Ok(exact);
        }
        let found = std::fs::read_dir(&self.root).ok().and_then(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .find(|p| p.is_dir() && p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name)))
        });
        Ok(found.unwrap_or(exact))
    }

    /// The member called `name`, with their folder.
    pub fn load(&self, name: &str) -> Result<(PathBuf, Member), String> {
        let dir = self.dir(name)?;
        let text = std::fs::read_to_string(dir.join(CAST_JSON)).map_err(|_| {
            format!(
                "There is no cast member named {name}. Add them with generate_media kind \"cast\", cast \"{name}\" and \
                 `image` a front-facing, full-body photo in good light; the owner confirms them once."
            )
        })?;
        let member: Member =
            serde_json::from_str(&text).map_err(|e| format!("{}'s cast.json could not be read: {e}", dir.display()))?;
        Ok((dir, member))
    }

    /// Writes `member` into `dir`, whole or not at all.
    pub fn save(&self, dir: &Path, member: &Member) -> Result<(), String> {
        let text = serde_json::to_string_pretty(member).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{CAST_JSON}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join(CAST_JSON)).map_err(|e| format!("Could not write {}: {e}", dir.display()))
    }

    /// Starts member `name` with `hero` as their hero image. Refused when
    /// they already exist.
    pub fn create(&self, name: &str, hero: &Path) -> Result<(PathBuf, Member), String> {
        let name = valid_name(name)?;
        let ext = image_ext(hero)?;
        if self.load(name).is_ok() {
            return Err(format!("{name} is already in the cast."));
        }
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
        std::fs::copy(hero, dir.join(format!("hero.{ext}")))
            .map_err(|e| format!("Could not copy {}: {e}", hero.display()))?;
        let member = Member { name: name.to_string(), ..Default::default() };
        self.save(&dir, &member)?;
        Ok((dir, member))
    }

    /// Adds `image` to member `name`: as their new hero (the old one and
    /// its stored copy are dropped), or as one more angle. The file name
    /// it was saved as.
    pub fn add(&self, name: &str, image: &Path, hero: bool) -> Result<String, String> {
        let ext = image_ext(image)?;
        let (dir, mut member) = self.load(name)?;
        let file = if hero {
            for old in self.images(&dir).into_iter().filter(|p| is_hero(p)) {
                let _ = std::fs::remove_file(&old);
                if let Some(f) = old.file_name() {
                    member.hub_file_ids.remove(f.to_string_lossy().as_ref());
                }
            }
            format!("hero.{ext}")
        } else {
            let n = (1..).find(|n| !IMAGE_EXTS.iter().any(|e| dir.join(format!("angle-{n}.{e}")).exists())).unwrap_or(1);
            format!("angle-{n}.{ext}")
        };
        std::fs::copy(image, dir.join(&file)).map_err(|e| format!("Could not copy {}: {e}", image.display()))?;
        self.save(&dir, &member)?;
        Ok(file)
    }

    /// A member's images: the hero first, then the angles by name. A
    /// model that takes one image always gets the hero, so the person stays
    /// the same from video to video.
    pub fn images(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && image_ext(p).is_ok())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|p| (!is_hero(p), p.file_name().map(|f| f.to_os_string())));
        out
    }

    /// Every member, by name.
    pub fn list(&self) -> Vec<(PathBuf, Member)> {
        let mut out: Vec<(PathBuf, Member)> = std::fs::read_dir(&self.root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let dir = e.path();
                        let text = std::fs::read_to_string(dir.join(CAST_JSON)).ok()?;
                        Some((dir, serde_json::from_str(&text).ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()));
        out
    }
}

fn is_hero(path: &Path) -> bool {
    path.file_stem().is_some_and(|s| s == "hero")
}

/// `path`'s image extension, lowercased, when it is one a cast image may be.
fn image_ext(path: &Path) -> Result<String, String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| IMAGE_EXTS.contains(&e.as_str()))
        .ok_or_else(|| format!("{} is not a PNG, JPEG or WebP image.", path.display()))
}

/// What a replace needs from a member before anything is uploaded: the
/// owner's attestation and a hero image.
pub fn ready<'a>(member: &'a Member, images: &[PathBuf]) -> Result<&'a Attestation, String> {
    let name = &member.name;
    let attestation = member.attestation.as_ref().ok_or_else(|| {
        format!(
            "{name} can't be used to replace a person in a video yet: the owner hasn't confirmed {name}'s consent. \
             When the owner is in a chat with you, call generate_media kind \"cast\" with cast \"{name}\" to show them \
             the confirmation. Nothing was uploaded or made."
        )
    })?;
    if !images.first().is_some_and(|p| is_hero(p)) {
        return Err(format!(
            "{name} has no hero image. Add one with generate_media kind \"cast\", cast \"{name}\", `hero` true and \
             `image` a front-facing, full-body photo in good light."
        ));
    }
    Ok(attestation)
}

/// Shows the owner the attestation card for `name` in the chat that asked,
/// and answers the member's type and attestation when he confirms. Only
/// the owner, on the card, ever confirms: an unattended run, or a reply
/// that only sounds like yes, never does.
pub async fn attest(ctx: &ToolContext, name: &str) -> Result<(String, Attestation), String> {
    if crate::origin::ExecutionMode::from(ctx.origin) != crate::origin::ExecutionMode::Interactive
        || ctx.ask_channels.is_none()
        || ctx.stream_tx.is_none()
    {
        return Err(format!(
            "Only the owner can confirm {name}, on a card in a chat with you, and nobody is here to see it. When they \
             are, call generate_media kind \"cast\" with cast \"{name}\"."
        ));
    }
    let mut options: Vec<&str> = ANSWERS.iter().map(|(label, _)| *label).collect();
    options.push(NO_ANSWER);
    let widgets = json!([{ "type": "options", "multiSelect": false, "options": options }]);
    let answer = ctx.ask_user(&question(name), widgets).await;
    let kind = answer
        .as_deref()
        .and_then(|a| ANSWERS.iter().find(|(label, _)| a.trim().eq_ignore_ascii_case(label)))
        .map(|(_, kind)| kind.to_string());
    match kind {
        Some(kind) => Ok((
            kind,
            Attestation {
                id: uuid::Uuid::new_v4().to_string(),
                by: if ctx.user_id.is_empty() { "owner".to_string() } else { ctx.user_id.clone() },
                at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                statements: statements(name),
            },
        )),
        None => Err(format!(
            "The owner did not confirm {name}, so {name} can't be used to replace a person in a video. Don't ask again \
             unless the owner brings it up."
        )),
    }
}
