//! `generate_media`: images, video, speech, music and sound effects made
//! through Janus and saved as files, into an app's served folder
//! (`agents.app_ui_path`) or the workspace; and transcripts of audio or
//! video, with word timings and speakers, saved as JSON (`audio::Transcript`).
//!
//! Janus answers images inline (`b64_json`), speech, music and sound as the
//! audio file's bytes (each tagged AI-generated here, `audio::tag_ai_generated`),
//! and video as a job: submit, poll until it has finished, download
//! the MP4. Made images and video are tagged AI-generated too (`tag`). A film meant for
//! scroll-scrubbing is re-encoded here with every frame a keyframe, because
//! Janus has no encoder and the bot has the file on disk anyway.
//!
//! The model only ever gets paths back, never pixels: images reach a model
//! through the vision helper alone.
//!
//! A character swap (`mode: "replace"`) puts a cast member (`cast`) in place
//! of the person in a clip. Its files are too large for a request body and
//! the providers fetch them by URL, so each local file goes up through the
//! one upload path (`POST /api/v1/files/upload`), gets a link that lasts an
//! hour (`/api/v1/shares`), and Janus is sent the URL that link opens to
//! (`/api/v1/shares/open`). The links are turned off when the job ends.

mod audio;
mod cast;
mod tag;

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
/// The speed speech is made and billed as when no `model` is named.
const SPEECH_MODEL: &str = "nebo-speech";
/// The speed transcripts are made and billed as.
const TRANSCRIBE_MODEL: &str = "nebo-transcribe";
/// The model a character swap is made with when no `model` is named.
const SWAP_MODEL: &str = "nebo-video-swap";
/// The resolution a character swap is made at when none is named.
const SWAP_RESOLUTION: &str = "720p";
/// A swap is waited on this many times as long as other video (30 minutes
/// by default): moving a person through every frame takes longer.
const SWAP_WAIT_FACTOR: u32 = 3;
/// The largest file the upload path takes (the edge's limit).
const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;
/// The most reference images one image or clip is sent; NeboAI says when a
/// model takes fewer.
const MAX_REFERENCES: usize = 4;
/// The largest reference image file sent (as a data URL).
const MAX_REFERENCE_BYTES: u64 = 8 * 1024 * 1024;
/// How long a file lent to Janus stays reachable by its link.
const LEND_FOR: chrono::TimeDelta = chrono::TimeDelta::hours(1);
/// How a wait that ran out begins (`Media::wait_video`): the job may still
/// finish, so what it was sent stays reachable.
const UNFINISHED: &str = "The video was not finished after";

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
            client: tls::http_client().user_agent(types::constants::USER_AGENT).build().expect("http client"),
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
            .map_err(|e| format!("Could not reach NeboAI to make the image: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("image", status.as_u16(), &text, signed_in));
        }
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| format!("NeboAI's answer for the image could not be read: {e}"))?;
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
                .map_err(|e| format!("NeboAI's image could not be decoded: {e}"))?;
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
            return Err("NeboAI answered with no image.".to_string());
        }
        Ok((out, model))
    }

    /// The audio file for `body`, posted to `path` (`/v1/audio/speech`, or
    /// `/v1/audio/generations` for music and sound): Janus answers the
    /// file's bytes and their content type. `what` names it in errors.
    pub async fn audio(&self, path: &str, what: &str, body: &Value) -> Result<(Vec<u8>, String), String> {
        let (req, signed_in) = self.request(reqwest::Method::POST, path);
        let resp = req
            .json(body)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| format!("Could not reach NeboAI to make the {what}: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure(what, status.as_u16(), &text, signed_in));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("The {what} from NeboAI stopped part way: {e}"))?;
        if bytes.is_empty() {
            return Err("NeboAI answered with no audio.".to_string());
        }
        Ok((bytes.to_vec(), content_type))
    }

    /// The voices speech can use (`/v1/audio/voices`): each by the id
    /// `voice` takes, with its name and description.
    pub async fn voices(&self) -> Result<Vec<Value>, String> {
        let (req, signed_in) = self.request(reqwest::Method::GET, "/v1/audio/voices");
        let resp = req
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Could not reach NeboAI for the voices: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("voice list", status.as_u16(), &text, signed_in));
        }
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| format!("NeboAI's voice list could not be read: {e}"))?;
        Ok(parsed.get("data").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    /// Transcribes an audio or video file (`/v1/audio/transcriptions`, the
    /// transcription speed, `verbose_json` with word timings): Janus's
    /// answer, words and speakers included when the audio has them.
    pub async fn transcribe(&self, filename: &str, bytes: Vec<u8>, language: Option<&str>) -> Result<Value, String> {
        let (req, signed_in) = self.request(reqwest::Method::POST, "/v1/audio/transcriptions");
        let mut form = reqwest::multipart::Form::new()
            .text("model", TRANSCRIBE_MODEL)
            .text("response_format", "verbose_json")
            .text("timestamp_granularities[]", "word");
        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }
        let form = form.part("file", reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string()));
        let resp = req
            .multipart(form)
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| format!("Could not reach NeboAI to transcribe {filename}: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("transcript", status.as_u16(), &text, signed_in));
        }
        resp.json()
            .await
            .map_err(|e| format!("NeboAI's transcript could not be read: {e}"))
    }

    /// Submits a video job for `body` (a `/v1/videos` request).
    pub async fn submit_video(&self, body: &Value) -> Result<Video, String> {
        let (req, signed_in) = self.request(reqwest::Method::POST, "/v1/videos");
        let resp = req
            .json(body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("Could not reach NeboAI to make the video: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(failure("video", status.as_u16(), &text, signed_in));
        }
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| format!("NeboAI's answer for the video could not be read: {e}"))?;
        let id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("NeboAI answered the video request with no job id.")?
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
    /// it has succeeded, failed, or `limit` has passed. A poll that does
    /// not get an answer is retried; a refusal ends the wait.
    pub async fn wait_video(&self, id: &str, limit: Duration) -> Result<(), String> {
        let started = tokio::time::Instant::now();
        let mut wait = self.polling.first;
        loop {
            if started.elapsed() + wait > limit {
                return Err(format!(
                    "{UNFINISHED} {} minutes. Its job id is {id}: call generate_media again with \
                     kind \"video\" and job \"{id}\" to pick it up instead of making a new one.",
                    limit.as_secs().div_ceil(60)
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
            .map_err(|e| format!("Could not download the video from NeboAI: {e}"))?;
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
                        "The video download from NeboAI stopped part way: {e}"
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
/// covering the work; the owner reads it, so it says what to do. Only a
/// category, or NeboAI's own words about the request (`seconds must be
/// between 1 and 30`), reach the model: a refusal passed on from the
/// service behind NeboAI can name that service or its model, and the owner
/// never sees those (owner rule 2026-10-05). Its words stay in the log.
fn failure(kind: &str, status: u16, body: &str, signed_in: bool) -> String {
    let code = janus_code(body);
    match status {
        429 if code == "provider_rate_limit" => format!(
            "NeboAI is busy making {kind}s right now. Wait a minute and try once more; if it is still busy, tell the \
             owner plainly."
        ),
        429 | 402 => format!(
            "The owner's NeboAI plan is used up this month and does not cover this {kind}. Tell the owner plainly; they \
             can upgrade their plan under Settings > Account, then ask again."
        ),
        401 | 403 if !signed_in => crate::janus::NOT_SIGNED_IN.to_string(),
        // No model serves this kind yet (Janus's `kind_unavailable`): a
        // retry cannot help. Live 2026-10-07 the model answered this by
        // sending the owner to outside music services by name, and offered
        // sound effects that had failed the same way minutes before.
        503 if code == "kind_unavailable" => format!(
            "This kind of media ({kind}) is not available on NeboAI yet. Tell the owner that in one plain sentence. Do not \
             name or recommend other apps, websites or services for it, and do not say another kind of media works unless \
             a call for it succeeded in this task. Do not try again in this task."
        ),
        _ => {
            let said = janus_message(body);
            tracing::warn!(kind, status, code = %code, said = %said, "NeboAI did not make the media");
            let request = (400..500).contains(&status);
            // NeboAI's own check of the request: its words say what to fix.
            let own = request && !code.starts_with("upstream") && !code.starts_with("provider");
            if own && !said.is_empty() && !names_a_provider(&said) {
                format!("NeboAI did not make the {kind}: {said}")
            } else if request {
                format!(
                    "NeboAI did not make the {kind}: the request was refused. Change the prompt or the settings and try \
                     once more; if it is refused again, tell the owner plainly."
                )
            } else {
                format!(
                    "NeboAI could not make the {kind} just now (a service error). Try once more; if it fails again, tell \
                     the owner plainly."
                )
            }
        }
    }
}

/// Words that name a company or model behind NeboAI's media. Text that
/// holds one is never passed on: the category is said instead.
const PROVIDER_WORDS: &[&str] = &[
    "openai", "gpt", "dall-e", "dalle", "sora", "whisper", "xai", "grok", "aurora", "google", "gemini", "imagen", "veo",
    "lyria", "vertex", "deepmind", "anthropic", "claude", "elevenlabs", "fal", "runway", "kling", "luma", "pika",
    "minimax", "hailuo", "seedance", "bytedance", "alibaba", "wan", "qwen", "stability", "flux", "midjourney", "suno",
    "udio", "replicate", "openrouter", "cartesia", "deepgram", "azure", "bedrock", "mistral",
];

/// Whether `text` names a company or model behind NeboAI's media: one of
/// [`PROVIDER_WORDS`] as a word, or followed by a version (`veo3`,
/// `gpt-image-1`).
fn names_a_provider(text: &str) -> bool {
    text.to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|word| {
            PROVIDER_WORDS.iter().any(|p| {
                word.strip_prefix(p)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| c == '-' || c.is_ascii_digit()))
            })
        })
}

/// The code in a Janus error body (`{"error": {"code"}}`), or "".
fn janus_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/code").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
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

/// What a failed video job says: NeboAI's sentence for it, never words
/// that name the service behind it (`failure`).
fn job_failed(job: &Value) -> String {
    let said = job
        .pointer("/error/message")
        .or_else(|| job.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if said.is_empty() {
        "The video could not be made.".to_string()
    } else if names_a_provider(said) {
        tracing::warn!(said, "a video job failed");
        "The video could not be made: it was refused. Change the prompt and try once more.".to_string()
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

/// A relative path that leads out of `base` (by `..` or a link). Absolute
/// and `~/` paths never come here (`written_at`, `readable`), so the way out
/// it names is one that is taken. Live 2026-10-07: "is outside …/Nebo/files.
/// Give a path inside that folder… or an absolute or `~/` path" refused the
/// absolute path it offered, and the model copied a whole project into
/// Nebo/files instead.
fn outside(into: &str, base: &Path) -> String {
    format!(
        "`{into}` leads out of {}, where relative paths stay. Name the file by its absolute or `~/` path instead \
         (e.g. `~/NeboAI/Media/Projects/lighthouse/frames/s1-start.png`); never copy files into this folder to reach them.",
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

/// The file a call writes for `name` (its `into`, or the default name), with
/// its folder made: an absolute or `~/` path is written where it says, as
/// `write_file` writes it (the safeguard and the job's folders check it
/// before the call runs, `safeguard::media_write`); a relative one is inside
/// `base`, and links may not lead out of it. Live 2026-10-03: `into:
/// "NeboAI/Media/outputs/vo.mp3"` meant the owner's ~/NeboAI and landed in
/// the workspace, so the next command could not find it; the result now
/// names the absolute path, and `~/NeboAI/...` goes there.
fn destination(base: &Path, name: &str) -> Result<PathBuf, String> {
    let path = written_at(base, name)?;
    if path.starts_with(base) {
        prepare(base, &path)?;
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Could not make {}: {e}", parent.display()))?;
    }
    // The result names the file by this path, so the next command finds it.
    Ok(std::path::absolute(&path).unwrap_or(path))
}

/// Where `name` resolves (`destination`), nothing made.
fn written_at(base: &Path, name: &str) -> Result<PathBuf, String> {
    let named = types::pathres::expand(name.trim());
    if !named.is_absolute() {
        return contained(base, name);
    }
    if named.file_name().is_none() || named.is_dir() {
        return Err(format!("`{name}` is a folder; `into` must name a file."));
    }
    Ok(named)
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

/// The speech file format for a call: `into`'s extension when it names one,
/// else `output_format`, else MP3. Answers Janus's `response_format`, which
/// is also the file extension.
fn speech_format(into: Option<&str>, output_format: Option<&str>) -> &'static str {
    let from = |s: &str| match s.to_ascii_lowercase().as_str() {
        "mp3" => Some("mp3"),
        "wav" => Some("wav"),
        _ => None,
    };
    into.and_then(|p| Path::new(p).extension()?.to_str().and_then(from))
        .or_else(|| output_format.and_then(from))
        .unwrap_or("mp3")
}

/// The `/v1/audio/speech` body for a call: the words, the format, the
/// voice only when one was named (Janus has its own default), and the
/// delivery (`direction`: tone, pace, emotion) sent as the model's
/// `instructions`.
pub fn speech_body(input: &Value) -> Value {
    let mut body = json!({
        "model": str_of(input, "model").unwrap_or(SPEECH_MODEL),
        "input": str_of(input, "text").unwrap_or(""),
        "response_format": speech_format(str_of(input, "into"), str_of(input, "output_format")),
    });
    if let Some(voice) = str_of(input, "voice") {
        body["voice"] = json!(voice.to_ascii_lowercase());
    }
    if let Some(direction) = str_of(input, "direction") {
        body["instructions"] = json!(direction);
    }
    body
}

/// How long an MP3 or WAV file plays, read from its own headers: a WAV's
/// data size over its byte rate, an MP3's frames walked and their samples
/// counted. `None` when the bytes are not one this can read.
fn audio_seconds(bytes: &[u8]) -> Option<f64> {
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        return wav_seconds(bytes);
    }
    mp3_seconds(bytes)
}

fn wav_seconds(bytes: &[u8]) -> Option<f64> {
    let u32_at = |i: usize| Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
    let mut at = 12;
    let mut byte_rate = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        match id {
            b"fmt " => byte_rate = u32_at(at + 16),
            b"data" => {
                let rate = byte_rate.filter(|r| *r > 0)?;
                // A streamed WAV can name a size larger than what is there.
                let size = size.min(bytes.len() - (at + 8));
                return Some(size as f64 / rate as f64);
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}

fn mp3_seconds(bytes: &[u8]) -> Option<f64> {
    let mut at = 0;
    // An ID3v2 tag first: ten bytes of header, then a syncsafe size.
    if bytes.starts_with(b"ID3") && bytes.len() >= 10 {
        let size = bytes[6..10].iter().fold(0usize, |n, b| (n << 7) | (*b as usize & 0x7f));
        at = 10 + size;
    }
    let (mut seconds, mut frames) = (0f64, 0u32);
    while at + 4 <= bytes.len() {
        let h = &bytes[at..at + 4];
        if h[0] != 0xff || h[1] & 0xe0 != 0xe0 {
            if frames > 0 {
                break; // a trailing tag
            }
            at += 1;
            continue;
        }
        let version = (h[1] >> 3) & 3; // 3 = MPEG 1, 2 = MPEG 2, 0 = MPEG 2.5
        let layer = (h[1] >> 1) & 3; // 1 = Layer III
        let bitrate_index = (h[2] >> 4) as usize;
        let rate_index = ((h[2] >> 2) & 3) as usize;
        if version == 1 || layer != 1 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
            if frames > 0 {
                break;
            }
            at += 1;
            continue;
        }
        const V1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
        const V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        let mpeg1 = version == 3;
        let kbps = if mpeg1 { V1 } else { V2 }[bitrate_index];
        let rate = [44100u32, 48000, 32000][rate_index] >> match version {
            3 => 0,
            2 => 1,
            _ => 2,
        };
        let samples = if mpeg1 { 1152 } else { 576 };
        let padding = ((h[2] >> 1) & 1) as usize;
        let len = (samples / 8 * kbps * 1000 / rate) as usize + padding;
        if len < 4 {
            return None;
        }
        seconds += samples as f64 / rate as f64;
        frames += 1;
        at += len;
    }
    (frames > 0).then_some(seconds)
}

/// The `/v1/videos` body for a call. `first_frame` is the start image,
/// already a URL or data URL.
pub fn video_body(input: &Value, first_frame: Option<String>) -> Value {
    let mut body = json!({
        "model": str_of(input, "model").unwrap_or(""),
        "prompt": str_of(input, "prompt").unwrap_or(""),
        // Janus makes whole seconds of video: a fraction is rounded up, so
        // the clip is never shorter than asked.
        "seconds": input.get("seconds").and_then(Value::as_f64).map(|s| s.ceil() as u64).unwrap_or(5).clamp(1, MAX_SECONDS),
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

/// The `/v1/videos` body for a character swap: the clip and the cast
/// member's images as URLs Janus can fetch, the owner's attestation, and
/// the prompt only when one was given.
pub fn swap_body(input: &Value, video: &str, references: &[String], attestation: &str) -> Value {
    let mut body = json!({
        "model": str_of(input, "model").unwrap_or(SWAP_MODEL),
        "mode": "replace",
        "video": video,
        "references": references,
        "resolution": str_of(input, "resolution").unwrap_or(SWAP_RESOLUTION),
        "attestation": attestation,
    });
    if let Some(prompt) = str_of(input, "prompt") {
        body["prompt"] = json!(prompt);
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
/// file read as `readable` reads it (absolute, `~/`, or inside `base`), sent
/// as a data URL. Live 2026-10-07: frames `generate_media` had just written
/// to `~/NeboAI/Media/...` were refused as the next clip's start frame.
fn first_frame(ctx: &ToolContext, base: &Path, image: &str) -> Result<String, String> {
    if image.starts_with("https://") || image.starts_with("data:") {
        return Ok(image.to_string());
    }
    let path = readable(ctx, base, image)?;
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

/// The reference images a call sends (`references`): pictures whose subjects
/// the image or clip keeps, such as a character's cast portrait. Each is a
/// file read as `readable` reads it (absolute, `~/`, or inside `base`) and
/// sent as a data URL, or a data URL as given. Empty when none were given.
fn references(ctx: &ToolContext, base: &Path, input: &Value) -> Result<Vec<String>, String> {
    let Some(given) = input.get("references") else {
        return Ok(Vec::new());
    };
    let given: Vec<&str> = match given {
        Value::Null => return Ok(Vec::new()),
        Value::String(one) => vec![one.as_str()],
        Value::Array(many) => many
            .iter()
            .map(|v| v.as_str().ok_or("`references` is a list of picture files."))
            .collect::<Result<_, _>>()?,
        _ => return Err("`references` is a list of picture files.".to_string()),
    };
    let given: Vec<&str> = given.into_iter().map(str::trim).filter(|g| !g.is_empty()).collect();
    if given.len() > MAX_REFERENCES {
        return Err(format!("`references` takes up to {MAX_REFERENCES} pictures; got {}.", given.len()));
    }
    given
        .into_iter()
        .map(|g| {
            if g.starts_with("data:image/") {
                return Ok(g.to_string());
            }
            if g.starts_with("https://") || g.starts_with("http://") {
                return Err(format!("`references` are picture files; save `{g}` to a file first and give its path."));
            }
            let path = readable(ctx, base, g)?;
            let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("png") => "image/png",
                Some("webp") => "image/webp",
                Some("jpg" | "jpeg") => "image/jpeg",
                _ => return Err(format!("Each of `references` must be a PNG, WebP or JPEG file; got `{g}`.")),
            };
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size > MAX_REFERENCE_BYTES {
                return Err(format!(
                    "{} is {} MB; a reference picture takes up to {} MB. Save a smaller copy (2048 px on the long side is plenty).",
                    path.display(),
                    size.div_ceil(1024 * 1024),
                    MAX_REFERENCE_BYTES / (1024 * 1024)
                ));
            }
            let bytes = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
            Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
        })
        .collect()
}

/// What a refusal of reference images becomes while NeboAI cannot take
/// them yet: one that predates them refuses the request's size (images) or
/// its mode (video). Any other failure is passed on as it is.
fn without_references(e: String) -> String {
    if e.contains("request body too large") || e.contains("mode must be generate or replace") {
        "Reference images aren't available on NeboAI yet. Make it without `references`: describe the character in          the same words as before, and start each clip from an approved start frame. Don't try `references` again in          this task."
            .to_string()
    } else {
        e
    }
}

/// A file this call reads, named by `given`: an absolute or `~/` path as it
/// is, a relative one inside `base`. It meets the limits and folders a
/// `read_file` of it meets, so nothing the run may not read is sent out.
fn readable(ctx: &ToolContext, base: &Path, given: &str) -> Result<PathBuf, String> {
    let named = types::pathres::expand(given.trim());
    let path = if named.is_absolute() { named } else { contained(base, given)? };
    let shown = path.to_string_lossy().into_owned();
    let refused = crate::safeguard::check_safeguard("read_file", &json!({ "path": shown }), ctx)
        .or_else(|| ctx.outside_folders("read", std::slice::from_ref(&shown)));
    if let Some(refused) = refused {
        return Err(refused);
    }
    if !path.is_file() {
        return Err(format!("There is no file at {shown}."));
    }
    Ok(path)
}

/// The clip a swap is sent: an https URL as given, else a local video file
/// the upload path takes. Checked before anything is uploaded.
enum Clip {
    Url(String),
    File { path: PathBuf, mime: &'static str },
}

fn clip(ctx: &ToolContext, base: &Path, given: &str) -> Result<Clip, String> {
    if given.starts_with("https://") {
        return Ok(Clip::Url(given.to_string()));
    }
    let path = readable(ctx, base, given)?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        _ => return Err(format!("`video` must be an MP4, MOV or WebM file or an https URL; got `{given}`.")),
    };
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_UPLOAD_BYTES {
        return Err(format!(
            "{} is {} MB; a swap takes up to {} MB. Trim it to 30 seconds or less and conform it to 24 fps and 720p \
             with the Nebo Media plugin first.",
            path.display(),
            size.div_ceil(1024 * 1024),
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ));
    }
    Ok(Clip::File { path, mime })
}

/// The MIME type of a cast image, by its extension.
fn image_mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Files lent to Janus for one job: each stored through the one upload
/// path and opened by a link that lasts [`LEND_FOR`]. Every link made is
/// remembered so it can be turned off when the job ends.
struct Lent {
    hub: Arc<comm::api::NeboAIApi>,
    shares: Vec<String>,
}

/// Why a link could not be made.
enum LinkError {
    /// The hub no longer has the file (a stored copy that is gone).
    Gone,
    Other(String),
}

impl Lent {
    /// Stores `path` through the one upload path; the hub's file id.
    async fn upload(&self, path: &Path, name: &str, mime: &str) -> Result<String, String> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        self.hub
            .upload_file(name, mime, data, &[])
            .await
            .map(|a| a.file_id)
            .map_err(|e| format!("Could not upload {} to NeboAI: {e}", path.display()))
    }

    /// A URL Janus can fetch file `file_id` from for the next hour: a link
    /// share, opened the way its page opens it.
    async fn link(&mut self, file_id: &str) -> Result<String, LinkError> {
        let settings = comm::api_types::FileShareSettings {
            access: "link".to_string(),
            expires_at: (chrono::Utc::now() + LEND_FOR).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ..Default::default()
        };
        let share = match self.hub.create_file_share(file_id, "", &settings).await {
            Ok(s) => s,
            Err(comm::CommError::Http { status: 404, .. }) => return Err(LinkError::Gone),
            Err(e) => return Err(LinkError::Other(format!("Could not make a link for NeboAI: {e}"))),
        };
        self.shares.push(share.id.clone());
        let token = share.url.rsplit('/').next().unwrap_or_default();
        let opened = self
            .hub
            .open_file_share(token)
            .await
            .map_err(|e| LinkError::Other(format!("Could not open the link for NeboAI: {e}")))?;
        if opened.state != "ok" || opened.file_url.is_empty() {
            return Err(LinkError::Other(format!("The link for NeboAI did not open ({}).", opened.state)));
        }
        Ok(if opened.file_url.starts_with("https://") || opened.file_url.starts_with("http://") {
            opened.file_url
        } else {
            format!("{}{}", self.hub.api_server().trim_end_matches('/'), opened.file_url)
        })
    }

    /// Uploads `path` and links it.
    async fn send(&mut self, path: &Path, name: &str, mime: &str) -> Result<(String, String), String> {
        let id = self.upload(path, name, mime).await?;
        match self.link(&id).await {
            Ok(url) => Ok((id, url)),
            Err(LinkError::Gone) => Err("NeboAI lost the file just uploaded. Try again.".to_string()),
            Err(LinkError::Other(e)) => Err(e),
        }
    }

    /// Turns off every link this job made.
    async fn revoke(&self) {
        for id in &self.shares {
            if let Err(e) = self.hub.revoke_file_share(id).await {
                tracing::warn!(share = %id, error = %e, "could not turn off a link lent to Janus; it ends on its own within the hour");
            }
        }
    }
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

impl Target {
    /// The folder the result says `path` went into: this target's label
    /// when it is inside it, else the folder `into` named.
    fn place(&self, path: &Path) -> String {
        if path.starts_with(&self.base) {
            return self.label.clone();
        }
        path.parent().map(|p| p.display().to_string()).unwrap_or_default()
    }
}

/// What a request says when it wants new media made (`DynTool::triggers`):
/// a request that says one has the tool loaded on its first step. Live
/// 2026-10-03: "Record a one-sentence voiceover" left it deferred. Live
/// 2026-10-04: asked whether we could make "AI creators" with the same face
/// across clips from reference photos, the employee answered from memory
/// with no tool in view and never named the cast or the swap. The matcher
/// allows a plural, so "ai creator" also says "AI creators".
const TRIGGERS: &[&str] = &[
    "voiceover",
    "voice over",
    "background music",
    "soundtrack",
    "jingle",
    "make music",
    "generate music",
    "sound effect",
    "transcribe",
    "transcript",
    "word timings",
    "narration",
    "text to speech",
    "read this aloud",
    "read it aloud",
    "generate an image",
    "generate a picture",
    "generate a video",
    "make an image",
    "make a picture",
    "create an image",
    "create a picture",
    "ai image",
    "ai video",
    "character swap",
    "swap the person",
    "replace the actor",
    "replace the person",
    "cast member",
    "same face",
    "consistent character",
    "ai character",
    "ai influencer",
    "ai creator",
    "ai persona",
    "reference photo",
    "image to video",
];

/// `generate_media`: the tool.
pub struct GenerateMediaTool {
    media: Media,
    store: Arc<db::Store>,
    triggers: Vec<String>,
    /// Tests only: the cast folder and the hub, in place of
    /// `<data_dir>/files/cast` and this bot's NeboAI connection.
    cast_root: Option<PathBuf>,
    hub: Option<Arc<comm::api::NeboAIApi>>,
}

impl GenerateMediaTool {
    pub fn new(media: Media, store: Arc<db::Store>) -> Self {
        Self {
            media,
            store,
            triggers: TRIGGERS.iter().map(|t| t.to_string()).collect(),
            cast_root: None,
            hub: None,
        }
    }

    #[cfg(test)]
    fn with_cast_and_hub(mut self, cast_root: PathBuf, hub: Arc<comm::api::NeboAIApi>) -> Self {
        self.cast_root = Some(cast_root);
        self.hub = Some(hub);
        self
    }

    /// The cast every employee on this bot shares: `<data_dir>/files/cast`.
    fn cast(&self) -> Result<cast::Cast, String> {
        let root = match &self.cast_root {
            Some(r) => r.clone(),
            None => config::data_dir()
                .map_err(|e| format!("The workspace could not be found: {e}"))?
                .join("files")
                .join("cast"),
        };
        Ok(cast::Cast { root })
    }

    /// This bot's NeboAI connection, for lending files to Janus.
    fn hub(&self) -> Result<Arc<comm::api::NeboAIApi>, String> {
        match &self.hub {
            Some(h) => Ok(h.clone()),
            None => crate::build_neboai_api(&self.store).map(Arc::new),
        }
    }

    /// The app named, else the employee running the call when it is an app,
    /// else the run's working folder when it has one (an isolated helper's
    /// copy, where the file tools take relative paths from), else the
    /// workspace. Only ever one of the owner's own apps: one
    /// installed from the marketplace is its maker's, and nothing is written
    /// into its folder (the developer pack's rule, `app_dev::is_own_app`).
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
                if !crate::app_dev::is_own_app(&found) {
                    return Err(crate::app_dev::not_yours(&found));
                }
                Some(found)
            }
            None => {
                let me = types::keyparser::extract_agent_id(&ctx.session_key);
                self.store
                    .get_agent(&me)
                    .ok()
                    .flatten()
                    .filter(|a| crate::app_dev::served_dir(a).is_some() && crate::app_dev::is_own_app(a))
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
        if let Some(cwd) = ctx.cwd.as_deref().filter(|c| Path::new(c).is_absolute()) {
            return Ok(Target {
                base: PathBuf::from(cwd),
                label: "the working folder".to_string(),
                default_folder: "media",
            });
        }
        let root =
            config::workspace_dir().map_err(|e| format!("The workspace could not be found: {e}"))?;
        Ok(Target {
            base: root,
            label: "the workspace".to_string(),
            // `Media`, as Nebo Media writes it: one folder on a case-sensitive disk too.
            default_folder: "Media",
        })
    }

    /// Makes the images and answers the result text and every file, each
    /// of which the chat shows as a card.
    async fn image(&self, ctx: &ToolContext, target: &Target, input: &Value) -> Result<(String, Vec<PathBuf>), String> {
        let into = str_of(input, "into");
        let (_, ext) = image_format(into, str_of(input, "output_format"));
        let refs = references(ctx, &target.base, input)?;
        let mut body = image_body(input);
        if !refs.is_empty() {
            body["references"] = json!(refs);
        }
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
            .map(|n| destination(&target.base, n))
            .collect::<Result<Vec<_>, _>>()?;
        let (images, model) = match self.media.images(&body).await {
            Ok(made) => made,
            Err(e) if !refs.is_empty() => return Err(without_references(e)),
            Err(e) => return Err(e),
        };
        // The model stays in the log: a result line can reach the owner,
        // who never sees what made it.
        tracing::info!(model = %model, count = images.len(), references = refs.len(), "made images");
        let mut lines = vec![format!(
            "Made {} image{}{} into {}:",
            images.len(),
            if images.len() == 1 { "" } else { "s" },
            match refs.len() {
                0 => String::new(),
                1 => " from 1 reference picture".to_string(),
                n => format!(" from {n} reference pictures"),
            },
            paths.first().map(|p| target.place(p)).unwrap_or_default(),
        )];
        let mut revised = None;
        let mut written = Vec::new();
        for (image, (name, path)) in images.into_iter().zip(names.iter().zip(&paths)) {
            // The bytes decide the extension: a model can answer JPEG for a
            // PNG request, and a `.png` holding a JPEG is a broken file.
            let asked = path.extension().and_then(|e| e.to_str()).unwrap_or(ext);
            let real = real_image_ext(&image.bytes, asked);
            let (name, path) = if real == asked {
                (name.clone(), path.clone())
            } else {
                (
                    Path::new(name)
                        .with_extension(real)
                        .to_string_lossy()
                        .into_owned(),
                    path.with_extension(real),
                )
            };
            let (bytes, tagged) = tag::image(image.bytes);
            std::fs::write(&path, &bytes)
                .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
            let mut line = format!(
                "- {name} ({} bytes{}) at {}",
                bytes.len(),
                if tagged { ", tagged AI-generated" } else { "" },
                path.display()
            );
            if real != asked {
                line.push_str(&format!(
                    " (the image came back as {}, not {}, so it is saved as .{real})",
                    real.to_ascii_uppercase(),
                    asked.to_ascii_uppercase()
                ));
            }
            lines.push(line);
            written.push(path);
            revised = revised.or(image.revised_prompt);
        }
        if let Some(r) = revised {
            lines.push(format!("Prompt as it was used: {r}"));
        }
        lines.push("To look at an image, use the vision helper on its path.".to_string());
        if written.is_empty() {
            return Err("NeboAI answered with no image.".to_string());
        }
        Ok((lines.join("\n"), written))
    }

    /// Makes speech, music or a sound effect (`kind`) and answers the
    /// result text and its file, which the chat shows as a card with a
    /// player. The file is tagged AI-generated in its own metadata.
    async fn audio_file(&self, target: &Target, input: &Value, kind: &str) -> Result<(String, Vec<PathBuf>), String> {
        let asked = speech_format(str_of(input, "into"), str_of(input, "output_format"));
        let (path_on_janus, what, body, named_for) = match kind {
            "speech" => {
                let text = str_of(input, "text").ok_or("Give the `text` to speak.")?;
                ("/v1/audio/speech", "speech", speech_body(input), text)
            }
            _ => {
                let prompt = str_of(input, "prompt").ok_or("Give a `prompt`.")?;
                let what = if kind == "music" { "music" } else { "sound effect" };
                ("/v1/audio/generations", what, audio::generation_body(kind, input, asked), prompt)
            }
        };
        let name = file_names(str_of(input, "into"), target.default_folder, named_for, asked, 1, now()).remove(0);
        let path = destination(&target.base, &name)?;
        let (bytes, content_type) = self.media.audio(path_on_janus, what, &body).await?;
        // The bytes decide the extension: a model can answer WAV for an MP3
        // request, and an `.mp3` holding a WAV is a broken file.
        let real = audio::audio_ext(&bytes, &content_type, asked);
        let (name, path) = if real == asked {
            (name, path)
        } else {
            (Path::new(&name).with_extension(real).to_string_lossy().into_owned(), path.with_extension(real))
        };
        let (bytes, tagged) = audio::tag_ai_generated(bytes);
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        let mut about = Vec::new();
        if let Some(s) = audio_seconds(&bytes) {
            about.push(format!("{s:.1} seconds"));
        }
        about.push(format!("{} bytes", bytes.len()));
        if let Some(voice) = body.get("voice").and_then(Value::as_str) {
            about.push(format!("voice {voice}"));
        }
        if body.get("instrumental").and_then(Value::as_bool) == Some(true) {
            about.push("instrumental".to_string());
        }
        if tagged {
            about.push("tagged AI-generated".to_string());
        }
        let mut lines = vec![format!(
            "Made {what} into {}: {name} ({}) at {}",
            target.place(&path),
            about.join(", "),
            path.display()
        )];
        if real != asked {
            lines.push(format!("(It came back as {}, so it is saved as .{real}.)", real.to_ascii_uppercase()));
        }
        lines.push(match kind {
            "speech" => "Speech is for an off-screen narrator or voice-over only, never a person seen speaking: an on-camera \
                         line is made inside its video clip (put the line in the video prompt and keep the clip's sound). \
                         To lay this voice-over under a video, use an installed media plugin's audio mix on this file and \
                         the video."
                .to_string(),
            _ => "To trim it, fade it or mix it under a video or voice-over, use an installed media plugin's audio \
                  commands (the Nebo Media plugin has them)."
                .to_string(),
        });
        Ok((lines.join("\n"), vec![path]))
    }

    /// kind "voices": the voices speech can use, one line each.
    async fn voice_list(&self) -> Result<String, String> {
        let voices = self.media.voices().await?;
        if voices.is_empty() {
            return Ok("No voices are listed right now; leave `voice` out for the default.".to_string());
        }
        let mut lines = vec![format!(
            "{} voices. Give one's id as `voice` with kind \"speech\" (an off-screen narrator or voice-over); ids never change. \
             Cast by the description: a woman's voice for a woman, a man's for a man, and a different voice for each \
             character:",
            voices.len()
        )];
        for v in &voices {
            let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
            let Some(id) = field("id") else { continue };
            let mut line = format!("- {id}");
            if let Some(name) = field("name").filter(|n| *n != id) {
                line.push_str(&format!(": {name}"));
            }
            if let Some(d) = field("description") {
                line.push_str(&format!(" - {d}"));
            }
            let traits: Vec<&str> = [field("gender"), field("language")].into_iter().flatten().collect();
            if !traits.is_empty() {
                line.push_str(&format!(" ({})", traits.join(", ")));
            }
            lines.push(line);
        }
        Ok(lines.join("\n"))
    }

    /// kind "transcript": the audio or video file `file`, transcribed with
    /// word timings and speakers, saved as transcript JSON (`into`, else
    /// `<file>.transcript.json` beside it when that may be written, else
    /// in the default folder).
    async fn transcript(&self, ctx: &ToolContext, target: &Target, input: &Value) -> Result<(String, Vec<PathBuf>), String> {
        let given = str_of(input, "file").ok_or("Give `file`: the audio or video file to transcribe.")?;
        let source = readable(ctx, &target.base, given)?;
        let filename = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = source.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !ai::transcribe::SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!(
                "{filename} cannot be transcribed as it is: send {}. Extract its audio to MP3 or M4A with the Nebo Media \
                 plugin first, then transcribe that file.",
                ai::transcribe::SUPPORTED_AUDIO_EXTENSIONS.join(", ")
            ));
        }
        let size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        if size as usize > ai::transcribe::MAX_AUDIO_BYTES {
            return Err(format!(
                "{filename} is {} MB; a transcript takes up to {} MB. Extract its audio as a compressed MP3 or M4A with the \
                 Nebo Media plugin (or split it), then transcribe that.",
                size.div_ceil(1024 * 1024),
                ai::transcribe::MAX_AUDIO_BYTES / (1024 * 1024)
            ));
        }
        let path = match str_of(input, "into") {
            Some(into) => destination(&target.base, into)?,
            None => {
                let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "audio".into());
                let beside = source.with_file_name(format!("{stem}.transcript.json"));
                let shown = beside.to_string_lossy().into_owned();
                let refused = crate::safeguard::check_safeguard("write_file", &json!({ "path": shown }), ctx)
                    .or_else(|| ctx.outside_folders("write", std::slice::from_ref(&shown)));
                if refused.is_none() {
                    beside
                } else {
                    destination(&target.base, &format!("{}/{stem}.transcript.json", target.default_folder))?
                }
            }
        };
        let bytes = std::fs::read(&source).map_err(|e| format!("Could not read {}: {e}", source.display()))?;
        let raw = self.media.transcribe(&filename, bytes, str_of(input, "language")).await?;
        let t = audio::transcript(&raw);
        let json = serde_json::to_string_pretty(&t).map_err(|e| format!("The transcript could not be written: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        let speakers: std::collections::BTreeSet<&str> =
            t.words.iter().chain(&t.segments).filter_map(|p| p.speaker.as_deref()).collect();
        let mut about = vec![format!("{} words", t.words.len()), format!("{} segments", t.segments.len())];
        if !speakers.is_empty() {
            about.push(format!("{} speakers", speakers.len()));
        }
        if t.duration > 0.0 {
            about.push(format!("{:.1} seconds", t.duration));
        }
        if !t.language.is_empty() {
            about.push(format!("language {}", t.language));
        }
        let mut lines = vec![format!(
            "Transcribed {} into {} ({}).",
            source.display(),
            path.display(),
            about.join(", ")
        )];
        if t.words.is_empty() && !t.segments.is_empty() {
            lines.push("No word timings came back for this file; the segments carry the timing.".to_string());
        }
        let text: String = t.segments.iter().map(|s| match &s.speaker {
            Some(sp) => format!("[{sp}] {}", s.text),
            None => s.text.clone(),
        }).collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            lines.push("No speech was heard in it.".to_string());
        } else {
            let preview: String = text.chars().take(600).collect();
            lines.push(format!("It begins: {preview}{}", if text.chars().count() > 600 { " …" } else { "" }));
        }
        lines.push(
            "The JSON holds `language`, `duration`, `words` and `segments` (each with `text`, `start`, `end` in seconds, \
             and `speaker` when there is more than one); the Nebo Media plugin reads it for captions and edit by transcript."
                .to_string(),
        );
        Ok((lines.join("\n"), vec![path]))
    }

    /// Makes the video and answers the result text and its file, which the
    /// chat shows as a card.
    ///
    /// The whole call counts as waiting (`ctx.waiting`): a clip can take its
    /// full wait (10 minutes, a swap 30) with no event at all, and a helper
    /// ended for silence at its own 10-minute bound was ended just as its
    /// clip landed, so the director made it again (audit 2026-10-07).
    async fn video(&self, ctx: &ToolContext, target: &Target, input: &Value) -> Result<(String, Vec<PathBuf>), String> {
        let _waiting = ctx.waiting.enter();
        let replace = str_of(input, "mode") == Some("replace");
        // A swap with no prompt is named for its cast member.
        let named_for = match (str_of(input, "prompt"), str_of(input, "cast")) {
            (Some(p), _) => p.to_string(),
            (None, Some(c)) if replace => format!("{c} swap"),
            _ => String::new(),
        };
        let into = str_of(input, "into");
        let name = file_names(into, target.default_folder, &named_for, "mp4", 1, now()).remove(0);
        // Everything a swap needs is checked before anything is uploaded.
        let swap = match (replace, str_of(input, "job")) {
            (true, None) => Some(self.swap_ready(ctx, target, input)?),
            _ => None,
        };
        let path = destination(&target.base, &name)?;
        let mut lent = None;
        let job = match (str_of(input, "job"), swap) {
            (Some(id), _) => Video {
                id: id.to_string(),
                model: String::new(),
                seconds: None,
            },
            (None, Some((member_dir, member, clip))) => {
                let mut files = Lent { hub: self.hub()?, shares: Vec::new() };
                let submitted = match self.swap_send(&mut files, &member_dir, member, clip).await {
                    Ok((video, references, attestation)) => {
                        self.media
                            .submit_video(&swap_body(input, &video, &references, &attestation))
                            .await
                    }
                    Err(e) => Err(e),
                };
                match submitted {
                    Ok(job) => {
                        lent = Some(files);
                        job
                    }
                    Err(e) => {
                        files.revoke().await;
                        return Err(e);
                    }
                }
            }
            (None, None) => {
                if str_of(input, "prompt").is_none() {
                    return Err("Give a `prompt` for the video.".to_string());
                }
                let frame = match str_of(input, "image") {
                    Some(img) => Some(first_frame(ctx, &target.base, img)?),
                    None => None,
                };
                // References in place of a first frame: the clip keeps
                // their subjects through it (mode "reference").
                let refs = references(ctx, &target.base, input)?;
                let mut body = video_body(input, frame);
                if !refs.is_empty() {
                    body["mode"] = json!("reference");
                    body["references"] = json!(refs);
                }
                match self.media.submit_video(&body).await {
                    Ok(job) => job,
                    Err(e) if !refs.is_empty() => return Err(without_references(e)),
                    Err(e) => return Err(e),
                }
            }
        };
        let waited = if replace {
            self.media.wait_video(&job.id, self.media.polling.limit * SWAP_WAIT_FACTOR).await
        } else {
            self.media.wait_video(&job.id, self.media.polling.limit).await
        };
        // The job has ended, made or not: its links go. A wait that ran out
        // leaves them for the job still running; they end within the hour.
        if let Some(files) = &lent
            && !matches!(&waited, Err(e) if e.starts_with(UNFINISHED))
        {
            files.revoke().await;
        }
        waited?;
        let size = self.media.download_video(&job.id, &path).await?;
        // Re-encoded first, so the tag is on the file that stays.
        let scrubbed = match input.get("scrub").and_then(Value::as_bool).unwrap_or(false) {
            true => Some(scrub(&path).await),
            false => None,
        };
        let tagging = path.clone();
        let tagged = tokio::task::spawn_blocking(move || tag::mp4(&tagging))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        let mut lines = vec![format!(
            "Made a video into {}: {name} ({size} bytes) at {}",
            target.place(&path),
            path.display()
        )];
        // The model stays in the log; the owner never sees what made it.
        tracing::info!(model = %job.model, job = %job.id, "made a video");
        let mut about = Vec::new();
        if let Some(s) = job.seconds {
            about.push(format!("{s} seconds"));
        }
        if replace && let Some(c) = str_of(input, "cast") {
            about.push(format!("cast {c}"));
        }
        about.push(format!("job {}", job.id));
        if tagged == Ok(true) {
            about.push("tagged AI-generated".to_string());
        }
        lines.push(format!("({})", about.join(", ")));
        if let Err(e) = &tagged {
            lines.push(format!("Not tagged AI-generated ({e}): tag it with the Nebo Media plugin before it goes anywhere."));
        }
        if replace {
            lines.push(
                "Before it goes anywhere: put the original clip's sound back and tag it AI-generated with the Nebo Media \
                 plugin's `audio mix` (`audio-from` the clip you sent, `ai-generated` \"true\"). That result reaches \
                 the owner as a card by itself; don't share_file it again."
                    .to_string(),
            );
        }
        if let Some(scrubbed) = scrubbed {
            lines.push(scrubbed);
            if let Ok(meta) = std::fs::metadata(&path) {
                lines.push(format!("Size now {} bytes.", meta.len()));
            }
        }
        Ok((lines.join("\n"), vec![path]))
    }
}

impl GenerateMediaTool {
    /// What a swap needs, checked before anything leaves this machine: a
    /// cast member the owner attested to, with a hero image, and a clip
    /// that can be sent. A replace takes its person only from the cast.
    fn swap_ready(
        &self,
        ctx: &ToolContext,
        target: &Target,
        input: &Value,
    ) -> Result<(PathBuf, cast::Member, Clip), String> {
        if str_of(input, "image").is_some() {
            return Err(
                "A replace takes the person only from the cast: leave `image` out and name the cast member in `cast`."
                    .to_string(),
            );
        }
        let name = str_of(input, "cast").ok_or_else(no_cast)?;
        let cast = self.cast()?;
        let (dir, member) = cast.load(name)?;
        cast::ready(&member, &cast.images(&dir))?;
        let video = str_of(input, "video").ok_or("Give `video`: the clip whose person is replaced.")?;
        Ok((dir, member, clip(ctx, &target.base, video)?))
    }

    /// Lends the clip and the member's images to Janus: the clip's URL, the
    /// images' URLs (the hero first) and the attestation id. An image goes
    /// up once; its stored id is kept in `cast.json`, and a stored copy the
    /// hub no longer has goes up again.
    async fn swap_send(
        &self,
        files: &mut Lent,
        dir: &Path,
        mut member: cast::Member,
        clip: Clip,
    ) -> Result<(String, Vec<String>, String), String> {
        let attestation = member.attestation.as_ref().map(|a| a.id.clone()).unwrap_or_default();
        let video = match clip {
            Clip::Url(url) => url,
            Clip::File { path, mime } => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "clip.mp4".into());
                files.send(&path, &name, mime).await?.1
            }
        };
        let cast = self.cast()?;
        let mut references = Vec::new();
        for image in cast.images(dir) {
            let file = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let upload_name = format!("{}-{file}", member.name);
            let cached = member.hub_file_ids.get(&file).cloned();
            let url = match cached {
                Some(id) => match files.link(&id).await {
                    Ok(url) => url,
                    Err(LinkError::Gone) => {
                        let (id, url) = files.send(&image, &upload_name, image_mime(&image)).await?;
                        member.hub_file_ids.insert(file.clone(), id);
                        cast.save(dir, &member)?;
                        url
                    }
                    Err(LinkError::Other(e)) => return Err(e),
                },
                None => {
                    let (id, url) = files.send(&image, &upload_name, image_mime(&image)).await?;
                    member.hub_file_ids.insert(file.clone(), id);
                    cast.save(dir, &member)?;
                    url
                }
            };
            references.push(url);
        }
        Ok((video, references, attestation))
    }

    /// kind "cast": with no `cast`, the cast listed; with `cast` and
    /// `image`, the image added (a new member gets it as their hero, then
    /// the owner's card); with `cast` alone, that member, and the owner's
    /// card when he has not confirmed them yet.
    async fn cast_change(&self, ctx: &ToolContext, target: &Target, input: &Value) -> Result<String, String> {
        let cast = self.cast()?;
        let Some(name) = str_of(input, "cast") else {
            let members = cast.list();
            if members.is_empty() {
                return Ok("The cast is empty. Add someone with `cast` (their name) and `image` (their hero photo).".to_string());
            }
            let mut lines = vec![format!("The cast ({}):", cast.root.display())];
            for (dir, m) in members {
                lines.push(describe(&cast, &dir, &m));
            }
            return Ok(lines.join("\n"));
        };
        let name = cast::valid_name(name)?;
        let image = match str_of(input, "image") {
            Some(given) => Some(readable(ctx, &target.base, given)?),
            None => None,
        };
        let hero = input.get("hero").and_then(Value::as_bool).unwrap_or(false);
        let (dir, mut member, said) = match (cast.load(name), image) {
            (Ok((dir, member)), Some(image)) => {
                if member.attestation.is_none() {
                    let file = cast.add(name, &image, hero)?;
                    return Ok(format!("Added {file} to {}'s images.", member.name));
                }
                // A new image could be someone else: the owner confirms
                // again before it joins a member a swap may use.
                let Ok((kind, attestation)) = cast::attest(ctx, &member.name).await else {
                    return Err(format!(
                        "The image was not added: {0} is already confirmed, and a new image of {0} joins only when \
                         the owner confirms {0} again on the card in a chat with you.",
                        member.name
                    ));
                };
                let file = cast.add(name, &image, hero)?;
                let (_, mut member) = cast.load(name)?;
                member.kind = Some(kind);
                member.attestation = Some(attestation);
                cast.save(&dir, &member)?;
                return Ok(format!("Added {file} to {}'s images; the owner confirmed them again.", member.name));
            }
            (Ok((dir, member)), None) => {
                let said = describe(&cast, &dir, &member);
                (dir, member, said)
            }
            (Err(_), Some(image)) => {
                let (dir, member) = cast.create(name, &image)?;
                let said = format!("Added {name} to the cast with their hero image ({}).", dir.display());
                (dir, member, said)
            }
            (Err(e), None) => return Err(e),
        };
        if member.attestation.is_some() {
            return Ok(said);
        }
        match cast::attest(ctx, &member.name).await {
            Ok((kind, attestation)) => {
                member.kind = Some(kind);
                member.attestation = Some(attestation);
                cast.save(&dir, &member)?;
                Ok(format!(
                    "{said} The owner confirmed {}; they can now replace a person in a video.",
                    member.name
                ))
            }
            Err(e) => Ok(format!("{said} {e}")),
        }
    }
}

/// One line about a cast member: whether the owner confirmed them, and
/// their images.
fn describe(cast: &cast::Cast, dir: &Path, m: &cast::Member) -> String {
    let images: Vec<String> =
        cast.images(dir).iter().filter_map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).collect();
    let status = match (&m.attestation, m.kind.as_deref()) {
        (Some(_), Some(kind)) => format!("confirmed by the owner ({kind})"),
        (Some(_), None) => "confirmed by the owner".to_string(),
        (None, _) => "not confirmed by the owner yet, so not usable for a swap".to_string(),
    };
    format!("- {}: {status}; images {}", m.name, images.join(", "))
}

/// What a swap without `cast` is told.
fn no_cast() -> String {
    "Give `cast`: the cast member's name. A replace takes the person only from the cast (generate_media kind \"cast\" \
     lists it)."
        .to_string()
}

/// The extension `bytes` really are: `asked` when they are what was asked
/// for (`jpg` and `jpeg` alike) or are not an image this can tell, else the
/// image type their magic bytes say.
fn real_image_ext<'a>(bytes: &[u8], asked: &'a str) -> &'a str {
    let real = match ai::sniff_image_mime(bytes) {
        Some("image/png") => "png",
        Some("image/jpeg") => "jpg",
        Some("image/webp") => "webp",
        Some("image/gif") => "gif",
        _ => return asked,
    };
    let same =
        asked.eq_ignore_ascii_case(real) || (real == "jpg" && asked.eq_ignore_ascii_case("jpeg"));
    if same { asked } else { real }
}

// ponytail: sound effects are off until Janus has a sound model; set this
// to `true` to offer kind "sound" again (schema, descriptions, validate_input).
const SOUND_EFFECTS: bool = false;

/// The kinds offered to the model: kind "sound" only while [`SOUND_EFFECTS`].
fn kinds() -> Vec<&'static str> {
    ["image", "video", "speech", "music", "sound", "transcript", "voices", "cast"]
        .into_iter()
        .filter(|k| SOUND_EFFECTS || *k != "sound")
        .collect()
}

/// `text` when sound effects are offered, else nothing.
fn if_sound(text: &str) -> &str {
    if SOUND_EFFECTS { text } else { "" }
}

/// The answer to a kind that isn't offered.
fn unknown_kind() -> String {
    let kinds = kinds();
    let (last, rest) = kinds.split_last().expect("kinds");
    format!("`kind` is {} or {last}.", rest.join(", "))
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
        format!("Makes new media with AI through NeboAI and saves it as a file, billed to the owner's plan; also transcribes.\n\
         - kind \"image\": 1-4 images. kind \"video\": one MP4 of 1-30 s, taking minutes. `image` sets the first frame \
           (image to video): a picture the owner approved. `scrub: true` re-encodes it for scroll-scrubbing.\n\
         - Someone speaking on camera: the line is made inside the clip (see `prompt`), never as kind \"speech\"; keep that \
           clip's sound, never extract or join its dialogue.\n\
         - Swap the person in an existing video: kind \"video\", `mode` \"replace\", `video`, `cast`; kind \"cast\" lists and \
           adds who a swap may use (the owner confirms each once). Recipe: the character-swap skill. One character across \
           new shots: start frames from their portrait (`references`).\n\
         - kind \"speech\": `text` read aloud, for an off-screen narrator or voice-over only; stock voices, never a real \
           person's. kind \"music\": a track from `prompt`.{} kind \"transcript\": `file` to JSON \
           with word timings, speakers.\n\
         - Remake only what failed, once at most without asking. What it makes is tagged AI-generated and reaches the owner \
           as a card by itself; don't share_file it.\n\
         - `into`: an absolute or `~/` path is saved there, a relative one inside the app's folder (yours or `app`) or the \
           workspace; project work inside `~/NeboAI/Media/Projects/<project>/`. Then use the path the result gives.\n\
         - It only makes new media; a media plugin such as Nebo Media only edits (mix, trim, resize, convert, inspect).\n\
         - The result gives paths, never pictures; to look at one, use the vision helper.\n\
         - Leave `model` out unless the owner named one.",
            if_sound(" kind \"sound\": a sound effect.")
        )
    }

    fn schema(&self) -> Value {
        let sound_prompt = if_sound(" Sound: the sound, its source, place and movement.");
        let audio_formats = if SOUND_EFFECTS { "Speech, music, sound" } else { "Speech, music" };
        let sound_seconds = if_sound(" Sound: up to 30.");
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": kinds(), "description": "What to make; `transcript` to transcribe `file`; `voices` to list the speech voices; `cast` to manage the people a swap may use." },
                "prompt": { "type": "string", "description": format!("Image or video: what it shows, in detail: subject, style, light, framing, motion. Someone speaking on camera: one speaker per clip, a medium or close shot, the exact line in quotes, the voice described in the same words in every clip of that person, and \"Only her voice. No music, no background sound.\" (his, for a man). Music: the style the brief asks for, never a house style (none given: offer 2-3 contrasting takes), mood, instruments, tempo, what it is for.{sound_prompt}") },
                "text": { "type": "string", "description": "Speech: the exact words to say, up to about 4,000 characters. Only for an off-screen narrator or voice-over, never a person seen speaking: their line goes in the video `prompt`." },
                "voice": { "type": "string", "description": "Speech (an off-screen narrator or voice-over): a voice id from kind `voices`, chosen by its description: a woman's voice for a woman, a man's for a man, and a different voice for each character, kept for that character every time. Left out: the default voice." },
                "direction": { "type": "string", "description": "Speech: how the words are said: tone, pace, warmth, emotion, accent, e.g. \"deep, warm, unhurried documentary narrator; calm authority, slight gravel, pauses between phrases\". Describe the qualities; never name a real person to imitate. It shapes how the chosen voice speaks, not whose voice it is: pick the voice for the character first, and give each character their own direction." },
                "lyrics": { "type": "string", "description": "Music: the words to sing, lines separated by newlines; [Verse], [Chorus], [Bridge] tags allowed. Left out: instrumental." },
                "instrumental": { "type": "boolean", "description": "Music: no vocals (the default without `lyrics`). false without `lyrics`: words are written for it." },
                "file": { "type": "string", "description": "Transcript: the audio or video file to transcribe (an absolute, `~/` or relative path)." },
                "language": { "type": "string", "description": "Transcript: the spoken language code (e.g. en, es) when known. Left out: detected." },
                "into": { "type": "string", "description": "The file to write: an absolute or `~/` path is saved there; a relative one (e.g. `assets/hero.png`) is inside the app's folder or the workspace. Project work always goes inside its project, `~/NeboAI/Media/Projects/<project>/`, in sources/, voice/, music/, frames/, clips/, work/, versions/ or deliver/ (finals), e.g. `~/NeboAI/Media/Projects/lighthouse/frames/s1-start.png`. Left out: a name from the prompt, loose in the workspace." },
                "app": { "type": "string", "description": "The app whose folder the file goes in. Leave out when you are the app, or for the workspace." },
                "model": { "type": "string", "description": "A NeboAI media model. Leave out for the default." },
                "n": { "type": "integer", "minimum": 1, "maximum": MAX_IMAGES, "description": "Image: how many (1-4)." },
                "size": { "type": "string", "description": "Image: e.g. 1024x1024, 1536x1024, 1024x1536." },
                "quality": { "type": "string", "description": "Image: low, medium, high." },
                "background": { "type": "string", "description": "Image: transparent or opaque." },
                "output_format": { "type": "string", "enum": ["png", "webp", "jpeg", "mp3", "wav"], "description": format!("File format. Image: png, webp or jpeg. {audio_formats}: mp3 (default) or wav.") },
                "seconds": { "type": "number", "minimum": 1, "maximum": audio::MAX_MUSIC_SECONDS, "description": format!("Video: length in whole seconds, 1-30 (default 5); a fraction is rounded up. Music: the length the owner chose (suggest the piece's length), up to 300; fractions such as 11.1 are kept.{sound_seconds}") },
                "resolution": { "type": "string", "description": "Video: e.g. 720p, 1080p." },
                "aspect_ratio": { "type": "string", "description": "Video: e.g. 16:9, 9:16, 1:1." },
                "image": { "type": "string", "description": "Video: the first frame, a picture the owner approved: make each shot's start frame as an image, show it and get the owner's yes, then make the clip from it. A file (absolute, `~/`, or relative to the folder), an https URL or a data URL. Kind cast: a photo of `cast` to add." },
                "references": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_REFERENCES, "description": "Image or video: up to 4 pictures whose people, products or places the result keeps, such as a character's cast portrait: files (absolute, `~/` or relative to the folder). Name them in the prompt by order: \"the woman in Image 1\", \"Image 2's red kart\". Video: the clip keeps them throughout and starts from no `image`; prefer an approved start frame made with them." },
                "scrub": { "type": "boolean", "description": "Video: re-encode with every frame a keyframe for scroll-scrubbing (needs ffmpeg)." },
                "job": { "type": "string", "description": "Video: a job id from an earlier call that did not finish; picks it up instead of making a new one." },
                "mode": { "type": "string", "enum": ["replace"], "description": "Video: `replace` puts cast member `cast` in place of the person in `video` (720p unless `resolution` says otherwise; `prompt` optional; up to 30 minutes). Prepare the clip with the Nebo Media plugin first; afterwards its `audio mix` with `audio-from` the clip and `ai-generated` puts the sound back and tags the file, which reaches the owner as a card by itself." },
                "video": { "type": "string", "description": "Video replace: the clip (30 s or less, 24 fps, up to 100 MB), a file or an https URL." },
                "cast": { "type": "string", "description": "Video replace: the cast member who plays the person; the person comes only from the cast, never from an image. Kind cast: the member to add `image` to (a new one gets it as their hero: front-facing, full body, good light; then the owner confirms them once on a card; a new image of a confirmed one shows him the card again) or to show (one not confirmed yet gets the card again). Left out: the cast is listed." },
                "hero": { "type": "boolean", "description": "Kind cast: `image` replaces their hero image instead of adding an angle." }
            },
            "required": ["kind"]
        })
    }

    fn search_hint(&self) -> &str {
        "generate image video voiceover music sound transcribe swap"
    }

    fn triggers(&self) -> &[String] {
        &self.triggers
    }

    fn capability(&self, _input: &Value) -> Option<&'static str> {
        Some("file")
    }

    /// An absolute or `~/` `into` is a write there: a folder rule judges it
    /// as it judges `write_file`'s path.
    fn rule_field(&self, input: &Value) -> Option<types::permissions::RuleField> {
        let path = types::pathres::expand(str_of(input, "into")?);
        path.is_absolute().then_some(types::permissions::RuleField::Folder(path))
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    /// What it makes is what the owner asked for: it shows in the chat as a
    /// card, the way `share_file` shows a file.
    fn emits_image(&self, input: &Value) -> bool {
        !matches!(str_of(input, "kind"), Some("cast" | "voices"))
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        match str_of(input, "kind") {
            Some("sound") if !SOUND_EFFECTS => return Err("Sound effects aren't available yet.".to_string()),
            Some("speech") => {
                if str_of(input, "text").is_none() {
                    return Err("Give the `text` to speak.".to_string());
                }
                return Ok(());
            }
            Some("voices") => return Ok(()),
            Some("transcript") => {
                if str_of(input, "file").is_none() {
                    return Err("Give `file`: the audio or video file to transcribe.".to_string());
                }
                return Ok(());
            }
            Some("music" | "sound") => {
                if str_of(input, "prompt").is_none() {
                    return Err("Give a `prompt`.".to_string());
                }
                return Ok(());
            }
            Some("cast") => {
                if str_of(input, "image").is_some() && str_of(input, "cast").is_none() {
                    return Err("Give `cast`: the name of the cast member the image is of.".to_string());
                }
                if input.get("hero").and_then(Value::as_bool) == Some(true) && str_of(input, "image").is_none() {
                    return Err("Give `image`: the new hero photo.".to_string());
                }
                return Ok(());
            }
            _ => {}
        }
        let video = str_of(input, "kind") == Some("video");
        let has_refs = match input.get("references") {
            None | Some(Value::Null) => false,
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::String(s)) => !s.trim().is_empty(),
            Some(_) => return Err("`references` is a list of picture files.".to_string()),
        };
        if has_refs {
            if str_of(input, "mode").is_some() {
                return Err("`references` is not used with `mode` replace: the person comes from `cast`.".to_string());
            }
            if str_of(input, "image").is_some() {
                return Err(
                    "Give `image` (an approved start frame) or `references`, not both: a clip from a start frame keeps \
                     what the frame shows."
                        .to_string(),
                );
            }
        }
        match str_of(input, "mode") {
            Some("replace") if video => {
                if str_of(input, "job").is_some() {
                    return Ok(());
                }
                if str_of(input, "cast").is_none() {
                    return Err(no_cast());
                }
                if str_of(input, "video").is_none() {
                    return Err("Give `video`: the clip whose person is replaced.".to_string());
                }
                return Ok(());
            }
            Some(other) => return Err(format!("`mode` is `replace` (kind video only) or left out; got `{other}`.")),
            None => {}
        }
        if str_of(input, "prompt").is_none() && !(video && str_of(input, "job").is_some()) {
            return Err("Give a `prompt`.".to_string());
        }
        Ok(())
    }

    fn activity(&self, input: &Value) -> String {
        match str_of(input, "kind") {
            Some("video") if str_of(input, "mode") == Some("replace") => "swapping the person in a video".to_string(),
            Some("video") => "making a video".to_string(),
            Some("cast") => "working on the cast".to_string(),
            Some("speech") => "making speech".to_string(),
            Some("music") => "making music".to_string(),
            Some("sound") => "making a sound effect".to_string(),
            Some("transcript") => "transcribing".to_string(),
            Some("voices") => "listing voices".to_string(),
            _ => "making an image".to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        match str_of(input, "kind") {
            Some("video") if str_of(input, "mode") == Some("replace") => "Swapped the person in a video".to_string(),
            Some("video") => "Made a video".to_string(),
            Some("cast") => "Worked on the cast".to_string(),
            Some("speech") => "Made speech".to_string(),
            Some("music") => "Made music".to_string(),
            Some("sound") => "Made a sound effect".to_string(),
            Some("transcript") => "Transcribed".to_string(),
            Some("voices") => "Listed voices".to_string(),
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
                Some("image") => self.image(ctx, &target, &input).await,
                Some("video") => self.video(ctx, &target, &input).await,
                Some(kind @ ("speech" | "music" | "sound")) => self.audio_file(&target, &input, kind).await,
                Some("transcript") => self.transcript(ctx, &target, &input).await,
                Some("voices") => {
                    return match self.voice_list().await {
                        Ok(text) => ToolResult::ok(text),
                        Err(e) => ToolResult::error(e),
                    };
                }
                Some("cast") => {
                    return match self.cast_change(ctx, &target, &input).await {
                        Ok(text) => ToolResult::ok(text),
                        Err(e) => ToolResult::error(e),
                    };
                }
                _ => Err(unknown_kind()),
            };
            match made {
                // Every file is its own card: the first on `image_url`,
                // the rest after it.
                Ok((text, files)) => files.iter().fold(ToolResult::ok(text), |result, file| {
                    result.with_image_url(file.to_string_lossy())
                }),
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

    /// Nothing is ever written into an app installed from the marketplace:
    /// naming it is refused, and its own employee's files go to the
    /// workspace. The owner's own apps still get theirs.
    #[test]
    fn an_installed_apps_folder_is_never_written() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
        for (id, name) in [("app-own", "Racer"), ("app-bought", "Bought")] {
            store.create_agent(id, Some("agent"), name, "", "", "{}", None, None).unwrap();
            store.set_agent_app_fields(id, true, Some(&format!("/tmp/{id}/ui")), None, None).unwrap();
        }
        store.set_agent_napp_path("app-bought", "/data/nebo/agents/bought.napp").unwrap();
        let tool = GenerateMediaTool::new(Media::new(String::new(), String::new(), None), store);
        let ctx = ToolContext::default();

        let own = tool.target(&ctx, &serde_json::json!({"app": "Racer"})).unwrap();
        assert_eq!(own.base, PathBuf::from("/tmp/app-own/ui"));

        let named = tool.target(&ctx, &serde_json::json!({"app": "Bought"})).err().expect("refused");
        assert!(named.contains("installed from the marketplace"), "{named}");

        let me = ToolContext { session_key: "agent:app-bought:web".to_string(), ..Default::default() };
        let mine = tool.target(&me, &serde_json::json!({})).unwrap();
        assert_ne!(mine.base, PathBuf::from("/tmp/app-bought/ui"), "its own call goes to the workspace");
        assert_eq!(mine.label, "the workspace");
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
        assert_eq!(video_body(&json!({"prompt": "p", "seconds": 5.2}), None)["seconds"], 6);
        assert_eq!(video_body(&json!({"prompt": "p", "seconds": 8.0}), None)["seconds"], 8);
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
            said.contains("plan is used up this month and does not cover this image"),
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

    /// Janus answered JPEG bytes for a PNG request (xai does): the file is
    /// saved as `.jpg`, never a `.png` holding a JPEG, the result names the
    /// real file, and that file rides on the result for the chat's card.
    #[tokio::test]
    async fn a_jpeg_returned_for_a_png_request_is_saved_as_jpg() {
        let jpeg = b"\xff\xd8\xff\xe0fake jpeg".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
        let answer = format!(r#"{{"created":1,"data":[{{"b64_json":"{b64}"}}]}}"#);
        let answer: &'static str = Box::leak(answer.into_boxed_str());
        let janus = mock(Box::new(move |_, _| {
            (200, "application/json", answer.as_bytes().to_vec())
        }))
        .await;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
        let tool =
            GenerateMediaTool::new(Media::new(janus.url.clone(), "bot-1".into(), None), store);
        let base = tmp.path().join("files");
        std::fs::create_dir_all(&base).unwrap();
        let target = Target {
            base: base.clone(),
            label: "the workspace".into(),
            default_folder: "media",
        };

        let (text, files) = tool
            .image(
                &ToolContext::default(),
                &target,
                &json!({"prompt": "a red kart", "into": "hero.png"}),
            )
            .await
            .unwrap();
        let file = &files[0];
        assert_eq!(files, [base.join("hero.jpg")]);
        assert_eq!(std::fs::read(file).unwrap(), jpeg);
        assert!(!base.join("hero.png").exists(), "no .png may hold the JPEG");
        assert!(
            text.contains("hero.jpg") && !text.contains("hero.png"),
            "{text}"
        );

        // What was asked for comes back under the name asked for.
        assert_eq!(real_image_ext(b"\x89PNG\r\n\x1a\nrest", "png"), "png");
        assert_eq!(real_image_ext(&jpeg, "jpeg"), "jpeg");
        assert_eq!(real_image_ext(b"RIFF\0\0\0\0WEBPVP8 ", "png"), "webp");
        assert_eq!(real_image_ext(b"not an image", "png"), "png");
        assert!(
            tool.emits_image(&json!({"kind": "image"})),
            "a made image is a card"
        );
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
        media.wait_video(&job.id, fast().limit).await.unwrap();
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

    /// Every video call counts as waiting (`ctx.waiting`) while it runs, not
    /// only a swap: a shot helper waiting on its clip is not silent, so its
    /// collector never ends it as stalled (audit 2026-10-07: helpers were
    /// ended at their 10-minute bound just as their clips landed). The
    /// result line never names the model that made it.
    #[tokio::test]
    async fn a_video_call_counts_as_waiting_and_names_no_model() {
        let waiting = Arc::new(crate::Waiting::default());
        let seen_waiting = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (w, seen) = (waiting.clone(), seen_waiting.clone());
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/videos" => (200, "application/json", br#"{"id":"vid_w","object":"video","status":"running","model":"vendor-video-9","seconds":5}"#.to_vec()),
            "/v1/videos/vid_w" => {
                seen.store(!w.is_idle(), std::sync::atomic::Ordering::SeqCst);
                (200, "application/json", br#"{"id":"vid_w","status":"succeeded"}"#.to_vec())
            }
            "/v1/videos/vid_w/content" => (200, "video/mp4", b"MP4DATA".to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
        let tool = GenerateMediaTool::new(Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(fast()), store);
        let target = Target { base: tmp.path().to_path_buf(), label: "the workspace".into(), default_folder: "media" };
        let ctx = ToolContext { waiting: waiting.clone(), ..Default::default() };
        assert!(waiting.is_idle());
        let (text, files) = tool
            .video(&ctx, &target, &json!({"kind": "video", "prompt": "a lighthouse at dusk", "into": "clips/s1.mp4"}))
            .await
            .unwrap();
        assert!(seen_waiting.load(std::sync::atomic::Ordering::SeqCst), "the wait counted as waiting");
        assert!(waiting.is_idle(), "the wait ends with the call");
        assert_eq!(files, [tmp.path().join("clips/s1.mp4")]);
        assert!(!text.contains("vendor-video-9") && !text.contains("model"), "{text}");
        assert!(text.contains("5 seconds"), "{text}");
    }

    /// Reference pictures (a character's cast portrait) go to Janus as data
    /// URLs, read the way a start frame is read, in the order given; the
    /// result says how many were used.
    #[tokio::test]
    async fn image_references_go_to_janus_as_data_urls_in_order() {
        let png = b"\x89PNG\r\n\x1a\nmade".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let answer: &'static str = Box::leak(format!(r#"{{"created":1,"data":[{{"b64_json":"{b64}"}}]}}"#).into_boxed_str());
        let janus = mock(Box::new(move |_, _| (200, "application/json", answer.as_bytes().to_vec()))).await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
        let tool = GenerateMediaTool::new(Media::new(janus.url.clone(), "bot-1".into(), None), store);
        std::fs::create_dir_all(tmp.path().join("cast/mara")).unwrap();
        std::fs::write(tmp.path().join("cast/mara/portrait.png"), b"mara-png").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let kart = elsewhere.path().join("kart.jpg");
        std::fs::write(&kart, b"kart-jpg").unwrap();
        let target = Target { base: tmp.path().to_path_buf(), label: "the workspace".into(), default_folder: "media" };
        let (text, files) = tool
            .image(
                &ToolContext::default(),
                &target,
                &json!({"prompt": "The woman in Image 1 beside Image 2's kart", "into": "frames/s1.png",
                        "references": ["cast/mara/portrait.png", kart.to_string_lossy()]}),
            )
            .await
            .unwrap();
        assert_eq!(files, [tmp.path().join("frames/s1.png")]);
        assert!(text.contains("from 2 reference pictures"), "{text}");
        let seen = janus.seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[0].1).unwrap();
        let enc = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        assert_eq!(
            body["references"],
            json!([format!("data:image/png;base64,{}", enc(b"mara-png")), format!("data:image/jpeg;base64,{}", enc(b"kart-jpg"))])
        );
        assert_eq!(body["prompt"], "The woman in Image 1 beside Image 2's kart");
        assert!(body.get("mode").is_none());
    }

    /// A clip from references is mode "reference" with no first frame.
    /// While NeboAI cannot take them yet (it refuses the mode, or an image
    /// request's size), the model is told plainly to go on without them,
    /// and nothing else changes for a call without references.
    #[tokio::test]
    async fn references_are_refused_cleanly_until_janus_takes_them() {
        let janus = mock(Box::new(|path, _| match path {
            "/v1/videos" => (400, "application/json", br#"{"error":{"message":"mode must be generate or replace","type":"invalid_request_error","code":"invalid_request"}}"#.to_vec()),
            _ => (400, "application/json", br#"{"error":{"message":"Invalid JSON: http: request body too large","type":"invalid_request_error","code":"invalid_json"}}"#.to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
        let tool = GenerateMediaTool::new(Media::new(janus.url.clone(), "bot-1".into(), None).with_polling(fast()), store);
        std::fs::write(tmp.path().join("mara.png"), b"mara-png").unwrap();
        let target = Target { base: tmp.path().to_path_buf(), label: "the workspace".into(), default_folder: "media" };
        let ctx = ToolContext::default();

        let video = tool
            .video(&ctx, &target, &json!({"kind": "video", "prompt": "Image 1 waves", "references": ["mara.png"]}))
            .await
            .unwrap_err();
        assert!(video.starts_with("Reference images aren't available on NeboAI yet."), "{video}");
        let image = tool
            .image(&ctx, &target, &json!({"prompt": "Image 1 on a porch", "references": ["mara.png"]}))
            .await
            .unwrap_err();
        assert!(image.starts_with("Reference images aren't available on NeboAI yet."), "{image}");
        {
            let seen = janus.seen.lock().unwrap();
            let body: Value = serde_json::from_str(&seen[0].1).unwrap();
            assert_eq!(body["mode"], "reference");
            assert!(body.get("image").is_none());
            assert_eq!(body["references"].as_array().map(Vec::len), Some(1));
        }
        // Without references the same refusal is passed on as it is.
        let plain = tool.image(&ctx, &target, &json!({"prompt": "a porch"})).await.unwrap_err();
        assert!(!plain.contains("Reference images"), "{plain}");
    }

    #[test]
    fn references_are_checked_before_anything_is_sent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.gif"), b"gif").unwrap();
        std::fs::write(tmp.path().join("a.png"), b"png").unwrap();
        let ctx = ToolContext::default();
        let refs = |v: Value| references(&ctx, tmp.path(), &json!({ "references": v }));
        assert_eq!(refs(json!([])).unwrap(), Vec::<String>::new());
        assert_eq!(refs(json!("a.png")).unwrap().len(), 1);
        assert!(refs(json!(["a.gif"])).unwrap_err().contains("PNG, WebP or JPEG"));
        assert!(refs(json!(["https://x/y.png"])).unwrap_err().contains("save `https://x/y.png` to a file"));
        assert!(refs(json!(["../a.png"])).is_err());
        assert!(refs(json!(["a.png", "a.png", "a.png", "a.png", "a.png"])).unwrap_err().contains("up to 4"));
        assert_eq!(refs(json!(["data:image/png;base64,eA=="])).unwrap(), ["data:image/png;base64,eA=="]);

        let tool_ok = |v: Value| {
            let tmp = tempfile::tempdir().unwrap();
            let store = Arc::new(db::Store::new(&tmp.path().join("t.db").to_string_lossy()).unwrap());
            GenerateMediaTool::new(Media::new("http://127.0.0.1:9".into(), "bot-1".into(), None), store).validate_input(&v)
        };
        assert!(tool_ok(json!({"kind": "image", "prompt": "p", "references": ["a.png"]})).is_ok());
        assert!(tool_ok(json!({"kind": "video", "prompt": "p", "references": ["a.png"]})).is_ok());
        assert!(tool_ok(json!({"kind": "video", "prompt": "p", "image": "s1.png", "references": ["a.png"]}))
            .unwrap_err()
            .contains("not both"));
        assert!(tool_ok(json!({"kind": "video", "mode": "replace", "cast": "mara", "video": "v.mp4", "references": ["a.png"]}))
            .unwrap_err()
            .contains("comes from `cast`"));
        assert!(tool_ok(json!({"kind": "image", "prompt": "p", "references": 3})).unwrap_err().contains("list of picture files"));
    }

    /// A refusal from the service behind NeboAI never reaches the model in
    /// its own words when they could name it: a category is said, and the
    /// words stay in the log. NeboAI's own check of a request keeps its
    /// words, which say what to fix.
    #[test]
    fn refusals_name_no_provider_or_model() {
        let upstream = failure("video", 400, r#"{"error":{"code":"upstream_rejected","message":"Veo3 rejected the prompt"}}"#, true);
        assert!(!upstream.to_lowercase().contains("veo"), "{upstream}");
        assert!(upstream.contains("request was refused"), "{upstream}");
        let named = failure("image", 400, r#"{"error":{"message":"gpt-image-1 does not take size 9x9"}}"#, true);
        assert!(!named.contains("gpt"), "{named}");
        let server = failure("music", 502, r#"{"error":{"code":"upstream_error","message":"Lyria timed out"}}"#, true);
        assert!(!server.contains("Lyria") && server.contains("service error"), "{server}");
        let busy = failure("video", 429, r#"{"error":{"code":"provider_rate_limit","message":""}}"#, true);
        assert!(busy.contains("busy") && !busy.contains("plan"), "{busy}");
        for said in [&upstream, &named, &server, &busy] {
            assert!(!said.contains("Janus"), "{said}");
        }
        assert_eq!(
            job_failed(&json!({"error": {"message": "Kling refused: content filter"}})),
            "The video could not be made: it was refused. Change the prompt and try once more."
        );
        assert_eq!(
            job_failed(&json!({"error": "the prompt was refused by the safety check"})),
            "The video could not be made: the prompt was refused by the safety check"
        );
        assert!(!names_a_provider("Falls of light; a wandering fox at dusk."));
        assert!(names_a_provider("made with veo3") && names_a_provider("OpenAI said no") && names_a_provider("flux-pro"));
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
        let err = media.wait_video("v", fast().limit).await.unwrap_err();
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
        let err = media.wait_video("v", Duration::from_millis(60)).await.unwrap_err();
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
        media.wait_video("v", fast().limit).await.unwrap();
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
        let made = command::new::<std::process::Command>(&ffmpeg, command::Console::Hidden)
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

    /// A tool over `janus` whose own app "Racer" serves from `ui`, so a
    /// call naming it writes there and nowhere else.
    fn tool_with_app(janus: &MockJanus, tmp: &Path) -> (GenerateMediaTool, PathBuf) {
        let store = Arc::new(db::Store::new(&tmp.join("t.db").to_string_lossy()).unwrap());
        let ui = tmp.join("ui");
        store.create_agent("app-own", Some("agent"), "Racer", "", "", "{}", None, None).unwrap();
        store
            .set_agent_app_fields("app-own", true, Some(&ui.to_string_lossy()), None, None)
            .unwrap();
        let tool = GenerateMediaTool::new(Media::new(janus.url.clone(), "bot-1".into(), None), store);
        (tool, ui)
    }

    /// One MPEG-1 Layer III frame header at 128 kb/s, 44.1 kHz, no padding:
    /// 417 bytes and 1152 samples a frame.
    fn mp3_of(frames: usize) -> Vec<u8> {
        let mut out = b"ID3\x04\0\0\0\0\0\x02xx".to_vec();
        for _ in 0..frames {
            let mut frame = vec![0u8; 417];
            frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
            out.extend(frame);
        }
        out
    }

    /// Speech: the right Janus request (the speech speed, the words, the
    /// voice, MP3), an MP3 saved in the folder, a result line with its
    /// length and size, and the file on the result for the chat's card.
    #[tokio::test]
    async fn speech_asks_janus_and_saves_an_mp3_with_a_card() {
        let mp3: &'static [u8] = Box::leak(mp3_of(38).into_boxed_slice());
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/audio/speech" => (200, "audio/mpeg", mp3.to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, ui) = tool_with_app(&janus, tmp.path());
        let input = json!({
            "kind": "speech",
            "app": "Racer",
            "text": "Three laps, one winner.",
            "voice": "Coral",
        });
        assert!(tool.validate_input(&input).is_ok());
        assert!(tool.emits_image(&input), "made speech is a card");
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(!result.is_error, "{}", result.content);

        let file = PathBuf::from(result.image_url.as_deref().expect("the file rides on the result"));
        assert!(result.more_files.is_empty());
        assert!(file.starts_with(&ui) && file.extension().is_some_and(|e| e == "mp3"), "{}", file.display());
        assert_eq!(file.file_name().unwrap().to_string_lossy().split('-').take(3).collect::<Vec<_>>(), ["three", "laps", "one"]);
        // Saved tagged AI-generated, the audio itself untouched.
        let tagged = audio::tag_ai_generated(mp3.to_vec()).0;
        assert_eq!(std::fs::read(&file).unwrap(), tagged);
        // 38 frames of 1152 samples at 44.1 kHz.
        assert!(result.content.contains("1.0 seconds"), "{}", result.content);
        assert!(result.content.contains(&format!("{} bytes", tagged.len())), "{}", result.content);
        assert!(result.content.contains("tagged AI-generated"), "{}", result.content);
        assert!(result.content.contains(&file.display().to_string()), "{}", result.content);

        let seen = janus.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one call to Janus");
        let (line, body, headers) = &seen[0];
        assert!(line.starts_with("POST /v1/audio/speech "), "{line}");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body,
            json!({"model": "nebo-speech", "input": "Three laps, one winner.", "voice": "coral", "response_format": "mp3"})
        );
        let header = |k: &str| headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
        assert_eq!(header("x-bot-id").as_deref(), Some("bot-1"));
        assert_eq!(header("authorization").as_deref(), Some("Bearer bot-1"));
    }

    /// `into` resolves the way the file tools take a path: `~/` is the
    /// owner's home and an absolute path is itself, wherever they are; a
    /// relative one is inside the folder. Live 2026-10-03: `into:
    /// "NeboAI/Media/outputs/nebo-voiceover.mp3"` meant ~/NeboAI and landed
    /// in Nebo's files folder, where the next command didn't look.
    #[test]
    fn into_resolves_home_absolute_and_relative_paths() {
        let base = Path::new("/apps/kart/ui");
        let home = types::pathres::expand("~");
        assert_eq!(written_at(base, "~/X/y.mp3").unwrap(), home.join("X/y.mp3"));
        assert_eq!(written_at(base, "/elsewhere/vo.mp3").unwrap(), PathBuf::from("/elsewhere/vo.mp3"));
        assert_eq!(written_at(base, "assets/vo.mp3").unwrap(), base.join("assets/vo.mp3"));
        assert!(written_at(base, "../vo.mp3").is_err(), "a relative path never leaves the folder");
        assert!(written_at(base, "/").is_err(), "a folder is not a file");
    }

    /// Speech into an absolute path outside the app's folder is saved
    /// there, and the result names that absolute path and its folder.
    #[tokio::test]
    async fn speech_into_an_absolute_path_is_saved_there_and_named() {
        let mp3: &'static [u8] = Box::leak(mp3_of(4).into_boxed_slice());
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/audio/speech" => (200, "audio/mpeg", mp3.to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, ui) = tool_with_app(&janus, tmp.path());
        let want = tmp.path().join("NeboAI/Media/outputs/nebo-voiceover.mp3");
        let input = json!({"kind": "speech", "app": "Racer", "text": "One lap.", "into": want.to_string_lossy()});
        assert_eq!(tool.rule_field(&input), Some(types::permissions::RuleField::Folder(want.clone())));
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.image_url.as_deref(), Some(want.to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(&want).unwrap(), audio::tag_ai_generated(mp3.to_vec()).0);
        assert!(!ui.exists(), "nothing went into the app's folder");
        assert!(result.content.contains(&format!("at {}", want.display())), "{}", result.content);
        assert!(result.content.contains(&format!("into {}:", want.parent().unwrap().display())), "{}", result.content);

        // A relative `into` is still inside the app's folder, named by it.
        let result = tool
            .execute_dyn(&ToolContext::default(), json!({"kind": "speech", "app": "Racer", "text": "Two laps.", "into": "vo/two.mp3"}))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(result.image_url.as_deref(), Some(ui.join("vo/two.mp3").to_string_lossy().as_ref()));
        assert!(result.content.contains("into Racer's folder") && result.content.contains(&ui.join("vo/two.mp3").display().to_string()), "{}", result.content);
    }

    #[test]
    fn speech_format_and_body_follow_what_was_asked() {
        assert_eq!(speech_format(None, None), "mp3");
        assert_eq!(speech_format(Some("vo/line.wav"), None), "wav");
        assert_eq!(speech_format(None, Some("WAV")), "wav");
        assert_eq!(speech_format(Some("vo/line.mp3"), Some("wav")), "mp3");
        assert_eq!(speech_format(None, Some("png")), "mp3");
        let body = speech_body(&json!({"text": "hi", "output_format": "wav", "model": "m"}));
        assert_eq!(body, json!({"model": "m", "input": "hi", "response_format": "wav"}));
        let body = speech_body(&json!({"text": "hi", "voice": "Onyx", "direction": "slow, warm narrator"}));
        assert_eq!(body["voice"], "onyx");
        assert_eq!(body["instructions"], "slow, warm narrator");
        let tool = GenerateMediaTool::new(
            Media::new(String::new(), String::new(), None),
            Arc::new(db::Store::new(":memory:").unwrap()),
        );
        let err = tool.validate_input(&json!({"kind": "speech", "prompt": "hi"})).unwrap_err();
        assert!(err.contains("`text`"), "{err}");
    }

    /// Music: the generation request (kind, prompt, length, instrumental,
    /// format), the file saved under the real format's extension, tagged,
    /// and on the result for the chat's card.
    #[tokio::test]
    async fn music_asks_janus_and_saves_a_tagged_track() {
        let mp3: &'static [u8] = Box::leak(mp3_of(77).into_boxed_slice());
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/audio/generations" => (200, "audio/mpeg", mp3.to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, ui) = tool_with_app(&janus, tmp.path());
        let input = json!({"kind": "music", "app": "Racer", "prompt": "upbeat synthwave for a race intro", "seconds": 45, "into": "audio/intro.wav"});
        assert!(tool.validate_input(&input).is_ok());
        assert!(tool.emits_image(&input));
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(!result.is_error, "{}", result.content);
        // WAV asked for, MP3 answered: saved as what it is.
        let file = PathBuf::from(result.image_url.as_deref().unwrap());
        assert_eq!(file, ui.join("audio/intro.mp3"));
        assert_eq!(std::fs::read(&file).unwrap(), audio::tag_ai_generated(mp3.to_vec()).0);
        assert!(result.content.starts_with("Made music into Racer's folder: audio/intro.mp3 (2.0 seconds"), "{}", result.content);
        assert!(result.content.contains("instrumental") && result.content.contains("saved as .mp3"), "{}", result.content);

        let seen = janus.seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[0].1).unwrap();
        assert_eq!(
            body,
            json!({"kind": "music", "prompt": "upbeat synthwave for a race intro", "response_format": "wav", "seconds": 45, "instrumental": true})
        );
    }

    /// A sound effect goes to the same door as kind "sound"; a refusal
    /// for the plan reads as the owner's plan.
    #[tokio::test]
    async fn sound_effects_and_their_refusals() {
        let janus = mock(Box::new(move |_, n| match n {
            1 => (200, "audio/mpeg", mp3_of(2)),
            _ => (429, "application/json", br#"{"error":{"message":"Usage limit exceeded"}}"#.to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, _) = tool_with_app(&janus, tmp.path());
        let input = json!({"kind": "sound", "app": "Racer", "prompt": "tires screech on wet asphalt", "seconds": 3});
        let result = tool.execute_dyn(&ToolContext::default(), input.clone()).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.starts_with("Made sound effect into Racer's folder: assets/tires-screech-on-wet-asphalt-"), "{}", result.content);
        let body: Value = serde_json::from_str(&janus.seen.lock().unwrap()[0].1).unwrap();
        assert_eq!(body, json!({"kind": "sound", "prompt": "tires screech on wet asphalt", "response_format": "mp3", "seconds": 3}));
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(result.is_error && result.content.contains("plan is used up"), "{}", result.content);
        // No sound model yet: said plainly, never a "try again".
        let said = failure(
            "sound effect",
            503,
            r#"{"error":{"code":"kind_unavailable","message":"This request couldn't be completed. Try again.","type":"server_error"}}"#,
            true,
        );
        assert!(said.contains("not available on NeboAI yet") && said.contains("Do not try again"), "{said}");
        assert!(said.contains("Do not name or recommend other apps"), "{said}");
        assert!(said.contains("unless a call for it succeeded"), "{said}");
        // Offered to the model only while SOUND_EFFECTS; until then refused plainly.
        let offered = tool.schema()["properties"]["kind"]["enum"].as_array().unwrap().contains(&json!("sound"));
        assert_eq!(offered, SOUND_EFFECTS);
        assert_eq!(tool.description().contains("kind \"sound\""), SOUND_EFFECTS);
        assert_eq!(unknown_kind().contains("sound"), SOUND_EFFECTS);
        let checked = tool.validate_input(&json!({"kind": "sound", "prompt": "a door creaks"}));
        if SOUND_EFFECTS {
            assert!(checked.is_ok());
            assert!(tool.validate_input(&json!({"kind": "sound"})).unwrap_err().contains("`prompt`"));
        } else {
            assert_eq!(checked.unwrap_err(), "Sound effects aren't available yet.");
            assert_eq!(unknown_kind(), "`kind` is image, video, speech, music, transcript, voices or cast.");
        }
    }

    /// kind "voices": the list as lines, ids first; nothing is made.
    #[tokio::test]
    async fn voices_are_listed_by_id() {
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/audio/voices" => (
                200,
                "application/json",
                br#"{"object":"list","data":[{"id":"narrator-warm","name":"Warm narrator","description":"Calm storyteller","gender":"male","language":"en"},{"id":"nova","name":"nova"}]}"#.to_vec(),
            ),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, _) = tool_with_app(&janus, tmp.path());
        let input = json!({"kind": "voices"});
        assert!(tool.validate_input(&input).is_ok() && !tool.emits_image(&input));
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(!result.is_error && result.image_url.is_none(), "{}", result.content);
        assert!(result.content.contains("- narrator-warm: Warm narrator - Calm storyteller (male, en)"), "{}", result.content);
        assert!(result.content.contains("\n- nova"), "{}", result.content);
    }

    /// A transcript: the file sent as the transcription speed with word
    /// timings asked for, the answer saved beside it as transcript JSON.
    #[tokio::test]
    async fn transcript_saves_word_timed_json_beside_the_file() {
        let answer = br#"{"text":"Speaker 1: Hi. Speaker 2: Yes.","language":"en","duration":2.0,
            "words":[{"text":"Hi.","start":0.1,"end":0.4,"speaker":0},{"text":"Yes.","start":1.0,"end":1.3,"speaker":1}]}"#;
        let janus = mock(Box::new(move |path, _| match path {
            "/v1/audio/transcriptions" => (200, "application/json", answer.to_vec()),
            _ => (404, "text/plain", b"no".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, _) = tool_with_app(&janus, tmp.path());
        let clip = tmp.path().join("in/interview.m4a");
        std::fs::create_dir_all(clip.parent().unwrap()).unwrap();
        std::fs::write(&clip, b"fake audio").unwrap();
        let input = json!({"kind": "transcript", "file": clip.to_string_lossy()});
        assert!(tool.validate_input(&input).is_ok());
        let result = tool.execute_dyn(&ToolContext::default(), input).await;
        assert!(!result.is_error, "{}", result.content);
        let out = tmp.path().join("in/interview.transcript.json");
        assert_eq!(result.image_url.as_deref(), Some(out.to_string_lossy().as_ref()));
        let t: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(t["language"], "en");
        assert_eq!(t["words"][1], json!({"text": "Yes.", "start": 1.0, "end": 1.3, "speaker": "S2"}));
        assert_eq!(t["segments"].as_array().unwrap().len(), 2);
        assert!(result.content.contains("2 words, 2 segments, 2 speakers"), "{}", result.content);
        assert!(result.content.contains("It begins: [S1] Hi. [S2] Yes."), "{}", result.content);

        let seen = janus.seen.lock().unwrap();
        let (line, body, _) = &seen[0];
        assert!(line.starts_with("POST /v1/audio/transcriptions "), "{line}");
        for part in ["nebo-transcribe", "verbose_json", "timestamp_granularities[]", "word", "interview.m4a", "fake audio"] {
            assert!(body.contains(part), "{part} missing from the form");
        }

        // A file the endpoint does not take is refused before it is sent.
        let mov = tmp.path().join("in/clip.mov");
        std::fs::write(&mov, b"x").unwrap();
        drop(seen);
        let result = tool.execute_dyn(&ToolContext::default(), json!({"kind": "transcript", "file": mov.to_string_lossy()})).await;
        assert!(result.is_error && result.content.contains("Extract its audio"), "{}", result.content);
        assert_eq!(janus.seen.lock().unwrap().len(), 1);
        assert!(tool.validate_input(&json!({"kind": "transcript"})).unwrap_err().contains("`file`"));
    }

    #[test]
    fn audio_length_is_read_from_the_file() {
        let secs = audio_seconds(&mp3_of(38)).unwrap();
        assert!((secs - 38.0 * 1152.0 / 44100.0).abs() < 1e-9, "{secs}");
        // 16-bit mono at 24 kHz: 48,000 bytes a second, 1.5 seconds of data.
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 1, 0]);
        wav.extend(24_000u32.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend([2, 0, 16, 0]);
        wav.extend(b"data");
        wav.extend(72_000u32.to_le_bytes());
        wav.extend(vec![0u8; 72_000]);
        assert_eq!(audio_seconds(&wav), Some(1.5));
        assert_eq!(audio_seconds(b"not audio at all"), None);
    }

    /// n=3: three images, three files, and all three on the result, so the
    /// chat shows a card for each, not only the first.
    #[tokio::test]
    async fn three_images_give_three_cards() {
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let one = format!(r#"{{"b64_json":"{b64}"}}"#);
        let answer = format!(r#"{{"created":1,"data":[{one},{one},{one}]}}"#);
        let answer: &'static str = Box::leak(answer.into_boxed_str());
        let janus = mock(Box::new(move |_, _| (200, "application/json", answer.as_bytes().to_vec()))).await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, ui) = tool_with_app(&janus, tmp.path());
        let result = tool
            .execute_dyn(
                &ToolContext::default(),
                json!({"kind": "image", "app": "Racer", "prompt": "a red kart", "n": 3, "into": "assets/kart.png"}),
            )
            .await;
        assert!(!result.is_error, "{}", result.content);
        let files: Vec<String> = result.files().map(str::to_string).collect();
        let want: Vec<String> = (1..=3)
            .map(|i| ui.join(format!("assets/kart-{i}.png")).to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, want);
        for f in &files {
            assert_eq!(std::fs::read(f).unwrap(), png);
        }
        assert!(result.content.starts_with("Made 3 images"), "{}", result.content);
    }

    #[test]
    fn a_first_frame_file_becomes_a_data_url() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/start.png"), b"png").unwrap();
        let ctx = ToolContext::default();
        assert_eq!(
            first_frame(&ctx, dir.path(), "assets/start.png").unwrap(),
            "data:image/png;base64,cG5n"
        );
        assert_eq!(
            first_frame(&ctx, dir.path(), "https://x/y.png").unwrap(),
            "https://x/y.png"
        );
        assert!(first_frame(&ctx, dir.path(), "../start.png").is_err());
        assert!(first_frame(&ctx, dir.path(), "assets/start.gif").is_err());
        // A frame outside the folder, named by its absolute path.
        let out = tempfile::tempdir().unwrap();
        let elsewhere = out.path().join("s2-start.jpg");
        std::fs::write(&elsewhere, b"jpg").unwrap();
        assert_eq!(
            first_frame(&ctx, dir.path(), &elsewhere.to_string_lossy()).unwrap(),
            "data:image/jpeg;base64,anBn"
        );
    }

    // ── Character swap ──────────────────────────────────────────────

    /// One local server for Janus and the hub alike: upload n answers
    /// `file-n`, link n is `share-n` with token `tok-n`, opening link n
    /// answers grant n, and the swap job has finished on its first poll.
    async fn swap_server() -> MockJanus {
        mock(Box::new(|path, count| {
            let json = |s: String| (200, "application/json", s.into_bytes());
            match path {
                "/api/v1/files/upload" => json(format!(
                    r#"{{"fileId":"file-{count}","filename":"f","mimeType":"video/mp4","size":4,"url":""}}"#
                )),
                "/api/v1/shares" => json(format!(
                    r#"{{"id":"share-{count}","url":"https://neboai.test/s/tok-{count}","filename":"f","access":"link","hasPassword":false,"expiresAt":"","createdAt":""}}"#
                )),
                "/api/v1/shares/open" => json(format!(
                    r#"{{"state":"ok","fileUrl":"/api/v1/files/grant-{count}?share=tok-{count}&exp=1&sig=s"}}"#
                )),
                p if p.starts_with("/api/v1/shares/share-") => json(r#"{"status":"revoked"}"#.to_string()),
                "/v1/videos" => json(r#"{"id":"vid_s","object":"video","status":"running","model":"nebo-video-swap"}"#.to_string()),
                "/v1/videos/vid_s" => json(r#"{"id":"vid_s","status":"succeeded"}"#.to_string()),
                "/v1/videos/vid_s/content" => (200, "video/mp4", b"SWAPPED".to_vec()),
                _ => (404, "text/plain", b"no".to_vec()),
            }
        }))
        .await
    }

    /// A tool over `server` (Janus and the hub) with its cast in
    /// `tmp/cast`, and the run's folder `tmp/work` holding `clip.mp4` and
    /// `maya.png`.
    fn swap_tool(server: &MockJanus, tmp: &Path) -> (Arc<GenerateMediaTool>, PathBuf) {
        let store = Arc::new(db::Store::new(&tmp.join("t.db").to_string_lossy()).unwrap());
        let hub = Arc::new(comm::api::NeboAIApi::new(server.url.clone(), "bot-1".into(), "token".into()));
        let media = Media::new(server.url.clone(), "bot-1".into(), None).with_polling(fast());
        let tool = GenerateMediaTool::new(media, store).with_cast_and_hub(tmp.join("cast"), hub);
        let work = tmp.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("clip.mp4"), b"CLIP").unwrap();
        std::fs::write(work.join("maya.png"), b"\x89PNG\r\n\x1a\nmaya").unwrap();
        (Arc::new(tool), work)
    }

    /// Cast member `name` with a hero image, confirmed by the owner when
    /// `attested`.
    fn cast_member(tmp: &Path, name: &str, attested: bool) {
        let dir = tmp.join("cast").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hero.png"), b"\x89PNG\r\n\x1a\nhero").unwrap();
        let member = cast::Member {
            name: name.to_string(),
            kind: attested.then(|| "release".to_string()),
            attestation: attested.then(|| cast::Attestation {
                id: "att-1".into(),
                by: "owner".into(),
                at: "2026-10-04T00:00:00Z".into(),
                statements: cast::statements(name),
            }),
            ..Default::default()
        };
        std::fs::write(dir.join(cast::CAST_JSON), serde_json::to_string(&member).unwrap()).unwrap();
    }

    fn in_folder(work: &Path) -> ToolContext {
        ToolContext { cwd: Some(work.to_string_lossy().into_owned()), ..Default::default() }
    }

    fn swap_input() -> Value {
        json!({"kind": "video", "mode": "replace", "video": "clip.mp4", "cast": "Maya", "into": "out/swapped.mp4"})
    }

    /// The request lines `server` saw, in order.
    fn lines(server: &MockJanus) -> Vec<String> {
        server.seen.lock().unwrap().iter().map(|(l, _, _)| l.clone()).collect()
    }

    /// The body Janus is sent is the contract, exactly: the swap model,
    /// replace, the clip's URL, the cast's images' URLs (hero first), 720p
    /// and the attestation id, and no prompt or length nobody gave. The
    /// result is a video card with the next steps named.
    #[tokio::test]
    async fn a_swap_sends_janus_exactly_the_contract() {
        let server = swap_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&server, tmp.path());
        cast_member(tmp.path(), "Maya", true);
        let input = swap_input();
        assert!(tool.validate_input(&input).is_ok());
        assert!(tool.emits_image(&input), "a swapped video is a card");

        let result = tool.execute_dyn(&in_folder(&work), input).await;
        assert!(!result.is_error, "{}", result.content);
        let file = work.join("out/swapped.mp4");
        assert_eq!(result.image_url.as_deref(), Some(file.to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(&file).unwrap(), b"SWAPPED");
        assert!(result.content.contains("audio-from") && result.content.contains("ai-generated"), "{}", result.content);
        assert!(result.content.contains("cast Maya"), "{}", result.content);

        let seen = server.seen.lock().unwrap();
        let (_, body, _) = seen.iter().find(|(l, _, _)| l.starts_with("POST /v1/videos ")).expect("submitted");
        let body: Value = serde_json::from_str(body).unwrap();
        let url = &server.url;
        assert_eq!(
            body,
            json!({
                "model": "nebo-video-swap",
                "mode": "replace",
                "video": format!("{url}/api/v1/files/grant-1?share=tok-1&exp=1&sig=s"),
                "references": [format!("{url}/api/v1/files/grant-2?share=tok-2&exp=1&sig=s")],
                "resolution": "720p",
                "attestation": "att-1",
            })
        );
        drop(seen);

        // A prompt is sent only when given; a named resolution wins.
        let body = swap_body(&json!({"prompt": "she smiles", "resolution": "1080p"}), "v", &["r".into()], "a");
        assert_eq!(body["prompt"], "she smiles");
        assert_eq!(body["resolution"], "1080p");
    }

    /// The local clip and the hero go up through the one upload path, each
    /// gets a link that lasts about an hour, Janus is sent what the links
    /// open to, and both links are turned off once the job has ended. A
    /// swap Janus refuses turns its links off too.
    #[tokio::test]
    async fn local_files_are_lent_by_link_and_the_links_turned_off() {
        let server = swap_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&server, tmp.path());
        cast_member(tmp.path(), "Maya", true);
        let result = tool.execute_dyn(&in_folder(&work), swap_input()).await;
        assert!(!result.is_error, "{}", result.content);

        let seen = lines(&server);
        let at = |prefix: &str| seen.iter().position(|l| l.starts_with(prefix)).unwrap_or_else(|| panic!("{prefix}: {seen:#?}"));
        assert_eq!(seen.iter().filter(|l| l.starts_with("POST /api/v1/files/upload ")).count(), 2, "{seen:#?}");
        assert_eq!(seen.iter().filter(|l| l.starts_with("POST /api/v1/shares ")).count(), 2, "{seen:#?}");
        assert_eq!(seen.iter().filter(|l| l.starts_with("POST /api/v1/shares/open ")).count(), 2, "{seen:#?}");
        let submitted = at("POST /v1/videos ");
        let finished = at("GET /v1/videos/vid_s ");
        assert!(at("DELETE /api/v1/shares/share-1 ") > finished && at("DELETE /api/v1/shares/share-2 ") > finished, "{seen:#?}");
        assert!(at("POST /api/v1/shares/open ") < submitted);

        let bodies = server.seen.lock().unwrap();
        let shares: Vec<Value> = bodies
            .iter()
            .filter(|(l, _, _)| l.starts_with("POST /api/v1/shares "))
            .map(|(_, b, _)| serde_json::from_str(b).unwrap())
            .collect();
        assert_eq!(shares[0]["fileId"], "file-1");
        assert_eq!(shares[1]["fileId"], "file-2");
        for share in &shares {
            assert_eq!(share["access"], "link");
            let until = chrono::DateTime::parse_from_rfc3339(share["expiresAt"].as_str().unwrap()).unwrap();
            let left = until.signed_duration_since(chrono::Utc::now());
            assert!(left > chrono::TimeDelta::minutes(55) && left <= chrono::TimeDelta::minutes(61), "{left}");
        }
        let opened: Value = serde_json::from_str(
            &bodies.iter().find(|(l, _, _)| l.starts_with("POST /api/v1/shares/open ")).unwrap().1,
        )
        .unwrap();
        assert_eq!(opened, json!({"token": "tok-1"}));
        let upload = &bodies.iter().find(|(l, _, _)| l.starts_with("POST /api/v1/files/upload ")).unwrap().1;
        assert!(upload.contains("filename=\"clip.mp4\"") && upload.contains("video/mp4") && upload.contains("CLIP"), "{upload}");
        drop(bodies);

        // Janus refuses the swap: nothing is made, and the links still go.
        let refusing = mock(Box::new(|path, count| match path {
            "/api/v1/files/upload" => (200, "application/json", format!(r#"{{"fileId":"file-{count}","filename":"f","mimeType":"video/mp4","size":4,"url":""}}"#).into_bytes()),
            "/api/v1/shares" => (200, "application/json", format!(r#"{{"id":"share-{count}","url":"https://neboai.test/s/tok-{count}"}}"#).into_bytes()),
            "/api/v1/shares/open" => (200, "application/json", br#"{"state":"ok","fileUrl":"/api/v1/files/g"}"#.to_vec()),
            "/v1/videos" => (400, "application/json", br#"{"error":{"message":"no person found in the clip"}}"#.to_vec()),
            _ => (200, "application/json", b"{}".to_vec()),
        }))
        .await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&refusing, tmp.path());
        cast_member(tmp.path(), "Maya", true);
        let result = tool.execute_dyn(&in_folder(&work), swap_input()).await;
        assert!(result.is_error && result.content.ends_with("no person found in the clip"), "{}", result.content);
        let seen = lines(&refusing);
        assert!(seen.iter().any(|l| l.starts_with("DELETE /api/v1/shares/share-1 ")), "{seen:#?}");
        assert!(seen.iter().any(|l| l.starts_with("DELETE /api/v1/shares/share-2 ")), "{seen:#?}");
        assert!(!work.join("out/swapped.mp4").exists());
    }

    /// A replace with no cast member, one nobody by that name, one the
    /// owner has not confirmed, or a face image passed in is refused with
    /// a plain reason before anything is uploaded or sent.
    #[tokio::test]
    async fn a_swap_without_a_confirmed_cast_is_refused_before_any_upload() {
        let server = swap_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&server, tmp.path());
        cast_member(tmp.path(), "Sam", false);
        cast_member(tmp.path(), "Maya", true);
        let ctx = in_folder(&work);

        let no_cast = json!({"kind": "video", "mode": "replace", "video": "clip.mp4"});
        let err = tool.validate_input(&no_cast).unwrap_err();
        assert!(err.contains("Give `cast`"), "{err}");
        let result = tool.execute_dyn(&ctx, no_cast).await;
        assert!(result.is_error && result.content.contains("Give `cast`"), "{}", result.content);

        let nobody = json!({"kind": "video", "mode": "replace", "video": "clip.mp4", "cast": "Nobody"});
        let result = tool.execute_dyn(&ctx, nobody).await;
        assert!(result.is_error && result.content.contains("no cast member named Nobody"), "{}", result.content);

        let unconfirmed = json!({"kind": "video", "mode": "replace", "video": "clip.mp4", "cast": "Sam"});
        let result = tool.execute_dyn(&ctx, unconfirmed).await;
        assert!(result.is_error, "{}", result.content);
        assert!(
            result.content.starts_with("Sam can't be used to replace a person in a video yet: the owner hasn't confirmed"),
            "{}",
            result.content
        );

        let a_face = json!({"kind": "video", "mode": "replace", "video": "clip.mp4", "cast": "Maya", "image": "maya.png"});
        let result = tool.execute_dyn(&ctx, a_face).await;
        assert!(result.is_error && result.content.contains("only from the cast"), "{}", result.content);

        assert!(lines(&server).is_empty(), "nothing reached the hub or Janus: {:#?}", lines(&server));
        assert!(!work.join("out").exists(), "no file was started");
    }

    /// Creating a cast member shows the owner the one-time card in the
    /// chat that asked; his answer is kept in cast.json with the
    /// statements he confirmed and an id. "No", or nobody here to answer,
    /// leaves the member unconfirmed.
    #[tokio::test]
    async fn the_attestation_card_stores_cast_json() {
        let server = swap_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&server, tmp.path());

        // The owner, in his chat, picks "An AI persona".
        let ask = |answer: &'static str, input: Value| {
            let tool = tool.clone();
            let work = work.clone();
            async move {
                let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(4);
                let channels: crate::origin::AskChannels = Default::default();
                let mut ctx = in_folder(&work);
                ctx.origin = crate::origin::Origin::User;
                ctx.stream_tx = Some(stream_tx);
                ctx.ask_channels = Some(channels.clone());
                assert!(!tool.emits_image(&input), "a cast change is no card of its own");
                let running = tokio::spawn(async move { tool.execute_dyn(&ctx, input).await });
                let card = stream_rx.recv().await.expect("the attestation card");
                assert_eq!(card.event_type, ai::StreamEventType::AskRequest);
                let id = card.error.clone().unwrap();
                channels.lock().await.remove(&id).unwrap().send(answer.to_string()).unwrap();
                (card, running.await.unwrap())
            }
        };

        let (card, result) = ask("An AI persona", json!({"kind": "cast", "cast": "Maya", "image": "maya.png"})).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("The owner confirmed Maya"), "{}", result.content);
        assert_eq!(card.text, cast::question("Maya"));
        for statement in cast::statements("Maya") {
            assert!(card.text.contains(&statement), "{}", card.text);
        }
        assert!(card.text.contains("not a public figure or a celebrity lookalike"));
        assert!(card.text.contains("testimonial"));
        assert!(card.text.contains("labelled as AI-generated where the law requires"));
        assert_eq!(
            card.widgets.unwrap()[0]["options"],
            json!(["Someone who gave consent or a release", "An AI persona", "Me", "No, don't use them"])
        );
        let dir = tmp.path().join("cast/Maya");
        assert_eq!(std::fs::read(dir.join("hero.png")).unwrap(), b"\x89PNG\r\n\x1a\nmaya");
        let kept: cast::Member = serde_json::from_str(&std::fs::read_to_string(dir.join("cast.json")).unwrap()).unwrap();
        assert_eq!(kept.name, "Maya");
        assert_eq!(kept.kind.as_deref(), Some("ai_persona"));
        let attestation = kept.attestation.expect("attested");
        assert!(uuid::Uuid::parse_str(&attestation.id).is_ok(), "{}", attestation.id);
        assert_eq!(attestation.by, "owner");
        assert!(chrono::DateTime::parse_from_rfc3339(&attestation.at).is_ok());
        assert_eq!(attestation.statements, cast::statements("Maya"));
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("cast.json")).unwrap()).unwrap();
        assert_eq!(raw["type"], "ai_persona", "{raw}");
        assert!(raw["hub_file_ids"].is_object(), "{raw}");

        // "No": kept in the cast, never usable for a swap.
        let (_, result) = ask("No, don't use them", json!({"kind": "cast", "cast": "Sam", "image": "maya.png"})).await;
        assert!(result.content.contains("did not confirm Sam"), "{}", result.content);
        let sam: cast::Member =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("cast/Sam/cast.json")).unwrap()).unwrap();
        assert!(sam.attestation.is_none() && sam.kind.is_none());

        // Naming Sam again, the owner in his chat: the card again, and now "Me".
        let (_, result) = ask("Me", json!({"kind": "cast", "cast": "Sam"})).await;
        assert!(result.content.contains("The owner confirmed Sam"), "{}", result.content);
        let sam: cast::Member =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("cast/Sam/cast.json")).unwrap()).unwrap();
        assert_eq!(sam.kind.as_deref(), Some("owner"));
        assert!(sam.attestation.is_some());

        // Nobody here to answer: made, unconfirmed, and told how to confirm.
        let result = tool
            .execute_dyn(&in_folder(&work), json!({"kind": "cast", "cast": "Lee", "image": "maya.png"}))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("kind \"cast\" with cast \"Lee\""), "{}", result.content);

        let list = tool.execute_dyn(&in_folder(&work), json!({"kind": "cast"})).await;
        assert!(list.content.contains("- Maya: confirmed by the owner (ai_persona); images hero.png"), "{}", list.content);
        assert!(list.content.contains("- Lee: not confirmed by the owner yet"), "{}", list.content);

        // A new image of a confirmed member joins only with the owner's
        // yes again: nobody here refuses it, his card adds it and renews
        // the attestation. The hero stays first.
        let refused = tool
            .execute_dyn(&in_folder(&work), json!({"kind": "cast", "cast": "Maya", "image": "maya.png"}))
            .await;
        assert!(refused.is_error && refused.content.starts_with("The image was not added"), "{}", refused.content);
        assert!(!dir.join("angle-1.png").exists());
        let before = attestation.id.clone();
        let (_, add) = ask("An AI persona", json!({"kind": "cast", "cast": "Maya", "image": "maya.png"})).await;
        assert!(add.content.contains("angle-1.png"), "{}", add.content);
        let kept: cast::Member = serde_json::from_str(&std::fs::read_to_string(dir.join("cast.json")).unwrap()).unwrap();
        assert_ne!(kept.attestation.unwrap().id, before, "a renewed attestation");
        let c = tool.cast().unwrap();
        let names: Vec<String> =
            c.images(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["hero.png", "angle-1.png"]);
        assert!(lines(&server).is_empty(), "the cast is local until a swap: {:#?}", lines(&server));
    }

    /// A cast image goes up once: its file id is kept in cast.json and the
    /// next swap links that copy; only the new clip is uploaded again.
    #[tokio::test]
    async fn a_cast_image_is_uploaded_once_and_reused() {
        let server = swap_server().await;
        let tmp = tempfile::tempdir().unwrap();
        let (tool, work) = swap_tool(&server, tmp.path());
        cast_member(tmp.path(), "Maya", true);
        let uploads = || lines(&server).iter().filter(|l| l.starts_with("POST /api/v1/files/upload ")).count();

        let result = tool.execute_dyn(&in_folder(&work), swap_input()).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(uploads(), 2, "the clip and the hero");
        let kept: cast::Member =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("cast/Maya/cast.json")).unwrap()).unwrap();
        assert_eq!(kept.hub_file_ids.get("hero.png").map(String::as_str), Some("file-2"));
        assert_eq!(kept.attestation.as_ref().unwrap().id, "att-1", "the attestation is untouched");

        let again = json!({"kind": "video", "mode": "replace", "video": "clip.mp4", "cast": "maya", "into": "out/second.mp4"});
        let result = tool.execute_dyn(&in_folder(&work), again).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(uploads(), 3, "only the clip went up again");
        let seen = server.seen.lock().unwrap();
        let linked: Vec<Value> = seen
            .iter()
            .filter(|(l, _, _)| l.starts_with("POST /api/v1/shares "))
            .map(|(_, b, _)| serde_json::from_str::<Value>(b).unwrap())
            .collect();
        assert_eq!(linked.len(), 4);
        assert_eq!(linked[3]["fileId"], "file-2", "the stored hero is linked again");
        assert_eq!(seen.iter().filter(|(l, _, _)| l.starts_with("DELETE /api/v1/shares/")).count(), 4);
    }

    /// A replace is waited on three times as long as other video.
    #[test]
    fn a_swap_waits_longer_than_other_video() {
        assert_eq!(Polling::default().limit * SWAP_WAIT_FACTOR, Duration::from_secs(30 * 60));
    }
}
