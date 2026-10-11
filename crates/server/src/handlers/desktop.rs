//! GET /ws/desktop — the live view of the bot's computer.
//!
//! A thin byte pipe between the browser (noVNC speaking RFB over binary
//! WebSocket frames) and the session's loopback x11vnc. The desktop starts
//! on demand when the first viewer connects; each connection holds a
//! ViewerGuard so the idle reaper leaves a watched desktop alone. Auth is
//! the same trust gate as /ws: locally that's the trusted-origin check, and
//! through the tunnel neboai.com has already authenticated the caller and
//! stripped the Origin header.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::extract::{Multipart, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use crate::state::AppState;

fn teach_err(e: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({ "error": e })),
    )
        .into_response()
}

/// How long past the cap the server waits for the app's own stop, which
/// carries the owner's narration, before it stops a recording itself. A
/// computer nobody is watching any more is stopped right at the cap.
const TEACH_STOP_GRACE: Duration = Duration::from_secs(60);

/// How long a taught session's turns build on the strongest intelligence
/// after its learning run starts: the learning run, and the turns that make
/// a workflow or a task from what it learned.
const TEACH_BUILD_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Who the recording under way is for, as its start named them: where the
/// server's own stop at the cap sends the learning run.
struct Teaching {
    id: String,
    agent_id: String,
    session_key: String,
}

fn teaching() -> &'static Mutex<Option<Teaching>> {
    static TEACHING: OnceLock<Mutex<Option<Teaching>>> = OnceLock::new();
    TEACHING.get_or_init(|| Mutex::new(None))
}

/// The last recording the server stopped at its cap, and what that stop
/// answered: the app's own stop arriving after it gets the same answer.
fn capped() -> &'static Mutex<Option<Stopped>> {
    static CAPPED: OnceLock<Mutex<Option<Stopped>>> = OnceLock::new();
    CAPPED.get_or_init(|| Mutex::new(None))
}

/// POST /api/v1/desktop/teach/start — begin recording a demonstration.
/// `where` picks the screen: "local" (this computer's own screen, the default
/// on a bot that has one) or "computer" (the bot's virtual desktop, the
/// default and only choice on a cloud bot; started if needed). `agentId` and
/// `sessionKey` say whose it is, so a recording the app never stops is still
/// learned from when the server stops it at the cap. `capSeconds` in the
/// answer is that cap.
pub async fn teach_start(
    State(state): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let requested = body.as_ref().and_then(|b| b["where"].as_str());
    let where_ = match tools::desktop_session::TeachWhere::resolve(
        requested,
        tools::desktop_session::own_screen(),
    ) {
        Ok(w) => w,
        Err(e) => return teach_err(&e),
    };
    if where_ == tools::desktop_session::TeachWhere::Computer
        && let Err(e) = tools::desktop_session::ensure_started().await
    {
        return teach_err(&e);
    }
    // The owner's replays and voice leave the disk after their week.
    tokio::task::spawn_blocking(|| tools::desktop_session::sweep_teach_media(tools::desktop_session::TEACH_VIDEO_KEEP));
    let (id, dir) = match tools::desktop_session::start_recording(where_).await {
        Ok(started) => started,
        // Missing Screen Recording permission is named, so the app can offer
        // the settings pane instead of a bare error.
        Err(e) if e == tools::desktop_session::SCREEN_RECORDING_PERMISSION => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({ "error": e, "reason": "screen_recording_permission" })),
            )
                .into_response();
        }
        Err(e) => {
            warn!(error = %e, where_ = where_.as_str(), "teach: recording did not start");
            return teach_err(&e);
        }
    };
    // Every browser with the extension records each action as the element it
    // was done to, on the recording's clock.
    if let Some(started_ms) = tools::desktop_session::recording_started_ms() {
        state.extension_bridge.teach_start(&id, dir.clone(), started_ms).await;
    }
    let field = |k: &str| body.as_ref().and_then(|b| b[k].as_str()).unwrap_or("").to_string();
    *teaching().lock().unwrap_or_else(|p| p.into_inner()) =
        Some(Teaching { id: id.clone(), agent_id: field("agentId"), session_key: field("sessionKey") });
    let st = state.clone();
    let capped_id = id.clone();
    tokio::spawn(async move { stop_at_cap(&st, &capped_id, where_).await });
    axum::Json(serde_json::json!({
        "sessionId": id,
        "dir": dir.to_string_lossy(),
        "where": where_.as_str(),
        "capSeconds": tools::desktop_session::TEACH_CAP.as_secs(),
    }))
    .into_response()
}

/// The server's own stop: at the cap, a recording still running is stopped
/// and learned from exactly as the app's stop would, so a closed app never
/// leaves one running. The app stops at the cap itself, with the owner's
/// narration; the server gives it [`TEACH_STOP_GRACE`] to, unless nobody is
/// watching the computer any more.
async fn stop_at_cap(state: &AppState, id: &str, where_: tools::desktop_session::TeachWhere) {
    tokio::time::sleep(tools::desktop_session::TEACH_CAP).await;
    let watched = where_ == tools::desktop_session::TeachWhere::Local || tools::desktop_session::viewers() > 0;
    if watched {
        tokio::time::sleep(TEACH_STOP_GRACE).await;
    }
    if tools::desktop_session::recording().await.is_none_or(|(running, _)| running != id) {
        return;
    }
    let who = {
        let mut held = teaching().lock().unwrap_or_else(|p| p.into_inner());
        if held.as_ref().is_some_and(|t| t.id == id) { held.take() } else { None }
    };
    let (agent_id, session_key) = who.map(|t| (t.agent_id, t.session_key)).unwrap_or_default();
    info!(session = %id, "teach: the recording reached its cap; stopping it");
    if agent_id.is_empty() {
        // Nobody to learn it: the recording still ends.
        stop_element_recording(state, Some(id)).await;
        if let Err(e) = tools::desktop_session::stop_recording(Some(id)).await {
            warn!(session = %id, error = %e, "teach: the capped recording did not stop");
        }
        return;
    }
    match finish(state, Some(id), &agent_id, &session_key, None).await {
        Ok(stopped) => *capped().lock().unwrap_or_else(|p| p.into_inner()) = Some(stopped),
        Err(e) => warn!(session = %id, error = %e, "teach: the capped recording did not stop"),
    }
}

/// End the extension's element recording (`only`: just that session) and
/// write its `elements.jsonl` and `actions.json` beside the recording.
async fn stop_element_recording(state: &AppState, only: Option<&str>) {
    match state.extension_bridge.teach_stop(only).await {
        Some(Ok(n)) => info!(actions = n, "teach: element records written"),
        Some(Err(e)) => warn!(error = %e, "teach: element records were not written"),
        None => {}
    }
}

/// The owner's narration as the app sent it: the audio, and how far into
/// the recording the microphone started.
struct NarrationUpload {
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
    offset: Duration,
}

/// The extension the narration is saved under: its own when the
/// transcriber reads it, else one its type names.
fn narration_ext(filename: &str, mime_type: &str) -> &'static str {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if let Some(known) = ai::transcribe::SUPPORTED_AUDIO_EXTENSIONS.iter().find(|k| **k == ext) {
        return known;
    }
    match mime_type.split(';').next().unwrap_or("").trim() {
        "audio/webm" | "video/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        _ => "m4a",
    }
}

/// What the learner hears about the owner's narration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Narration {
    /// None was recorded, or it held no speech.
    None,
    /// The owner's words were written into this file (timeline.md or
    /// narration.md).
    Into(&'static str),
    /// It was recorded but could not be transcribed.
    Failed,
}

/// The briefing the learning run reads (the `mention_context` rail): where
/// the recording is, how to study it, what the narration is, and what to do
/// with what it learns.
fn teach_briefing(rec: &tools::desktop_session::Finished, has_timeline: bool, narration: Narration) -> String {
    let keyframes = rec.keyframes;
    // The virtual computer logs every click and keystroke into timeline.md;
    // a recording of the owner's own screen is keyframes only.
    let study = if has_timeline {
        format!(
            "Start with timeline.md — the reconstructed click-and-keystroke timeline of \
             exactly what they did — then confirm the visual context by viewing 5-6 spread \
             keyframes from frames/ (there are {keyframes}; do NOT read them all, and do not \
             use sub-agents)."
        )
    } else {
        format!(
            "There is no click-and-keystroke timeline for this one: it was recorded as \
             screenshots of their screen. View about 8 keyframes from frames/, spread from \
             first to last (there are {keyframes}, numbered in order; do NOT read them all, \
             and do not use sub-agents), and work out the steps from what changes between them."
        )
    };
    let narrated = match narration {
        Narration::None => String::new(),
        Narration::Into("timeline.md") => " They narrated as they worked: what they said is in timeline.md \
             as `owner said: \"…\"` lines, in time order beside what they did."
            .to_string(),
        Narration::Into(file) => format!(
            " They narrated as they worked: {file} has what they said in time order, each line \
             naming the keyframe on screen as they said it — read it first, and include the \
             keyframes it names in the ones you view."
        ),
        Narration::Failed => " They also narrated as they worked, but the narration could not be \
             transcribed, so learn from the recording alone and say so in one line of your reply."
            .to_string(),
    };
    let intent = if matches!(narration, Narration::Into(_)) {
        " The narration is the owner's explicit intent: it says why each step happens and \
         which inputs vary from run to run; the clicks, their order and the screens are the \
         implicit part and show how. Where the two conflict, the narration wins; where it is \
         silent, infer from the actions."
    } else {
        " The clicks, their order and the screens are all you have of their intent: infer it \
         from them."
    };
    format!(
        "The owner just demonstrated a task on this computer (teach session {id}). The \
         recording is in {dir}. {study}{narrated}{intent}\n\n\
         Learn the shape of the task, not this one instance. Every concrete value in the \
         demonstration is a variable by default: file names, customer and account names, \
         dates, amounts, row positions, search terms, URLs with ids in them. What you build \
         must work again when the file or the name is different. A value is fixed only when \
         the owner says so (\"always this customer\", \"only this account\", \"always this \
         folder\"); their words pinning a value override the default.\n\n\
         Credentials: never write a password, token or other secret into anything you \
         build or into your reply — the timeline shows them as [typed secret] or [pasted \
         text], and that is all you may know of them. Declare each one the task needs in \
         the skill's frontmatter (metadata: secrets:, with a key such as \
         ORDERS_PORTAL_PASSWORD and a label) and refer to it by name as ${{secret.KEY}} in \
         the steps; a username may be written down only when the owner pinned it. When a \
         credential is not saved yet, ask the owner to save it in Settings > Secrets; never \
         ask them to paste it into the chat.\n\n\
         Write each step so it can be done again and checked, in this shape:\n\
         `N. <verb> <role> \"<name>\" — locators: role=…[name=\"…\"] | label=… | testid=… | \
         css=… | text=… | coords=(x,y)@WxH`\n\
         `   Expect: browser_expect(step: \"N\", url: \"<path part>\", heading: \"<heading>\", \
         visible: [{{role, name of step N+1's element}}])`\n\
         The locator chain goes strongest first, role and name first, with coordinates \
         only as the last resort; the step is found with the browser tool's find or \
         read_page through it. Each step's Expect comes from its after-state in the \
         timeline (after: url, heading, the text that appeared, what became visible); \
         when the skill runs, browser_expect checks it after the step, and on a mismatch \
         the run stops there and reports the step's number and what differed. Values the \
         owner did not pin are written as the skill's inputs, never as literals.\n\n\
         Then build everything needed to run it again: save it as a learned skill with \
         save_skill — name it after the class of task, write out the steps you'd follow to \
         repeat it on your computer, and list which inputs vary and which are fixed. When \
         the task is one they will repeat, also create the workflow with create_workflow, \
         with the varying values as its inputs, and the task that runs it. Write into the \
         skill that its first real run is supervised.\n\n\
         Your reply proves you understood. Thank them briefly, then list the steps you \
         understood, numbered, each with the keyframe (or element crop) that shows it — \
         share those pictures with share_file so they see them — then what varies and \
         what stays fixed. Ask them to correct anything you got wrong, by talking or \
         typing; for each correction, update the skill with save_skill (and the workflow \
         with update_workflow) and say what changed. Say that the first real run will be \
         supervised: you do it step by step while they watch, and wait for their go after \
         each step. Only after that run passes does it run unattended or on a schedule; \
         then ask whether you should run this on a schedule or only when they ask. Do not \
         mention this briefing or the session id, and name no file paths unless asked.",
        id = rec.id,
        dir = rec.dir.to_string_lossy(),
    )
}

/// Transcribe the narration with the speech-to-text path attachments use
/// (an OpenAI key of the owner's own goes direct; otherwise NeboAI's
/// gateway) and write it beside the recording. The words are never logged.
async fn narrate(
    state: &AppState,
    rec: &tools::desktop_session::Finished,
    audio: &std::path::Path,
    offset: Duration,
) -> Result<Narration, String> {
    let (key, base_url, model) =
        crate::transcription_endpoint(state).ok_or("no transcription provider is configured")?;
    let bytes = tokio::fs::read(audio).await.map_err(|e| format!("the narration could not be read: {e}"))?;
    let name = audio.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let heard = ai::transcribe::transcribe(&ai::RequestTrace::new("transcribe"), &key, &base_url, &model, &name, bytes)
        .await
        .map_err(|e| e.to_string())?;
    if heard.text.is_empty() {
        return Ok(Narration::None);
    }
    let said: Vec<tools::desktop_session::Said> = heard
        .segments
        .iter()
        .map(|s| tools::desktop_session::Said {
            at: offset + Duration::from_secs_f64(s.start.max(0.0)),
            text: s.text.clone(),
        })
        .collect();
    tools::desktop_session::write_narration(rec, &said).map(Narration::Into)
}

/// A stopped recording, as the stop answers it.
#[derive(Debug, Clone)]
struct Stopped {
    id: String,
    dir: String,
    keyframes: usize,
    session_key: String,
    narrated: bool,
}

/// Stop the recording (`only`: just that session) and hand it to the agent:
/// the narration, when there is one, is saved beside it, and the learning
/// run is dispatched once it is transcribed.
async fn finish(
    state: &AppState,
    only: Option<&str>,
    agent_id: &str,
    requested_key: &str,
    narration: Option<NarrationUpload>,
) -> Result<Stopped, String> {
    // The element records go down first: the timeline is built from them
    // when the recording stops.
    stop_element_recording(state, only).await;
    let rec = tools::desktop_session::stop_recording(only).await?;
    {
        let mut held = teaching().lock().unwrap_or_else(|p| p.into_inner());
        if held.as_ref().is_some_and(|t| t.id == rec.id) {
            *held = None;
        }
    }
    // Route to the caller's thread when it names one of THIS agent's sessions;
    // otherwise start a fresh thread so the exchange has a home.
    let session_key = if !requested_key.is_empty()
        && requested_key.starts_with(&types::keyparser::agent_session_prefix(agent_id))
    {
        requested_key.to_string()
    } else {
        crate::handlers::agents::create_agent_thread(state, agent_id)
            .map(|(_chat, key)| key)
            .map_err(|e| e.to_string())?
    };
    let audio = match narration.filter(|n| !n.bytes.is_empty()) {
        Some(n) => {
            let path = rec.dir.join(format!("narration.{}", narration_ext(&n.filename, &n.mime_type)));
            match tokio::fs::write(&path, &n.bytes).await {
                Ok(()) => Some((path, n.offset)),
                Err(e) => {
                    warn!(session = %rec.id, error = %e, "teach: the narration could not be saved");
                    None
                }
            }
        }
        None => None,
    };
    let answer = Stopped {
        id: rec.id.clone(),
        dir: rec.dir.to_string_lossy().into_owned(),
        keyframes: rec.keyframes,
        session_key: session_key.clone(),
        narrated: audio.is_some(),
    };
    let st = state.clone();
    let agent_id = agent_id.to_string();
    tokio::spawn(async move { learn(&st, rec, &agent_id, session_key, audio).await });
    Ok(answer)
}

const TEACH_VISIBLE: &str = "I just demonstrated a task for you on your computer — \
    watch the recording back and learn how to do it.";

/// The learning run: the narration transcribed (a failure is one line of
/// the reply, never a stop), then the run dispatched HERE on the deepest
/// intelligence. The engine owns both the visible message and the steering
/// briefing: the chat shows one human sentence, and the operational
/// instructions ride `mention_context` — the existing ephemeral
/// `<system-reminder>` rail — so they reach the model without ever being
/// rendered or persisted.
async fn learn(
    state: &AppState,
    rec: tools::desktop_session::Finished,
    agent_id: &str,
    session_key: String,
    audio: Option<(std::path::PathBuf, Duration)>,
) {
    let (narration, transcribed) = match &audio {
        None => (Narration::None, false),
        Some((path, offset)) => match narrate(state, &rec, path, *offset).await {
            Ok(n) => (n, true),
            Err(e) => {
                warn!(session = %rec.id, error = %e, "teach: the narration was not transcribed; learning without it");
                (Narration::Failed, false)
            }
        },
    };
    // The owner's replay, the narration in it. The learner never reads it.
    let replay = match tools::desktop_session::finish_video(&rec, audio.as_ref().map(|(p, o)| (p.as_path(), *o))).await {
        Ok(path) => Some(path),
        Err(e) => {
            warn!(session = %rec.id, error = %e, "teach: no replay was made");
            None
        }
    };
    // The raw narration leaves once it is both transcribed and in the
    // replay; otherwise the week's sweep takes it.
    if let Some((path, _)) = &audio
        && transcribed
        && replay.is_some()
    {
        let _ = tokio::fs::remove_file(path).await;
    }
    let attachments = replay
        .as_ref()
        .map(|path| {
            vec![comm::wire::Attachment {
                file_id: String::new(),
                filename: REPLAY_NAME.to_string(),
                mime_type: "video/mp4".to_string(),
                size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                url: replay_url(&rec.id),
                thumbnail_url: None,
                width: None,
                height: None,
                duration: Some(rec.length.as_secs_f64()),
            }]
        })
        .unwrap_or_default();
    let briefing = teach_briefing(&rec, rec.dir.join("timeline.md").exists(), narration);
    // Training uses the deepest intelligence: the run, and the turns that
    // follow it in this thread, build from their first step on the build
    // lane of the employee's pack (Nebo AI's: Deep, which Janus serves on
    // its strongest model). The owner's own setting is untouched.
    state.harness.build_session(&session_key, TEACH_BUILD_WINDOW);
    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", agent_id);
    let config = crate::chat_dispatch::ChatConfig {
        session_key: session_key.clone(),
        prompt: TEACH_VISIBLE.to_string(),
        user_id: String::new(),
        channel: "web".to_string(),
        origin: tools::Origin::User,
        door: types::permissions::Door::Chat,
        narrow: None,
        agent_id: agent_id.to_string(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        // The owner's replay, playable on the message that started it.
        attachments,
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: Some(briefing),
        tool_scope: None,
        channel_ctx: None,
        handoff_depth: 0,
        seed_taint: vec![],
        tool_allowlist: None,
        hidden_prompt: false,
        coworker: None,
        audience: None,
        cwd: None,
        model_override: None,
        client_id: None,
        platform: None,
        message_id: None,
    };
    crate::chat_dispatch::run_chat(state, config).await;
    if replay.is_some() {
        offload_replay(state, &rec, &session_key).await;
    }
}

/// What the owner's replay is called on the teach message.
const REPLAY_NAME: &str = "demonstration.mp4";

/// Where the owner plays a teach session's replay.
fn replay_url(id: &str) -> String {
    format!("/api/v1/desktop/teach/{id}/video")
}

/// Where a replay offloaded to NeboAI is recorded: its file id there.
const REPLAY_STORED: &str = "video.json";

/// How long the learning run gets before the replay leaves this disk anyway.
const LEARNING_RUN_WAIT: Duration = Duration::from_secs(2 * 3600);

/// Once the learning run finishes, the replay leaves this disk for NeboAI's
/// storage (the teach-recordings shelf, kept a week there), and the owner
/// plays it through a short-lived signed link. Not signed in, or the upload
/// failing, it stays here until the week's sweep.
async fn offload_replay(state: &AppState, rec: &tools::desktop_session::Finished, session_key: &str) {
    // The run is queued: wait for it to start, then to finish.
    let started = tokio::time::Instant::now();
    while !state.run_registry.is_session_active(session_key).await && started.elapsed() < Duration::from_secs(60) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    while state.run_registry.is_session_active(session_key).await && started.elapsed() < LEARNING_RUN_WAIT {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let path = rec.dir.join(tools::desktop_session::VIDEO);
    let Ok(api) = crate::codes::build_api_client(state) else { return };
    let Ok(bytes) = tokio::fs::read(&path).await else { return };
    let fields = [("purpose".to_string(), "teach".to_string()), ("session".to_string(), rec.id.clone())];
    match api.upload_file(tools::desktop_session::VIDEO, "video/mp4", bytes, &fields).await {
        Ok(stored) => {
            let record = serde_json::json!({ "fileId": stored.file_id }).to_string();
            if let Err(e) = tokio::fs::write(rec.dir.join(REPLAY_STORED), record).await {
                warn!(session = %rec.id, error = %e, "teach: the offloaded replay was not recorded; keeping it here");
                return;
            }
            let _ = tokio::fs::remove_file(&path).await;
            info!(session = %rec.id, "teach: the replay moved to NeboAI storage");
        }
        Err(e) => warn!(session = %rec.id, error = %e, "teach: the replay stays on this disk"),
    }
}

/// GET /api/v1/desktop/teach/{id}/video — the owner's replay of a teach
/// session: from this disk while it is here, else a redirect to a
/// short-lived signed link to where it was offloaded.
pub async fn teach_video(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: axum::extract::Request,
) -> Response {
    let Some(dir) = tools::desktop_session::session_dir(&id) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let local = dir.join(tools::desktop_session::VIDEO);
    if local.exists() {
        return crate::handlers::files::stream_file(&local, "video/mp4", request).await;
    }
    let stored = std::fs::read_to_string(dir.join(REPLAY_STORED))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v["fileId"].as_str().map(str::to_string));
    let Some(file_id) = stored else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let link = match crate::codes::build_api_client(&state) {
        Ok(api) => api.file_link(&file_id).await,
        Err(e) => return teach_err(&e.to_string()),
    };
    match link {
        Ok(url) => axum::response::Redirect::temporary(&url).into_response(),
        // Past its week the replay is gone; say so rather than fail.
        Err(comm::CommError::Http { status: 410, .. }) => axum::http::StatusCode::GONE.into_response(),
        Err(e) => {
            warn!(session = %id, error = %e, "teach: no link to the replay");
            axum::http::StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// POST /api/v1/desktop/teach/stop — finalize the recording and hand the
/// session to the agent (see [`finish`]). Multipart: `agentId`, `sessionKey`,
/// `sessionId` (the recording, from start), and, when the owner narrated,
/// `narration` (the audio) with `narrationOffsetMs` (how far into the
/// recording the microphone started). A recording the server already
/// stopped at its cap answers with that stop's answer.
pub async fn teach_stop(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut agent_id = String::new();
    let mut requested_key = String::new();
    let mut recording_id = String::new();
    let mut offset_ms: u64 = 0;
    let mut narration: Option<NarrationUpload> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return teach_err(&format!("the stop could not be read: {e}")),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "narration" {
            let filename = field.file_name().unwrap_or("narration").to_string();
            let mime_type = field.content_type().unwrap_or("").to_string();
            match field.bytes().await {
                Ok(b) => narration = Some(NarrationUpload { filename, mime_type, bytes: b.to_vec(), offset: Duration::ZERO }),
                Err(e) => return teach_err(&format!("the narration could not be read: {e}")),
            }
            continue;
        }
        let text = field.text().await.unwrap_or_default();
        match name.as_str() {
            "agentId" => agent_id = text,
            "sessionKey" => requested_key = text,
            "sessionId" => recording_id = text,
            "narrationOffsetMs" => offset_ms = text.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    // Validate BEFORE finalizing: a bad request must not orphan a finished
    // recording — the session stays live and stop can be retried correctly.
    if agent_id.is_empty() {
        return teach_err("agentId is required");
    }
    if let Some(n) = narration.as_mut() {
        n.offset = Duration::from_millis(offset_ms);
    }
    let stopped = match finish(&state, None, &agent_id, &requested_key, narration).await {
        Ok(stopped) => (stopped, false),
        // The server stopped it at the cap before this arrived: the same
        // answer, said so.
        Err(e) if e == tools::desktop_session::NOT_RECORDING => {
            let held = capped().lock().unwrap_or_else(|p| p.into_inner()).clone();
            match held.filter(|s| !recording_id.is_empty() && s.id == recording_id) {
                Some(stopped) => (stopped, true),
                None => return teach_err(&e),
            }
        }
        Err(e) => return teach_err(&e),
    };
    let (stopped, capped) = stopped;
    axum::Json(serde_json::json!({
        "sessionId": stopped.id,
        "dir": stopped.dir,
        "keyframes": stopped.keyframes,
        // Server-owned copy of the persisted user message, so the client can
        // echo it optimistically without a second source of truth to drift.
        "message": TEACH_VISIBLE,
        "sessionKey": stopped.session_key,
        "narrated": stopped.narrated,
        "capped": capped,
    }))
    .into_response()
}

pub async fn desktop_ws_handler(
    State(_state): State<AppState>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !super::ws::origin_is_trusted(&headers) {
        warn!("ws/desktop: rejected upgrade from untrusted origin");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    if !cfg!(target_os = "linux") {
        // The on-demand desktop exists only in the cloud image; desktop
        // installs already have a real screen.
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(handle_desktop_ws)
}

async fn handle_desktop_ws(socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let port = match tools::desktop_session::ensure_started().await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "desktop session failed to start");
            let _ = ws_tx
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 1011,
                    reason: format!("desktop unavailable: {e}").into(),
                })))
                .await;
            return;
        }
    };

    // x11vnc may need a beat to bind after spawn.
    let mut tcp = None;
    for _ in 0..20 {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(150)).await,
        }
    }
    let Some(tcp) = tcp else {
        warn!("ws/desktop: could not reach x11vnc");
        let _ = ws_tx
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 1011,
                reason: "desktop stream unavailable".into(),
            })))
            .await;
        return;
    };
    let _ = tcp.set_nodelay(true);
    let (mut tcp_r, mut tcp_w) = tcp.into_split();

    let _viewer = tools::desktop_session::viewer_connected();
    info!("ws/desktop: viewer connected");

    let mut to_client = tokio::spawn(async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match tcp_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = &mut to_client => break,
            msg = ws_rx.next() => match msg {
                // RFB client → server bytes; every message is user activity.
                Some(Ok(Message::Binary(data))) => {
                    tools::desktop_session::touch();
                    if tcp_w.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    to_client.abort();
    info!("ws/desktop: viewer disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> tools::desktop_session::Finished {
        tools::desktop_session::Finished {
            id: "s1".into(),
            dir: "/home/bot/teach-sessions/s1".into(),
            keyframes: 40,
            length: Duration::from_secs(90),
            frames: vec![],
        }
    }

    /// Narration is the owner's intent: why, and which inputs vary; the
    /// clicks show how. Where it lives is named.
    #[test]
    fn the_briefing_treats_narration_as_the_owners_intent() {
        let folded = teach_briefing(&rec(), true, Narration::Into("timeline.md"));
        assert!(folded.contains("owner said"), "{folded}");
        assert!(folded.contains("the owner's explicit intent"), "{folded}");
        assert!(folded.contains("which inputs vary"), "{folded}");
        assert!(folded.contains("show how"), "{folded}");
        assert!(folded.contains("the narration wins"), "{folded}");
        assert!(folded.contains("infer from the actions"), "{folded}");

        let alone = teach_briefing(&rec(), false, Narration::Into("narration.md"));
        assert!(alone.contains("narration.md") && alone.contains("keyframe on screen"), "{alone}");
        assert!(alone.contains("the owner's explicit intent"), "{alone}");

        let none = teach_briefing(&rec(), true, Narration::None);
        assert!(!none.contains("narrat"), "{none}");
    }

    /// The shape, not the instance: every concrete value varies unless the
    /// owner pins it, and the learner says which is which.
    #[test]
    fn the_briefing_makes_every_value_a_variable_by_default() {
        for n in [Narration::None, Narration::Into("timeline.md"), Narration::Failed] {
            let b = teach_briefing(&rec(), true, n);
            assert!(b.contains("variable by default"), "{b}");
            for kind in ["file names", "dates", "amounts", "row positions", "search terms", "URLs"] {
                assert!(b.contains(kind), "{kind}: {b}");
            }
            assert!(b.contains("fixed only when the owner says so"), "{b}");
            assert!(b.contains("override the default"), "{b}");
            assert!(b.contains("which inputs vary and which are fixed"), "in the skill: {b}");
            assert!(b.contains("then what varies and what stays fixed"), "in the reply: {b}");
            assert!(b.contains("save_skill") && b.contains("create_workflow"), "{b}");
            assert!(b.contains("on a schedule"), "{b}");
        }
    }

    /// Nothing built from a demonstration holds a literal secret: it names
    /// a saved credential, and asks the owner to save one that is missing.
    #[test]
    fn the_briefing_never_lets_a_secret_into_what_is_built() {
        let b = teach_briefing(&rec(), true, Narration::None);
        assert!(b.contains("never write a password, token or other secret"), "{b}");
        assert!(b.contains("[typed secret]") && b.contains("[pasted text]"), "{b}");
        assert!(b.contains("metadata: secrets:"), "{b}");
        assert!(b.contains("${secret.KEY}"), "{b}");
        assert!(b.contains("Settings > Secrets"), "{b}");
        assert!(b.contains("never ask them to paste it into the chat"), "{b}");
    }

    /// A narration that could not be transcribed never stops the learning:
    /// the run goes on from the recording and says so in one line.
    #[test]
    fn a_failed_transcription_is_one_line_of_the_reply() {
        let b = teach_briefing(&rec(), true, Narration::Failed);
        assert!(b.contains("could not be transcribed"), "{b}");
        assert!(b.contains("learn from the recording alone"), "{b}");
        assert!(b.contains("one line"), "{b}");
        assert!(b.contains("save_skill"), "the run still learns: {b}");
    }

    #[test]
    fn narration_is_saved_under_a_format_the_transcriber_reads() {
        assert_eq!(narration_ext("recording-20261010-101500.m4a", "audio/mp4"), "m4a");
        assert_eq!(narration_ext("recording.webm", "audio/webm"), "webm");
        assert_eq!(narration_ext("blob", "audio/webm;codecs=opus"), "webm");
        assert_eq!(narration_ext("blob", ""), "m4a");
    }

    /// The learning run is pinned to the build lane (the deepest
    /// intelligence) for its session, and the transcript is never logged.
    #[test]
    fn the_learning_run_builds_and_never_logs_what_was_said() {
        let src = include_str!("desktop.rs");
        let learn = &src[src.find("async fn learn(").unwrap()..];
        let learn = &learn[..learn.find("\n}\n").unwrap()];
        assert!(learn.contains("build_session(&session_key, TEACH_BUILD_WINDOW)"));
        let narrate = &src[src.find("async fn narrate(").unwrap()..];
        let narrate = &narrate[..narrate.find("\n}\n").unwrap()];
        assert!(!narrate.contains("info!") && !narrate.contains("warn!") && !narrate.contains("debug!"));
        let deleted = &learn[learn.find("let _ = tokio::fs::remove_file(path)").unwrap() - 120..];
        assert!(deleted.contains("transcribed") && deleted.contains("replay.is_some()"), "the raw audio leaves once transcribed and muxed");
    }

    /// Every step is found again through its locator chain and checked
    /// against its after-state; a mismatch stops the run at that step.
    #[test]
    fn the_briefing_asks_for_locator_chains_and_after_states() {
        let b = teach_briefing(&rec(), true, Narration::None);
        assert!(b.contains("locator chain goes strongest first"), "{b}");
        assert!(b.contains("role and name first"), "{b}");
        assert!(b.contains("coordinates only as the last resort"), "{b}");
        assert!(b.contains("after-state"), "{b}");
        assert!(b.contains("Expect: browser_expect(step: \"N\""), "{b}");
        assert!(b.contains("reports the step's number and what differed"), "{b}");
        assert!(b.contains("written as the skill's inputs, never as literals"), "{b}");
    }

    /// The reply proves understanding: the steps with their pictures, what
    /// varies, corrections by talking update the skill, and the first real
    /// run is supervised before anything runs unattended.
    #[test]
    fn the_briefing_asks_for_proof_and_a_supervised_first_run() {
        let b = teach_briefing(&rec(), true, Narration::Into("timeline.md"));
        assert!(b.contains("list the steps you"), "{b}");
        assert!(b.contains("each with the keyframe"), "{b}");
        assert!(b.contains("share_file"), "{b}");
        assert!(b.contains("by talking"), "{b}");
        assert!(b.contains("update the skill with save_skill"), "{b}");
        assert!(b.contains("first real run will be") && b.contains("supervised"), "{b}");
        assert!(b.contains("wait for their go after"), "{b}");
        assert!(b.contains("Only after that run passes"), "{b}");
    }
}
