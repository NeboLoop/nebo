//! `generate_media`: images and video made through Janus and saved as files,
//! into an app's served folder (`agents.app_ui_path`) or the workspace.
//!
//! Janus answers images inline (`b64_json`) and video as a job: submit,
//! poll until it has finished, download the MP4. A film meant for
//! scroll-scrubbing is re-encoded here with every frame a keyframe, because
//! Janus has no encoder and the bot has the file on disk anyway.
//!
//! The model only ever gets paths back, never pixels: images reach a model
//! through the vision helper alone.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const GENERATE_MEDIA: &str = "generate_media";

/// The most images one call makes (Janus's own cap).
const MAX_IMAGES: u64 = 4;
/// The longest video Janus makes, in seconds.
const MAX_SECONDS: u64 = 30;

/// How a video job is waited on: the first wait, the longest wait between
/// polls, and how long in all before giving up.
#[derive(Debug, Clone, Copy)]
pub struct Polling {
    pub first: Duration,
    pub max: Duration,
    pub limit: Duration,
}

impl Default for Polling {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            max: Duration::from_secs(10),
            limit: Duration::from_secs(600),
        }
    }
}

impl Polling {
    /// The wait after `current`: doubled, never past `max`.
    fn next(&self, current: Duration) -> Duration {
        (current * 2).min(self.max)
    }
}

/// Where Janus is and who this bot is to it.
pub struct Media {
    base_url: String,
    bot_id: String,
    store: Option<Arc<db::Store>>,
    client: reqwest::Client,
    polling: Polling,
}

/// One image Janus made.
#[derive(Debug)]
pub struct Image {
    pub bytes: Vec<u8>,
    pub revised_prompt: Option<String>,
}

/// A finished video job.
#[derive(Debug)]
pub struct Video {
    pub id: String,
    pub model: String,
    pub seconds: Option<u64>,
}

impl Media {
    /// `base_url` is the Janus root without `/v1`.
    pub fn new(base_url: String, bot_id: String, store: Option<Arc<db::Store>>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            bot_id,
            store,
            client: reqwest::Client::new(),
            polling: Polling::default(),
        }
    }

    pub fn with_polling(mut self, polling: Polling) -> Self {
        self.polling = polling;
        self
    }

    /// A Janus request with the one Janus auth: the bearer and `X-Bot-ID`.
    fn request(&self, method: reqwest::Method, path: &str) -> (reqwest::RequestBuilder, bool) {
        let (bearer, signed_in) = crate::janus::bearer(self.store.as_deref(), &self.bot_id);
        let req = self
            .client
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(bearer)
            .header("X-Bot-ID", &self.bot_id);
        (req, signed_in)
    }

    /// Images for `body` (a `/v1/images/generations` request) and the model
    /// that made them.
    pub async fn images(&self, body: &Value) -> Result<(Vec<Image>, String), String> {
        let (req, signed_in) = self.request(reqwest::Method::POST, "/v1/images/generations");
        let resp = req
            .json(body)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| format!("Could not reach Janus to make the image: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("image", status.as_u16(), &text, signed_in));
        }
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| format!("Janus's answer for the image could not be read: {e}"))?;
        let model = parsed
            .get("model")
            .and_then(Value::as_str)
            .or_else(|| body.get("model").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let mut out = Vec::new();
        for item in parsed
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(b64) = item.get("b64_json").and_then(Value::as_str) else {
                continue;
            };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("Janus's image could not be decoded: {e}"))?;
            out.push(Image {
                bytes,
                revised_prompt: item
                    .get("revised_prompt")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            });
        }
        if out.is_empty() {
            return Err("Janus answered with no image.".to_string());
        }
        Ok((out, model))
    }

    /// Submits a video job for `body` (a `/v1/videos` request).
    pub async fn submit_video(&self, body: &Value) -> Result<Video, String> {
        let (req, signed_in) = self.request(reqwest::Method::POST, "/v1/videos");
        let resp = req
            .json(body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("Could not reach Janus to make the video: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("video", status.as_u16(), &text, signed_in));
        }
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| format!("Janus's answer for the video could not be read: {e}"))?;
        let id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Janus answered the video request with no job id.")?
            .to_string();
        if parsed.get("status").and_then(Value::as_str) == Some("failed") {
            return Err(job_failed(&parsed));
        }
        Ok(Video {
            id,
            model: parsed
                .get("model")
                .and_then(Value::as_str)
                .or_else(|| body.get("model").and_then(Value::as_str))
                .unwrap_or("")
                .to_string(),
            seconds: parsed.get("seconds").and_then(Value::as_u64),
        })
    }

    /// Waits for video job `id` to finish: polls with a growing wait until
    /// it has succeeded, failed, or the limit has passed. A poll that does
    /// not get an answer is retried; a refusal ends the wait.
    pub async fn wait_video(&self, id: &str) -> Result<(), String> {
        let started = tokio::time::Instant::now();
        let mut wait = self.polling.first;
        loop {
            if started.elapsed() + wait > self.polling.limit {
                return Err(format!(
                    "The video was not finished after {} minutes. Its job id is {id}: call generate_media again with \
                     kind \"video\" and job \"{id}\" to pick it up instead of making a new one.",
                    self.polling.limit.as_secs().div_ceil(60)
                ));
            }
            tokio::time::sleep(wait).await;
            wait = self.polling.next(wait);
            let (req, signed_in) = self.request(reqwest::Method::GET, &format!("/v1/videos/{id}"));
            let resp = match req.timeout(Duration::from_secs(30)).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(id, error = %e, "video poll got no answer; polling again");
                    continue;
                }
            };
            let status = resp.status();
            if status.is_server_error() {
                tracing::debug!(id, %status, "video poll answered a server error; polling again");
                continue;
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(failure("video", status.as_u16(), &text, signed_in));
            }
            let Ok(parsed) = resp.json::<Value>().await else {
                continue;
            };
            match parsed.get("status").and_then(Value::as_str) {
                Some("succeeded") => return Ok(()),
                Some("failed") => return Err(job_failed(&parsed)),
                _ => {}
            }
        }
    }

    /// Downloads finished video `id` into `path`, streamed, never held whole.
    /// The file appears only once complete.
    pub async fn download_video(&self, id: &str, path: &Path) -> Result<u64, String> {
        use tokio::io::AsyncWriteExt;
        let (req, signed_in) =
            self.request(reqwest::Method::GET, &format!("/v1/videos/{id}/content"));
        let mut resp = req
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| format!("Could not download the video from Janus: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("video", status.as_u16(), &text, signed_in));
        }
        let part = part_path(path);
        let mut file = tokio::fs::File::create(&part)
            .await
            .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
        let mut size = 0u64;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    size += chunk.len() as u64;
                    if let Err(e) = file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&part).await;
                        return Err(format!("Could not write {}: {e}", part.display()));
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(format!(
                        "The video download from Janus stopped part way: {e}"
                    ));
                }
            }
        }
        file.flush()
            .await
            .map_err(|e| format!("Could not write {}: {e}", part.display()))?;
        drop(file);
        tokio::fs::rename(&part, path)
            .await
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        Ok(size)
    }
}

/// `path` with `.part` added: where a download is written until complete.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// The sentence a Janus refusal becomes. A 429 is the plan or balance not
/// covering the work; the owner reads it, so it says what to do.
fn failure(kind: &str, status: u16, body: &str, signed_in: bool) -> String {
    match status {
        429 | 402 => format!(
            "The owner's NeboAI plan or balance does not cover this {kind}. Tell the owner plainly; they can add funds or \
             change plans under Settings > Account, then ask again."
        ),
        401 | 403 if !signed_in => crate::janus::NOT_SIGNED_IN.to_string(),
        _ => {
            let said = janus_message(body);
            if said.is_empty() {
                format!("Janus did not make the {kind} (status {status}).")
            } else {
                format!("Janus did not make the {kind} (status {status}): {said}")
            }
        }
    }
}

/// The message in a Janus error body (`{"error": {"message"}}`,
/// `{"error": "…"}`, or plain text), shortened.
fn janus_message(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let said = parsed
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .or_else(|| v.get("message"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.trim().to_string());
    said.chars().take(300).collect()
}

/// What a failed video job says.
fn job_failed(job: &Value) -> String {
    let said = job
        .pointer("/error/message")
        .or_else(|| job.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if said.is_empty() {
        "The video could not be made.".to_string()
    } else {
        format!("The video could not be made: {said}")
    }
}

/// `into` inside `base`: relative and with no `..`, or an absolute path
/// already inside `base`. Anything that would leave the folder is refused.
pub fn contained(base: &Path, into: &str) -> Result<PathBuf, String> {
    let into = into.trim();
    if into.is_empty() {
        return Err("`into` is empty.".to_string());
    }
    let named = Path::new(into);
    let rel = if named.is_absolute() {
        named.strip_prefix(base).map_err(|_| outside(into, base))?
    } else {
        named
    };
    let mut out = base.to_path_buf();
    for part in rel.components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return Err(outside(into, base)),
        }
    }
    if out == base {
        return Err("`into` must name a file, not the folder itself.".to_string());
    }
    Ok(out)
}

fn outside(into: &str, base: &Path) -> String {
    format!(
        "`{into}` is outside {}. Give a path inside that folder, such as `assets/hero.png`.",
        base.display()
    )
}

/// Makes `path`'s folder and checks that, links resolved, it is still
/// inside `base`.
fn prepare(base: &Path, path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or(base);
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not make {}: {e}", parent.display()))?;
    let real_base = base
        .canonicalize()
        .map_err(|e| format!("{}: {e}", base.display()))?;
    let real_parent = parent
        .canonicalize()
        .map_err(|e| format!("{}: {e}", parent.display()))?;
    if !real_parent.starts_with(&real_base) {
        return Err(outside(&path.display().to_string(), base));
    }
    Ok(())
}

/// The image file format for a call: `into`'s extension when it names one,
/// else `output_format`, else PNG. Answers Janus's `output_format` and the
/// file extension.
fn image_format(into: Option<&str>, output_format: Option<&str>) -> (&'static str, &'static str) {
    let from = |s: &str| match s.to_ascii_lowercase().as_str() {
        "png" => Some(("png", "png")),
        "webp" => Some(("webp", "webp")),
        "jpg" | "jpeg" => Some(("jpeg", "jpg")),
        _ => None,
    };
    into.and_then(|p| Path::new(p).extension()?.to_str().and_then(from))
        .or_else(|| output_format.and_then(from))
        .unwrap_or(("png", "png"))
}

/// The `/v1/images/generations` body for a call.
pub fn image_body(input: &Value) -> Value {
    let (format, _) = image_format(str_of(input, "into"), str_of(input, "output_format"));
    let mut body = json!({
        "model": str_of(input, "model").unwrap_or(""),
        "prompt": str_of(input, "prompt").unwrap_or(""),
        "n": input.get("n").and_then(Value::as_u64).unwrap_or(1).clamp(1, MAX_IMAGES),
        "output_format": format,
    });
    for key in ["size", "quality", "background"] {
        if let Some(v) = str_of(input, key) {
            body[key] = json!(v);
        }
    }
    body
}

/// The `/v1/videos` body for a call. `first_frame` is the start image,
/// already a URL or data URL.
pub fn video_body(input: &Value, first_frame: Option<String>) -> Value {
    let mut body = json!({
        "model": str_of(input, "model").unwrap_or(""),
        "prompt": str_of(input, "prompt").unwrap_or(""),
        "seconds": input.get("seconds").and_then(Value::as_u64).unwrap_or(5).clamp(1, MAX_SECONDS),
    });
    for key in ["resolution", "aspect_ratio"] {
        if let Some(v) = str_of(input, key) {
            body[key] = json!(v);
        }
    }
    if let Some(frame) = first_frame {
        body["image"] = json!(frame);
    }
    body
}

fn str_of<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The start frame Janus is sent: an https or data URL as given, else a
/// file inside `base`, sent as a data URL.
fn first_frame(base: &Path, image: &str) -> Result<String, String> {
    if image.starts_with("https://") || image.starts_with("data:") {
        return Ok(image.to_string());
    }
    let path = contained(base, image)?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => {
            return Err(format!(
                "`image` must be a PNG, WebP or JPEG file, an https URL or a data URL; got `{image}`."
            ));
        }
    };
    let bytes =
        std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Short, file-safe words from the prompt, for a default file name.
fn slug(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "media".to_string()
    } else {
        words.join("-")
    }
}

/// The relative file names a call writes: `into` (numbered when it makes
/// more than one), else `<folder>/<prompt words>-<time>`, with `ext` added
/// when the name has another or none.
fn file_names(
    into: Option<&str>,
    folder: &str,
    prompt: &str,
    ext: &str,
    count: usize,
    stamp: i64,
) -> Vec<String> {
    let stem = match into {
        Some(p) => {
            let path = Path::new(p);
            let has_ext = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                e.eq_ignore_ascii_case(ext) || (ext == "jpg" && e.eq_ignore_ascii_case("jpeg"))
            });
            if has_ext {
                path.with_extension("").to_string_lossy().to_string()
            } else {
                p.to_string()
            }
        }
        None => format!("{folder}/{}-{stamp}", slug(prompt)),
    };
    let ext = into
        .and_then(|p| Path::new(p).extension()?.to_str().map(str::to_string))
        .filter(|e| e.eq_ignore_ascii_case(ext) || (ext == "jpg" && e.eq_ignore_ascii_case("jpeg")))
        .unwrap_or_else(|| ext.to_string());
    if count == 1 {
        vec![format!("{stem}.{ext}")]
    } else {
        (1..=count).map(|i| format!("{stem}-{i}.{ext}")).collect()
    }
}

/// Re-encodes `path` for scroll-scrubbing (every frame a keyframe, H.264,
/// `+faststart`, no sound) with the system ffmpeg. Answers the line the
/// result carries; the original stays when there is no ffmpeg or it fails.
async fn scrub(path: &Path) -> String {
    let Ok(ffmpeg) = which::which("ffmpeg") else {
        return "Not re-encoded for scrubbing: ffmpeg is not installed on this computer, so the original file was kept. \
                For smooth scroll-scrubbing, install ffmpeg and call again with the same job, or draw it as an image \
                sequence on a canvas."
            .to_string();
    };
    let out = path.with_extension("scrub.mp4");
    let run = command::new::<tokio::process::Command>(&ffmpeg, command::Console::Hidden)
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-c:v",
            "libx264",
            "-g",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-an",
        ])
        .arg(&out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;
    match run {
        Ok(o) if o.status.success() => match std::fs::rename(&out, path) {
            Ok(()) => {
                "Re-encoded for scrubbing: every frame a keyframe, H.264, fast start, no sound."
                    .to_string()
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                format!("Not re-encoded for scrubbing ({e}); the original file was kept.")
            }
        },
        Ok(o) => {
            let _ = std::fs::remove_file(&out);
            let said = String::from_utf8_lossy(&o.stderr);
            format!(
                "Not re-encoded for scrubbing: ffmpeg stopped ({}); the original file was kept.",
                said.trim().chars().take(200).collect::<String>()
            )
        }
        Err(e) => format!(
            "Not re-encoded for scrubbing: ffmpeg could not run ({e}); the original file was kept."
        ),
    }
}

/// Where a call's files go: the app's served folder, with the label the
/// result names it by.
struct Target {
    base: PathBuf,
    label: String,
    /// The folder a file goes in when `into` is left out.
    default_folder: &'static str,
}

/// `generate_media`: the tool.
pub struct GenerateMediaTool {
    media: Media,
    store: Arc<db::Store>,
}

impl GenerateMediaTool {
    pub fn new(media: Media, store: Arc<db::Store>) -> Self {
        Self { media, store }
    }

    /// The app named, else the employee running the call when it is an app,
    /// else the workspace.
    fn target(&self, ctx: &ToolContext, input: &Value) -> Result<Target, String> {
        let app = match str_of(input, "app") {
            Some(named) => {
                let found = self
                    .store
                    .get_agent(named)
                    .ok()
                    .flatten()
                    .or_else(|| self.store.get_agent_by_name(named).ok().flatten())
                    .ok_or_else(|| format!("There is no app named {named} on this bot."))?;
                if crate::app_dev::served_dir(&found).is_none() {
                    return Err(format!(
                        "{} is not an app with a folder of its own.",
                        found.name
                    ));
                }
                Some(found)
            }
            None => {
                let me = types::keyparser::extract_agent_id(&ctx.session_key);
                self.store
                    .get_agent(&me)
                    .ok()
                    .flatten()
                    .filter(|a| crate::app_dev::served_dir(a).is_some())
            }
        };
        if let Some(app) = app {
            let dir = crate::app_dev::served_dir(&app).unwrap_or_default();
            return Ok(Target {
                base: PathBuf::from(dir),
                label: format!("{}'s folder", app.name),
                default_folder: "assets",
            });
        }
        let root =
            config::data_dir().map_err(|e| format!("The workspace could not be found: {e}"))?;
        Ok(Target {
            base: root.join("files"),
            label: "the workspace".to_string(),
            default_folder: "media",
        })
    }

    async fn image(&self, target: &Target, input: &Value) -> Result<String, String> {
        let into = str_of(input, "into");
        let (_, ext) = image_format(into, str_of(input, "output_format"));
        let body = image_body(input);
        let count = body["n"].as_u64().unwrap_or(1) as usize;
        let names = file_names(
            into,
            target.default_folder,
            str_of(input, "prompt").unwrap_or(""),
            ext,
            count,
            now(),
        );
        let paths = names
            .iter()
            .map(|n| contained(&target.base, n))
            .collect::<Result<Vec<_>, _>>()?;
        for p in &paths {
            prepare(&target.base, p)?;
        }
        let (images, model) = self.media.images(&body).await?;
        let mut lines = vec![format!(
            "Made {} image{} into {}{}:",
            images.len(),
            if images.len() == 1 { "" } else { "s" },
            target.label,
            if model.is_empty() {
                String::new()
            } else {
                format!(" with {model}")
            }
        )];
        let mut revised = None;
        for (image, (name, path)) in images.iter().zip(names.iter().zip(&paths)) {
            std::fs::write(path, &image.bytes)
                .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
            lines.push(format!(
                "- {name} ({} bytes) at {}",
                image.bytes.len(),
                path.display()
            ));
            revised = revised.or(image.revised_prompt.clone());
        }
        if let Some(r) = revised {
            lines.push(format!("Prompt as Janus used it: {r}"));
        }
        lines.push("To look at an image, use the vision helper on its path.".to_string());
        Ok(lines.join("\n"))
    }

    async fn video(&self, target: &Target, input: &Value) -> Result<String, String> {
        let into = str_of(input, "into");
        let name = file_names(
            into,
            target.default_folder,
            str_of(input, "prompt").unwrap_or(""),
            "mp4",
            1,
            now(),
        )
        .remove(0);
        let path = contained(&target.base, &name)?;
        prepare(&target.base, &path)?;
        let job = match str_of(input, "job") {
            Some(id) => Video {
                id: id.to_string(),
                model: String::new(),
                seconds: None,
            },
            None => {
                if str_of(input, "prompt").is_none() {
                    return Err("Give a `prompt` for the video.".to_string());
                }
                let frame = match str_of(input, "image") {
                    Some(img) => Some(first_frame(&target.base, img)?),
                    None => None,
                };
                self.media.submit_video(&video_body(input, frame)).await?
            }
        };
        self.media.wait_video(&job.id).await?;
        let size = self.media.download_video(&job.id, &path).await?;
        let mut lines = vec![format!(
            "Made a video into {}: {name} ({size} bytes) at {}",
            target.label,
            path.display()
        )];
        let mut about = Vec::new();
        if !job.model.is_empty() {
            about.push(format!("model {}", job.model));
        }
        if let Some(s) = job.seconds {
            about.push(format!("{s} seconds"));
        }
        about.push(format!("job {}", job.id));
        lines.push(format!("({})", about.join(", ")));
        if input.get("scrub").and_then(Value::as_bool).unwrap_or(false) {
            lines.push(scrub(&path).await);
            if let Ok(meta) = std::fs::metadata(&path) {
                lines.push(format!("Size now {} bytes.", meta.len()));
            }
        }
        Ok(lines.join("\n"))
    }
}

/// Seconds since the epoch, for default file names.
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl DynTool for GenerateMediaTool {
    fn name(&self) -> &str {
        GENERATE_MEDIA
    }

    fn description(&self) -> String {
        "Makes an image or a short video from a prompt through NeboAI and saves it as a file, billed to the owner's plan.\n\
         - kind \"image\": 1-4 images (PNG unless `into` or `output_format` says webp or jpeg).\n\
         - kind \"video\": one MP4 of 1-30 seconds; it can take minutes. `image` sets the first frame (a file in the same \
           folder, an https URL or a data URL). `scrub: true` re-encodes it for scroll-scrubbing.\n\
         - Files go into the app's folder when you are an app or name one with `app`, else the workspace. `into` is a \
           path inside that folder, such as `assets/hero.png`.\n\
         - The result gives the files' paths, never the pictures; to look at one, use the vision helper on its path.\n\
         - Leave `model` out unless the owner named one."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["image", "video"], "description": "What to make." },
                "prompt": { "type": "string", "description": "What it shows, in detail: subject, style, light, framing, motion." },
                "into": { "type": "string", "description": "The file to write, inside the app's folder or the workspace (e.g. `assets/hero.png`). Left out: a name from the prompt." },
                "app": { "type": "string", "description": "The app whose folder the file goes in. Leave out when you are the app, or for the workspace." },
                "model": { "type": "string", "description": "A Janus media model. Leave out for the default." },
                "n": { "type": "integer", "minimum": 1, "maximum": MAX_IMAGES, "description": "Image: how many (1-4)." },
                "size": { "type": "string", "description": "Image: e.g. 1024x1024, 1536x1024, 1024x1536." },
                "quality": { "type": "string", "description": "Image: low, medium, high." },
                "background": { "type": "string", "description": "Image: transparent or opaque." },
                "output_format": { "type": "string", "enum": ["png", "webp", "jpeg"], "description": "Image: file format." },
                "seconds": { "type": "integer", "minimum": 1, "maximum": MAX_SECONDS, "description": "Video: length (default 5)." },
                "resolution": { "type": "string", "description": "Video: e.g. 720p, 1080p." },
                "aspect_ratio": { "type": "string", "description": "Video: e.g. 16:9, 9:16, 1:1." },
                "image": { "type": "string", "description": "Video: the first frame. A file in the same folder, an https URL or a data URL." },
                "scrub": { "type": "boolean", "description": "Video: re-encode with every frame a keyframe for scroll-scrubbing (needs ffmpeg)." },
                "job": { "type": "string", "description": "Video: a job id from an earlier call that did not finish; picks it up instead of making a new one." }
            },
            "required": ["kind"]
        })
    }

    fn search_hint(&self) -> &str {
        "generate create image picture video art"
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let video = str_of(input, "kind") == Some("video");
        if str_of(input, "prompt").is_none() && !(video && str_of(input, "job").is_some()) {
            return Err("Give a `prompt`.".to_string());
        }
        Ok(())
    }

    fn activity(&self, input: &Value) -> String {
        match str_of(input, "kind") {
            Some("video") => "making a video".to_string(),
            _ => "making an image".to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        match str_of(input, "kind") {
            Some("video") => "Made a video".to_string(),
            _ => "Made an image".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let target = match self.target(ctx, &input) {
                Ok(t) => t,
                Err(e) => return ToolResult::error(e),
            };
            let made = match str_of(&input, "kind") {
                Some("image") => self.image(&target, &input).await,
                Some("video") => self.video(&target, &input).await,
                _ => Err("`kind` is image or video.".to_string()),
            };
            match made {
                Ok(text) => ToolResult::ok(text),
                Err(e) => ToolResult::error(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A local Janus: answers each request with the first reply whose path
    /// prefix matches, recording every request line and body.
    struct MockJanus {
        url: String,
        seen: Arc<Mutex<Vec<(String, String, Vec<(String, String)>)>>>,
    }

    type Reply = Box<dyn Fn(&str, usize) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

    async fn mock(reply: Reply) -> MockJanus {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<(String, String, Vec<(String, String)>)>>> = Arc::default();
        let seen2 = seen.clone();
        let reply = Arc::new(reply);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen2.clone();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut lines = head.lines();
                    let line = lines.next().unwrap_or("").to_string();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let len = headers
                        .iter()
                        .find(|(k, _)| k == "content-length")
                        .and_then(|(_, v)| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    while buf.len() < head_end + len {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let count = {
                        let mut s = seen.lock().unwrap();
                        s.push((line.clone(), body, headers));
                        s.iter()
                            .filter(|(l, _, _)| l.split_whitespace().nth(1) == Some(path.as_str()))
                            .count()
                    };
                    let (status, ctype, out) = reply(&path, count);
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        out.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&out).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        MockJanus { url, seen }
    }

    fn fast() -> Polling {
        Polling {
            first: Duration::from_millis(5),
            max: Duration::from_millis(20),
            limit: Duration::from_secs(5),
        }
    }

    #[test]
    fn contained_keeps_paths_inside_the_folder() {
        let base = Path::new("/apps/kart/ui");
        assert_eq!(
            contained(base, "assets/hero.png").unwrap(),
            base.join("assets/hero.png")
        );
        assert_eq!(
            contained(base, "./assets/./hero.png").unwrap(),
            base.join("assets/hero.png")
        );
        assert_eq!(
            contained(base, "/apps/kart/ui/assets/a.png").unwrap(),
            base.join("assets/a.png")
        );
        for bad in [
            "../x.png",
            "assets/../../x.png",
            "/etc/passwd",
            "/apps/kart/uix/a.png",
            "",
            ".",
            "/apps/kart/ui",
        ] {
            assert!(contained(base, bad).is_err(), "{bad} must be refused");
        }
    }

    #[cfg(unix)]
    #[test]
    fn prepare_refuses_a_link_out_of_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("ui");
        let away = dir.path().join("away");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&away).unwrap();
        std::os::unix::fs::symlink(&away, base.join("assets")).unwrap();
        let path = contained(&base, "assets/hero.png").unwrap();
        assert!(prepare(&base, &path).is_err());
        let ok = contained(&base, "img/hero.png").unwrap();
        prepare(&base, &ok).unwrap();
        assert!(base.join("img").is_dir());
    }

    #[test]
    fn bodies_carry_only_what_was_asked() {
        let body = image_body(
            &json!({"kind": "image", "prompt": "a red kart", "n": 9, "into": "assets/k.webp", "size": "1024x1024"}),
        );
        assert_eq!(body["n"], 4);
        assert_eq!(body["output_format"], "webp");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["model"], "");
        assert!(body.get("quality").is_none());

        let body = video_body(
            &json!({"prompt": "a kart drifts", "seconds": 99, "aspect_ratio": "16:9"}),
            Some("https://x/y.png".into()),
        );
        assert_eq!(body["seconds"], 30);
        assert_eq!(body["aspect_ratio"], "16:9");
        assert_eq!(body["image"], "https://x/y.png");
        assert!(body.get("resolution").is_none());
        assert_eq!(video_body(&json!({"prompt": "p"}), None)["seconds"], 5);
    }

    #[test]
    fn file_names_follow_into_or_the_prompt() {
        assert_eq!(
            file_names(Some("assets/hero.png"), "assets", "x", "png", 1, 7),
            ["assets/hero.png"]
        );
        assert_eq!(
            file_names(Some("assets/hero"), "assets", "x", "png", 2, 7),
            ["assets/hero-1.png", "assets/hero-2.png"]
        );
        assert_eq!(
            file_names(Some("a/b.jpeg"), "assets", "x", "jpg", 1, 7),
            ["a/b.jpeg"]
        );
        assert_eq!(
            file_names(Some("films/intro.mov"), "assets", "x", "mp4", 1, 7),
            ["films/intro.mov.mp4"]
        );
        assert_eq!(
            file_names(None, "media", "A red kart, at dusk!", "png", 1, 7),
            ["media/a-red-kart-at-dusk-7.png"]
        );
    }

    #[test]
    fn a_429_says_the_plan_does_not_cover_it() {
        let said = failure(
            "image",
            429,
            r#"{"error":{"message":"insufficient funds"}}"#,
            true,
        );
        assert!(
            said.contains("plan or balance does not cover this image"),
            "{said}"
        );
        let said = failure(
            "video",
            400,
            r#"{"error":{"message":"seconds too long"}}"#,
            true,
        );
        assert!(said.ends_with("seconds too long"), "{said}");
        assert_eq!(
            failure("image", 401, "", false),
            crate::janus::NOT_SIGNED_IN
        );
    }

    #[test]
    fn polling_grows_to_its_cap() {
        let p = Polling::default();
        let waits: Vec<u64> = std::iter::successors(Some(p.first), |w| Some(p.next(*w)))
            .take(5)
            .map(|d| d.as_secs())
            .collect();
        assert_eq!(waits, [2, 4, 8, 10, 10]);
    }

    #[tokio::test]
    async fn images_are_decoded_from_b64_with_janus_auth() {
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let answer = format!(
            r#"{{"created":1,"data":[{{"b64_json":"{b64}","revised_prompt":"a red kart"}},{{"b64_json":"{b64}"}}]}}"#
        );
        let answer: &'static str = Box::leak(answer.into_boxed_str());
        let janus = mock(Box::new(move |_, _| {
            (200, "application/json", answer.as_bytes().to_vec())
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None);
        let (images, _) = media
            .images(&image_body(&json!({"prompt": "a red kart", "n": 2})))
            .await
            .unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].bytes, png);
        assert_eq!(images[0].revised_prompt.as_deref(), Some("a red kart"));
        let seen = janus.seen.lock().unwrap();
        let (line, body, headers) = &seen[0];
        assert!(line.starts_with("POST /v1/images/generations"));
        assert!(body.contains("\"n\":2"));
        let header = |k: &str| headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
        assert_eq!(header("x-bot-id").as_deref(), Some("bot-1"));
        assert_eq!(header("authorization").as_deref(), Some("Bearer bot-1"));
    }

    #[tokio::test]
    async fn an_image_refused_for_funds_says_so() {
        let janus = mock(Box::new(|_, _| {
            (
                429,
                "application/json",
                br#"{"error":{"message":"no funds"}}"#.to_vec(),
            )
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None);
        let err = media.images(&json!({"prompt": "x"})).await.unwrap_err();
        assert!(err.contains("does not cover"), "{err}");
    }

    #[tokio::test]
    async fn a_video_is_submitted_waited_on_and_downloaded() {
        let janus = mock(Box::new(|path, count| match path {
            "/v1/videos" => (200, "application/json", br#"{"id":"vid_1","object":"video","status":"running","model":"nebo-video","seconds":5}"#.to_vec()),
            "/v1/videos/vid_1" if count < 3 => (200, "application/json", br#"{"id":"vid_1","status":"running"}"#.to_vec()),
            "/v1/videos/vid_1" => (200, "application/json", br#"{"id":"vid_1","status":"succeeded"}"#.to_vec()),
            "/v1/videos/vid_1/content" => (200, "video/mp4", b"MP4DATA".to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(fast());
        let job = media
            .submit_video(&video_body(&json!({"prompt": "a kart"}), None))
            .await
            .unwrap();
        assert_eq!(
            (job.id.as_str(), job.model.as_str(), job.seconds),
            ("vid_1", "nebo-video", Some(5))
        );
        media.wait_video(&job.id).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intro.mp4");
        assert_eq!(media.download_video(&job.id, &path).await.unwrap(), 7);
        assert_eq!(std::fs::read(&path).unwrap(), b"MP4DATA");
        assert!(!part_path(&path).exists());
        let polls = janus
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, _, _)| l.starts_with("GET /v1/videos/vid_1 "))
            .count();
        assert_eq!(polls, 3);
    }

    #[tokio::test]
    async fn a_failed_video_job_ends_the_wait() {
        let janus = mock(Box::new(|_, _| {
            (
                200,
                "application/json",
                br#"{"id":"v","status":"failed","error":"prompt refused"}"#.to_vec(),
            )
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(fast());
        let err = media.wait_video("v").await.unwrap_err();
        assert_eq!(err, "The video could not be made: prompt refused");
    }

    #[tokio::test]
    async fn a_video_wait_gives_up_with_the_job_id() {
        let janus = mock(Box::new(|_, _| {
            (
                200,
                "application/json",
                br#"{"id":"v","status":"running"}"#.to_vec(),
            )
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(Polling {
            first: Duration::from_millis(5),
            max: Duration::from_millis(10),
            limit: Duration::from_millis(60),
        });
        let err = media.wait_video("v").await.unwrap_err();
        assert!(err.contains("job \"v\""), "{err}");
    }

    #[tokio::test]
    async fn a_server_error_while_polling_is_polled_through() {
        let janus = mock(Box::new(|_, count| {
            if count == 1 {
                (502, "text/plain", b"bad gateway".to_vec())
            } else {
                (
                    200,
                    "application/json",
                    br#"{"id":"v","status":"succeeded"}"#.to_vec(),
                )
            }
        }))
        .await;
        let media = Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(fast());
        media.wait_video("v").await.unwrap();
    }

    #[tokio::test]
    async fn scrub_re_encodes_all_keyframe_or_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("film.mp4");
        let Ok(ffmpeg) = which::which("ffmpeg") else {
            std::fs::write(&path, b"MP4").unwrap();
            assert!(scrub(&path).await.contains("ffmpeg is not installed"));
            assert_eq!(std::fs::read(&path).unwrap(), b"MP4");
            return;
        };
        let made = std::process::Command::new(&ffmpeg)
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=10",
            ])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());
        let said = scrub(&path).await;
        assert!(said.starts_with("Re-encoded for scrubbing"), "{said}");
        assert!(path.is_file());
        assert!(!path.with_extension("scrub.mp4").exists());
    }

    #[test]
    fn a_first_frame_file_becomes_a_data_url() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/start.png"), b"png").unwrap();
        assert_eq!(
            first_frame(dir.path(), "assets/start.png").unwrap(),
            "data:image/png;base64,cG5n"
        );
        assert_eq!(
            first_frame(dir.path(), "https://x/y.png").unwrap(),
            "https://x/y.png"
        );
        assert!(first_frame(dir.path(), "../start.png").is_err());
        assert!(first_frame(dir.path(), "assets/start.gif").is_err());
    }
}
