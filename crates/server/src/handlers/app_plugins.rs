//! An app runs an installed plugin directly, with no AI in the middle.
//!
//! `POST /api/v1/apps/<id>/plugins/<slug>/run` takes the command, flags and
//! time limit the employee's `plugin__<slug>` tool takes and runs that tool
//! through the registry's one door (`Registry::execute`), as the app's own
//! employee: the same installed version, sandbox and plugin permissions,
//! the employee's permissions and the owner's rules, the bot's limits, the
//! same account and the same billing. What would ask the owner is refused
//! (nothing here can wait). Nebo starts the plugin; the app never executes
//! it, and never names another employee or a longer limit than
//! [`TIMEOUT_MAX`].
//!
//! Who may call: whoever the app's own routes admit
//! (`apps::validate_app_token`): the app's sidecar with its `NEBO_APP_TOKEN`,
//! the app's page with its own pass, the owner through the hub. The call is
//! refused unless the app's manifest grants the plugin (`plugin:<slug>`, or
//! `plugin:<slug>:<command>` for one command, read as the runner reads a
//! command), the app holds that grant (`app_grants::granted`: an app built
//! here holds what it declares; one from the marketplace only what the
//! owner allowed, asked as "Media Studio wants to use Nebo Media"), the
//! plugin is installed and not turned off, and the app has fewer than
//! [`RUNS_AT_ONCE`] runs going.
//!
//! Long work: `"background": true` answers at once with a job, and
//! `GET /api/v1/apps/<id>/plugins/jobs/<job>` reads it: its progress (each
//! line the plugin writes, as it writes it) and, once done, its result. With
//! `Accept: text/event-stream` the read streams instead: a `progress` event
//! per line, then one `result` event.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// The body of a run: the `plugin__<slug>` tool's input (a command, its
/// flags, a time limit), and whether to answer at once with a job. Nothing
/// else of the tool's input is an app's to give.
#[derive(Debug, Deserialize)]
pub struct RunRequest {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: serde_json::Map<String, serde_json::Value>,
    /// Seconds; at most [`TIMEOUT_MAX`].
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    background: bool,
}

/// The longest an app's run may take, whatever it asks for: a render.
const TIMEOUT_MAX: u64 = 600;

/// What a run came to: the text the employee's tool gets back, whether the
/// plugin succeeded, and the files it reported making.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub output: String,
    pub files: Vec<String>,
}

impl From<&tools::ToolResult> for Outcome {
    fn from(r: &tools::ToolResult) -> Self {
        Self { ok: !r.is_error, output: r.content.clone(), files: r.files().map(str::to_string).collect() }
    }
}

/// What the gate reads about an app.
struct AppFacts {
    name: String,
    /// What its manifest asks for.
    declared: Vec<String>,
    /// What it holds now (`app_grants`).
    held: Vec<String>,
}

/// The app `app_id` as the gate reads it, `None` when no app has that id.
async fn app_facts(state: &AppState, app_id: &str) -> Option<AppFacts> {
    let asked = crate::app_grants::asked(state, app_id).await?;
    let held = crate::app_grants::granted_of(state, app_id, &asked);
    let name = super::apps::loaded_app(state, app_id).await.map(|a| a.agent_def.name).unwrap_or_else(|| app_id.to_string());
    Some(AppFacts { name, declared: asked.permissions, held })
}

/// Why `app` may not run `command` on the plugin `slug`, or `None` when it
/// may. The command is read the way the runner reads it
/// (`plugin_tool::command_words`). `installed`: the plugin is installed and
/// on. A refusal names the plugin by its slug, so it never tells an app what
/// the owner has installed.
fn refusal(app: &AppFacts, slug: &str, command: &str, installed: bool) -> Option<(StatusCode, String)> {
    let Some(words) = tools::plugin_tool::command_words(slug, command) else {
        return Some((StatusCode::BAD_REQUEST, format!("Could not read the command `{command}`: a quote is not closed.")));
    };
    let first = words.first().map(String::as_str);
    if !napp::manifest::grants_plugin_command(&app.declared, slug, first) {
        return Some((
            StatusCode::FORBIDDEN,
            format!("{} may not use {slug} this way: its manifest grants no plugin:{slug} permission that covers this command.", app.name),
        ));
    }
    if !napp::manifest::grants_plugin_command(&app.held, slug, first) {
        return Some((StatusCode::FORBIDDEN, format!("{} wants to use {slug}, and the owner has not allowed it.", app.name)));
    }
    if !installed {
        return Some((StatusCode::NOT_FOUND, format!("{slug} is not installed on this bot, or it is turned off.")));
    }
    None
}

/// The runs one app may have going at once, waiting and in the background
/// alike. One more is refused (429) until one ends.
const RUNS_AT_ONCE: usize = 4;

static RUNNING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> = LazyLock::new(Default::default);

/// A place for one more of `app`'s runs, `None` when it has [`RUNS_AT_ONCE`]
/// going. The run holds it until it ends.
fn run_slot(app: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
    let gate = RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(app.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(RUNS_AT_ONCE)))
        .clone();
    gate.try_acquire_owned().ok()
}

/// The run's context: the app's employee running unattended through the
/// local API, its result read by a program. The registry's one door
/// decides the call as for any of the employee's calls (its permissions,
/// the owner's rules, the bot's limits); what would ask the owner is
/// refused, since nothing here can wait, and an expired sign-in is
/// reported, never put in front of him as a card.
fn app_context(app_id: &str, output_lines: Option<tokio::sync::mpsc::UnboundedSender<String>>) -> tools::ToolContext {
    tools::ToolContext {
        origin: tools::Origin::App,
        door: types::permissions::Door::LocalApi,
        session_key: types::keyparser::build_agent_session_key(app_id, "app"),
        whole_result: true,
        cannot_wait: true,
        output_lines,
        ..Default::default()
    }
}

/// The `plugin__<slug>` tool's input for a run.
fn tool_input(body: &RunRequest) -> serde_json::Value {
    let mut input = serde_json::json!({ "command": body.command, "args": body.args });
    // None: the tool's own default, under the most an app may take.
    if let Some(t) = body.timeout {
        input["timeout"] = t.min(TIMEOUT_MAX).into();
    }
    input
}

/// POST /apps/{agent_id}/plugins/{slug}/run
pub async fn run(
    State(state): State<AppState>,
    Path((app_id, slug)): Path<(String, String)>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<RunRequest>,
) -> Response {
    if let Err(r) = super::apps::validate_app_token(&state, &app_id, &headers).await {
        return r;
    }
    let Some(app) = app_facts(&state, &app_id).await else {
        return (StatusCode::NOT_FOUND, "no such app").into_response();
    };
    let installed = tools::plugin_tool::installed_plugin_slugs(&state.plugin_store, &state.store).contains(&slug);
    if let Some(refused) = refusal(&app, &slug, &body.command, installed) {
        return refused.into_response();
    }
    let Some(slot) = run_slot(&app_id) else {
        return (StatusCode::TOO_MANY_REQUESTS, format!("{} already has {RUNS_AT_ONCE} plugin runs going. Try again when one ends.", app.name))
            .into_response();
    };
    let tool = format!("plugin__{slug}");
    let input = tool_input(&body);
    if !body.background {
        let result = state.tools.execute(&app_context(&app_id, None), &tool, input).await;
        drop(slot);
        return axum::Json(Outcome::from(&result)).into_response();
    }
    let job = start_job(&app_id, &slug, &body.command);
    let (lines, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ending = Ending(job.clone());
    let tools = state.tools.clone();
    tokio::spawn(async move {
        let progress = {
            let job = ending.0.clone();
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    job.progress(line);
                }
            })
        };
        let ctx = app_context(&app_id, Some(lines));
        let result = tools.execute(&ctx, &tool, input).await;
        drop(slot);
        // The context holds the last sender: the progress ends with it.
        drop(ctx);
        let _ = progress.await;
        ending.0.finish(Outcome::from(&result));
    });
    (StatusCode::ACCEPTED, axum::Json(serde_json::json!({ "job": job.id }))).into_response()
}

/// Ends a job whose run stopped without a result (its task panicked or was
/// dropped), so it is let go in time and every stream reading it ends.
struct Ending(Arc<Job>);

impl Drop for Ending {
    fn drop(&mut self) {
        if self.0.result().is_none() {
            self.0.finish(Outcome { ok: false, output: "The plugin run stopped before it finished.".into(), files: Vec::new() });
        }
    }
}

/// GET /apps/{agent_id}/plugins/jobs/{job}
pub async fn job(
    State(state): State<AppState>,
    Path((app_id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = super::apps::validate_app_token(&state, &app_id, &headers).await {
        return r;
    }
    // Another app's job is no job of this one's.
    let Some(job) = find_job(&job_id).filter(|j| j.app == app_id) else {
        return (StatusCode::NOT_FOUND, "no such job").into_response();
    };
    let streams = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/event-stream"));
    if !streams {
        return axum::Json(job.snapshot()).into_response();
    }
    let stream = futures::stream::unfold((job, 0usize, false), |(job, next, ended)| async move {
        if ended {
            return None;
        }
        loop {
            let mut changed = job.tick.subscribe();
            if let Some((line, at)) = job.line_from(next) {
                return Some((Ok::<_, std::convert::Infallible>(Event::default().event("progress").data(line)), (job, at + 1, false)));
            }
            if let Some(result) = job.result() {
                let data = serde_json::to_string(&result).unwrap_or_default();
                return Some((Ok(Event::default().event("result").data(data)), (job, next, true)));
            }
            let _ = changed.changed().await;
        }
    });
    Sse::new(stream).into_response()
}

// ── jobs ────────────────────────────────────────────────────────────────

/// The lines of progress a job keeps: the newest, enough to show where it is.
const PROGRESS_KEPT: usize = 200;

/// How long a finished job can still be read.
const FINISHED_KEPT_FOR: Duration = Duration::from_secs(10 * 60);

/// The most finished jobs of one app kept at once, the newest.
const FINISHED_KEPT_PER_APP: usize = 4;

static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

/// One background run of an app's plugin call.
struct Job {
    id: String,
    app: String,
    plugin: String,
    command: String,
    log: Mutex<Log>,
    /// Bumped on every new line and on the result, for the streams reading it.
    tick: tokio::sync::watch::Sender<()>,
}

#[derive(Default)]
struct Log {
    /// The index of `lines[0]` among every line the job wrote.
    first: usize,
    lines: VecDeque<String>,
    result: Option<Outcome>,
    ended: Option<Instant>,
}

/// A new job for `app`, kept where [`find_job`] reads it. Jobs that ended
/// long ago are let go here.
fn start_job(app: &str, plugin: &str, command: &str) -> Arc<Job> {
    let job = Arc::new(Job {
        id: uuid::Uuid::new_v4().to_string(),
        app: app.to_string(),
        plugin: plugin.to_string(),
        command: command.to_string(),
        log: Mutex::new(Log::default()),
        tick: tokio::sync::watch::channel(()).0,
    });
    let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    jobs.retain(|_, j| j.log().ended.is_none_or(|at| at.elapsed() < FINISHED_KEPT_FOR));
    // The app's finished jobs beyond the newest few are let go too.
    let mut finished: Vec<(Instant, String)> =
        jobs.values().filter(|j| j.app == app).filter_map(|j| j.log().ended.map(|at| (at, j.id.clone()))).collect();
    finished.sort();
    let over = finished.len().saturating_sub(FINISHED_KEPT_PER_APP);
    for (_, id) in &finished[..over] {
        jobs.remove(id);
    }
    jobs.insert(job.id.clone(), job.clone());
    job
}

fn find_job(id: &str) -> Option<Arc<Job>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
}

impl Job {
    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn progress(&self, line: String) {
        {
            let mut log = self.log();
            log.lines.push_back(line);
            if log.lines.len() > PROGRESS_KEPT {
                log.lines.pop_front();
                log.first += 1;
            }
        }
        self.tick.send_replace(());
    }

    fn finish(&self, result: Outcome) {
        {
            let mut log = self.log();
            log.result = Some(result);
            log.ended = Some(Instant::now());
        }
        self.tick.send_replace(());
    }

    /// The first line kept at or after index `from`, with its index.
    fn line_from(&self, from: usize) -> Option<(String, usize)> {
        let log = self.log();
        let at = from.max(log.first);
        log.lines.get(at - log.first).map(|l| (l.clone(), at))
    }

    fn result(&self) -> Option<Outcome> {
        self.log().result.clone()
    }

    fn snapshot(&self) -> serde_json::Value {
        let log = self.log();
        serde_json::json!({
            "job": self.id,
            "plugin": self.plugin,
            "command": self.command,
            "done": log.result.is_some(),
            "progress": log.lines,
            "result": log.result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(permissions: &[&str], allowed: bool) -> AppFacts {
        let declared: Vec<String> = permissions.iter().map(|p| p.to_string()).collect();
        let held = if allowed { declared.clone() } else { Vec::new() };
        AppFacts { name: "Media Studio".into(), declared, held }
    }

    /// The gate, in its order: the manifest's grant, then the owner's yes,
    /// then the plugin being there. A refusal names the plugin by its slug.
    #[test]
    fn an_app_runs_a_plugin_only_when_granted_allowed_and_installed() {
        assert_eq!(refusal(&app(&["plugin:nebo-media"], true), "nebo-media", "strip --in a.mp4", true), None);
        assert_eq!(refusal(&app(&["plugin:nebo-media:strip"], true), "nebo-media", "strip --in a.mp4", true), None);

        let (status, why) = refusal(&app(&["plugin:nebo-media:strip"], true), "nebo-media", "render", true).unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(why.contains("plugin:nebo-media"), "{why}");
        let (status, _) = refusal(&app(&["storage:readwrite"], true), "nebo-media", "strip", true).unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "an ungranted plugin");
        let (status, _) = refusal(&app(&["plugin:gmail"], true), "nebo-media", "strip", true).unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "a grant for another plugin");

        let (status, why) = refusal(&app(&["plugin:nebo-media"], false), "nebo-media", "strip", true).unwrap();
        assert_eq!((status, why.as_str()), (StatusCode::FORBIDDEN, "Media Studio wants to use nebo-media, and the owner has not allowed it."));

        let (status, why) = refusal(&app(&["plugin:nebo-media"], true), "nebo-media", "strip", false).unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "an uninstalled plugin: {why}");
        let (status, _) = refusal(&app(&[], true), "nebo-media", "strip", false).unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "an ungranted plugin is refused as ungranted, installed or not");
    }

    /// A command-scoped grant reads the command the way the runner does:
    /// a leading plugin name, quotes and extra spaces neither hide nor
    /// widen the command.
    #[test]
    fn a_scoped_grant_reads_the_command_as_the_runner_does() {
        let strip = app(&["plugin:nebo-media:strip"], true);
        for allowed in ["strip", "nebo-media strip --every 2", "\"strip\" --in a.mp4", "  strip   --in a.mp4", "NEBO-MEDIA strip"] {
            assert_eq!(refusal(&strip, "nebo-media", allowed, true), None, "{allowed}");
        }
        for refused in ["render", "nebo-media render", "\"render\" strip", "nebo-media", ""] {
            assert_eq!(refusal(&strip, "nebo-media", refused, true).map(|r| r.0), Some(StatusCode::FORBIDDEN), "{refused}");
        }
        assert_eq!(refusal(&strip, "nebo-media", "strip \"open", true).map(|r| r.0), Some(StatusCode::BAD_REQUEST));
    }

    /// An app gives the command, its flags and a time limit, and nothing
    /// else of the tool's input (never another employee); the limit is at
    /// most a render's.
    #[test]
    fn an_apps_input_is_its_command_flags_and_a_bounded_limit() {
        let body: RunRequest = serde_json::from_value(serde_json::json!({
            "command": "strip", "args": {"in": "a.mp4", "every": 2}, "timeout": 1_000_000_000u64, "employee": "Bookkeeper"
        }))
        .unwrap();
        let input = tool_input(&body);
        assert_eq!(input, serde_json::json!({"command": "strip", "args": {"in": "a.mp4", "every": 2}, "timeout": TIMEOUT_MAX}));
        let body: RunRequest = serde_json::from_value(serde_json::json!({"command": "strip"})).unwrap();
        assert!(tool_input(&body).get("timeout").is_none(), "the tool's own default");
    }

    /// One app has a few runs going at once; one more waits for one to end,
    /// and another app is not held up by it.
    #[test]
    fn an_app_has_a_few_runs_going_at_once() {
        let held: Vec<_> = (0..RUNS_AT_ONCE).map(|_| run_slot("busy-app").expect("a slot")).collect();
        assert!(run_slot("busy-app").is_none(), "one more is refused");
        assert!(run_slot("other-app").is_some(), "another app has its own");
        drop(held);
        assert!(run_slot("busy-app").is_some(), "free again once one ends");
    }

    /// An app's finished jobs beyond the newest few are let go; a job whose
    /// run stopped without a result ends anyway.
    #[test]
    fn finished_jobs_are_few_and_every_job_ends() {
        let app = format!("keep-{}", uuid::Uuid::new_v4().simple());
        let jobs: Vec<_> = (0..FINISHED_KEPT_PER_APP + 2)
            .map(|_| {
                let j = start_job(&app, "p", "c");
                std::thread::sleep(Duration::from_millis(2));
                j.finish(Outcome { ok: true, output: String::new(), files: vec![] });
                j
            })
            .collect();
        start_job(&app, "p", "c");
        let kept = jobs.iter().filter(|j| find_job(&j.id).is_some()).count();
        assert_eq!(kept, FINISHED_KEPT_PER_APP);
        assert!(find_job(&jobs[0].id).is_none() && find_job(&jobs.last().unwrap().id).is_some(), "the oldest go first");

        let stopped = start_job(&app, "p", "c");
        drop(Ending(stopped.clone()));
        assert_eq!(stopped.result().map(|r| r.ok), Some(false));
    }

    /// A job's stream reads every line from where it is, skips what was let
    /// go, and ends with the result.
    #[tokio::test]
    async fn a_job_keeps_its_newest_lines_and_its_result() {
        let job = start_job("a1", "nebo-media", "render");
        for i in 0..PROGRESS_KEPT + 5 {
            job.progress(format!("line {i}"));
        }
        assert_eq!(job.line_from(0), Some(("line 5".to_string(), 5)), "the oldest five were let go");
        assert_eq!(job.line_from(PROGRESS_KEPT + 4).map(|(l, _)| l).as_deref(), Some(&*format!("line {}", PROGRESS_KEPT + 4)));
        assert_eq!(job.line_from(PROGRESS_KEPT + 5), None);
        assert_eq!(job.snapshot()["done"], false);

        job.finish(Outcome { ok: true, output: "Result: /tmp/a.mp4".into(), files: vec!["/tmp/a.mp4".into()] });
        let snap = job.snapshot();
        assert_eq!(snap["done"], true);
        assert_eq!(snap["result"]["files"][0], "/tmp/a.mp4");
        assert_eq!(snap["progress"].as_array().unwrap().len(), PROGRESS_KEPT);
        assert!(find_job(&job.id).is_some());
    }
}
