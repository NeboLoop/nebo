//! The guided first-run chat: the owner's first conversation with their
//! assistant, once sign-up and connecting are done.
//!
//! The apps show three intro screens, then open the assistant's chat. The
//! conversation itself is the assistant speaking in its own chat, in the
//! owner's language, with the one-tap cards every chat already has
//! (`ask_owner` choices, the plugin connect cards, the hire card). What
//! comes next, and what each answer changes, is decided HERE: the step lives
//! in `user_profiles.onboarding_step`, the assistant reads it and reports
//! each answer through the `onboarding` tool, and the apps read it through
//! `GET /onboarding`. A conversation cut short (an app closed, a restart)
//! picks up at the same step: `POST /onboarding/start` starts the chat or
//! resumes it.
//!
//! Steps: intro → name → focus → connect → (day | plate) → daily → hire →
//! set → done. Every answer the owner gives is one tap, and typing always
//! works (an `ask_owner` card takes his own words).

use axum::extract::{Path, State};
use axum::response::Json;
use serde_json::{Value, json};
use tracing::warn;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

const PRIMARY: &str = tools::team_tool::PRIMARY_AGENT_ID;

/// The Google plugins "Connect Google" signs the assistant in to: email and
/// calendar, the successors of the retired `gws` bundle.
pub(crate) const GOOGLE: [&str; 2] = ["gmail", "google-calendar"];

/// The names offered for the assistant, the default first.
pub(crate) const NAMES: [&str; 3] = ["Nebo", "Aspen", "Logan"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Intro,
    Name,
    Focus,
    Connect,
    Plate,
    Day,
    Daily,
    Hire,
    Set,
    Done,
}

impl Step {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Step::Intro => "intro",
            Step::Name => "name",
            Step::Focus => "focus",
            Step::Connect => "connect",
            Step::Plate => "plate",
            Step::Day => "day",
            Step::Daily => "daily",
            Step::Hire => "hire",
            Step::Set => "set",
            Step::Done => "done",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Step> {
        [
            Step::Intro,
            Step::Name,
            Step::Focus,
            Step::Connect,
            Step::Plate,
            Step::Day,
            Step::Daily,
            Step::Hire,
            Step::Set,
            Step::Done,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
    }
}

/// What the owner wants help with most. It picks the first hire and the
/// account type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Store,
    Content,
    Clients,
    Team,
    Home,
}

impl Focus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Focus::Store => "store",
            Focus::Content => "content",
            Focus::Clients => "clients",
            Focus::Team => "team",
            Focus::Home => "home",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Focus> {
        [Focus::Store, Focus::Content, Focus::Clients, Focus::Team, Focus::Home]
            .into_iter()
            .find(|f| f.as_str() == s.trim())
    }

    /// The marketplace employee suggested as the first hire.
    pub(crate) fn hire_listing(self) -> &'static str {
        match self {
            Focus::Store => "store-manager",
            Focus::Content => "content-strategist",
            Focus::Clients => "client-account-manager",
            Focus::Team => "chief-of-staff",
            Focus::Home => "personal-admin",
        }
    }

    /// How the assistant says what the owner does.
    fn phrase(self) -> &'static str {
        match self {
            Focus::Store => "run a store",
            Focus::Content => "create content",
            Focus::Clients => "work with clients",
            Focus::Team => "lead a team",
            Focus::Home => "run a busy home",
        }
    }

    /// The account type the profile has always kept ("personal" or "business").
    pub(crate) fn account_type(self) -> &'static str {
        if self == Focus::Home { "personal" } else { "business" }
    }
}

/// The morning brief's time, from the answer to "Make it daily".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Daily {
    /// A cron expression (seconds first, as every schedule here).
    At(String),
    /// Not now: the schedule is turned off; "what's my day?" works any time.
    Off,
}

/// What an answer changes, besides the step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    None,
    Name(String),
    Focus(Focus),
    Connected,
    Daily(Daily),
    Hired,
}

/// The step after `step` for `value`, and what the answer changes. Pure:
/// the state machine, with nothing read or written. `skip` at any chat step
/// moves on without a change; `finish` ends the guided chat from anywhere.
pub(crate) fn transition(step: Step, value: &str) -> Result<(Step, Effect), String> {
    let value = value.trim();
    if value == "finish" && step != Step::Done {
        return Ok((Step::Done, Effect::None));
    }
    let skip = value == "skip" || value == tools::origin::SKIP_SENTINEL;
    match step {
        Step::Intro => Ok((Step::Name, Effect::None)),
        Step::Name if skip => Ok((Step::Focus, Effect::None)),
        Step::Name => {
            if value.is_empty() || value.chars().count() > 40 {
                return Err("A name is 1 to 40 characters.".into());
            }
            Ok((Step::Focus, Effect::Name(value.to_string())))
        }
        Step::Focus if skip => Ok((Step::Connect, Effect::None)),
        Step::Focus => Focus::parse(value)
            .map(|f| (Step::Connect, Effect::Focus(f)))
            .ok_or_else(|| "focus is one of store, content, clients, team, home (or skip).".into()),
        Step::Connect => match value {
            "connected" => Ok((Step::Day, Effect::Connected)),
            "not_now" => Ok((Step::Plate, Effect::None)),
            _ if skip => Ok((Step::Plate, Effect::None)),
            _ => Err("connect is connected or not_now.".into()),
        },
        Step::Plate | Step::Day => Ok((Step::Daily, Effect::None)),
        Step::Daily => daily(value).map(|d| (Step::Hire, Effect::Daily(d))),
        Step::Hire => match value {
            "hired" => Ok((Step::Set, Effect::Hired)),
            "later" => Ok((Step::Set, Effect::None)),
            _ if skip => Ok((Step::Set, Effect::None)),
            _ => Err("hire is hired or later.".into()),
        },
        Step::Set => Ok((Step::Done, Effect::None)),
        Step::Done => Err("The first-run chat is finished.".into()),
    }
}

/// "weekdays" (7:30 on weekdays), "daily" (7:00 every day), "HH:MM" (every
/// day then), or "not_now" / skip.
fn daily(value: &str) -> Result<Daily, String> {
    match value {
        "weekdays" => return Ok(Daily::At("0 30 7 * * 1-5".into())),
        "daily" => return Ok(Daily::At("0 0 7 * * *".into())),
        "not_now" | "skip" => return Ok(Daily::Off),
        v if v == tools::origin::SKIP_SENTINEL => return Ok(Daily::Off),
        _ => {}
    }
    let (h, m) = value
        .split_once(':')
        .and_then(|(h, m)| Some((h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)))
        .filter(|(h, m)| *h < 24 && *m < 60)
        .ok_or_else(|| "daily is weekdays, daily, a time as HH:MM (24-hour), or not_now.".to_string())?;
    Ok(Daily::At(format!("0 {m} {h} * * *")))
}

/// The edit "Make it daily" makes to the morning brief's ONE schedule (the
/// Inbox's `morning-brief`, through `PUT /tasks/{name}`): its time, or off.
fn daily_edit(d: &Daily) -> Value {
    match d {
        Daily::At(cron) => json!({ "schedule": cron, "enabled": true, "reason": "the owner chose it in the first-run chat" }),
        Daily::Off => json!({ "enabled": false, "reason": "the owner said not now in the first-run chat" }),
    }
}

/// Where the guided chat is. A profile that never had a step is an install
/// from before it: one whose first employee was already named (the old
/// christening, or a name other than the default) is done; a fresh one
/// starts at the intro.
pub(crate) fn current(store: &db::Store) -> Step {
    let profile = store.get_user_profile().ok().flatten();
    if let Some(step) = profile.as_ref().and_then(|p| p.onboarding_step.as_deref()).and_then(Step::parse) {
        return step;
    }
    let christened = store.get_plugin_setting("core", "primary_christened").ok().flatten().is_some()
        || store.get_agent(PRIMARY).ok().flatten().is_some_and(|a| a.name != NAMES[0]);
    if christened {
        if let Err(e) = store.set_onboarding(Step::Done.as_str(), None, None) {
            warn!(error = %e, "onboarding: could not record an earlier install as done");
        }
        Step::Done
    } else {
        Step::Intro
    }
}

/// What the assistant is told to do at `step`: the words to say (in the
/// owner's language) and the card to show. Pure.
pub(crate) fn guide(step: Step, f: &Facts) -> String {
    let owner = f.owner.as_deref().unwrap_or("there");
    let me = &f.assistant;
    let names = NAMES.join(", ");
    match step {
        Step::Intro => "The owner is on the intro screens; nothing to say yet.".into(),
        Step::Name => format!(
            "Greet {owner} by first name, warm and short, for example: \"Hi {owner}! 👋 I'm your new assistant. \
             I'll take things off your plate, at work and at home.\" Then ask what they'd like to call you, \
             saying they can change it any time, with ask_owner and options exactly: {names}, Something else… \
             ({first} first; it is the default). If they pick Something else…, ask for the name with ask_owner \
             and no options. Then call onboarding(action: \"answer\", step: \"name\", value: <the name>).",
            first = NAMES[0]
        ),
        Step::Focus => format!(
            "Say \"{me} it is. Nice to meet you, {owner}.\" Then \"A little about me:\" and exactly three short \
             bullets: I work on my own computer, so I keep going while you're away. / I use your apps, like \
             email and calendar, the way you would. / Nothing goes out without your OK. Then ask \"What should I \
             help with most?\" with ask_owner and options: 🛍 Run a store, 🎬 Create content, 🤝 Work with \
             clients, 👥 Lead a team, 🏡 Home and family. Call onboarding(action: \"answer\", step: \"focus\", \
             value: store | content | clients | team | home) for the pick (for words of their own, the closest)."
        ),
        Step::Connect => format!(
            "Say \"Got it. Let's start with something that pays off today.\" Then: \"If I can see your email and \
             calendar, I'll find what needs you today and draft the replies. It takes about 30 seconds.\" Then, \
             BEFORE any connect card, the trust line: \"I read so I can help. I never send, delete or accept \
             anything without your OK.\" Then connect Google for yourself: the {gmail} and {cal} plugins \
             (install them if they aren't, then show their connect cards; the cards are how the owner says \
             yes). When both are connected, call onboarding(action: \"answer\", step: \"connect\", value: \
             \"connected\"). If the owner skips a card or says not now, or a connection isn't ready, call it \
             with value \"not_now\" — say nothing about errors and never apologize; the next step is the \
             next-best thing.",
            gmail = GOOGLE[0],
            cal = GOOGLE[1]
        ),
        Step::Plate => format!(
            "Say \"No problem. Tell me three things on your plate this week and I'll turn them into a plan.\" \
             Ask with ask_owner and no options. Turn what they say into a short plan, start the first one now \
             if you can, and offer to schedule the rest. Then call onboarding(action: \"answer\", step: \
             \"plate\", value: \"done\"). {owner} never sees an error from you here."
        ),
        Step::Day => format!(
            "This is the first real value. Work on {owner}'s day now, showing progress as you go: read the \
             last 2 days of email, check today's calendar, find bills and deadlines. Then say \"Here's your \
             day, {owner}. <N> things need you; the rest can wait.\" and show: today's meetings with times; \
             the (at most 3) emails that need them, each with a reply already drafted (save drafts, never \
             send); and what's coming up (bills, renewals, deadlines). This is also today's morning brief: write it \
             into the Inbox exactly as the daily brief does (its instructions follow; in this chat you show \
             it as well): {brief} Then offer to send one drafted reply with one tap \
             (ask_owner with options like \"Send the reply to <who>\" and \"Not now\"); sending goes through \
             the usual approval. Then call onboarding(action: \"answer\", step: \"day\", value: \"done\").",
            brief = super::notification::MORNING_BRIEF_ASK
        ),
        Step::Daily => format!(
            "Ask \"Want this waiting for you every morning?\" with ask_owner and options: Every weekday at \
             7:30 AM, Every day at 7:00 AM, Pick a time…, Not now (the first is recommended). For Pick a \
             time…, ask the time. Call onboarding(action: \"answer\", step: \"daily\", value: weekdays | daily \
             | HH:MM (24-hour) | not_now). Then confirm in one line: \"Done. You'll find it in your Inbox each \
             morning\" with the time{zone}; or, for not now, \"Sure. Just ask 'what's my day?' any time.\"",
            zone = f.zone.as_deref().map(|z| format!(" and their time zone ({z})")).unwrap_or_default()
        ),
        Step::Hire => match f.focus {
            Some(focus) => format!(
                "Say \"One more thing. I can bring in help.\" Since they {phrase}, suggest one first hire: the \
                 marketplace employee '{listing}'. Find it with find_employees(query: \"{listing}\"), say in a \
                 line what it does for them, and offer it with hire_employee (its card is the Hire button); \
                 \"Maybe later\" is fine. Then call onboarding(action: \"answer\", step: \"hire\", value: hired | \
                 later). After a hire: \"Welcome aboard. I'll show them how you like things done.\"",
                phrase = focus.phrase(),
                listing = focus.hire_listing()
            ),
            None => "Say \"One more thing. I can bring in help.\" Ask what they'd hand off first, suggest one \
                     marketplace employee for it with find_employees, and offer it with hire_employee. Then call \
                     onboarding(action: \"answer\", step: \"hire\", value: hired | later)."
                .into(),
        },
        Step::Set => format!(
            "Celebrate: \"You're all set, {owner} 🎉\" and a short checklist of what's done: {done}. Then \
             \"Anything else on your plate? Just tell me.\" Call onboarding(action: \"answer\", step: \"set\", \
             value: \"finish\").",
            done = f.checklist().join("; ")
        ),
        Step::Done => "The first-run chat is finished: carry on as usual.".into(),
    }
}

/// What the guide and the apps read: who the owner is and what is done.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub owner: Option<String>,
    pub assistant: String,
    pub focus: Option<Focus>,
    pub google_connected: bool,
    pub brief_daily: bool,
    pub hired: Option<String>,
    pub zone: Option<String>,
}

impl Facts {
    fn read(state: &AppState) -> Facts {
        let store = &state.store;
        let profile = store.get_user_profile().ok().flatten();
        let first_name = |n: &str| n.split_whitespace().next().map(str::to_string);
        Facts {
            owner: profile.as_ref().and_then(|p| p.display_name.as_deref()).and_then(first_name),
            assistant: store.get_agent(PRIMARY).ok().flatten().map(|a| a.name).unwrap_or_else(|| NAMES[0].to_string()),
            focus: profile.as_ref().and_then(|p| p.onboarding_focus.as_deref()).and_then(Focus::parse),
            google_connected: GOOGLE
                .iter()
                .all(|slug| tools::plugin_tool::plugin_connected(&state.plugin_store, store, slug)),
            brief_daily: store
                .get_cron_job_by_name(super::notification::MORNING_BRIEF)
                .ok()
                .flatten()
                .is_some_and(|j| j.enabled.unwrap_or(1) != 0),
            hired: profile
                .as_ref()
                .and_then(|p| p.onboarding_hire.as_deref())
                .and_then(|id| store.get_agent(id).ok().flatten())
                .map(|a| a.name),
            zone: tools::owner_clock::OwnerZone::of(store).name().map(str::to_string),
        }
    }

    /// The "You're set" checklist: only what is true.
    fn checklist(&self) -> Vec<String> {
        let mut done = vec![format!("named your assistant {}", self.assistant)];
        if self.google_connected {
            done.push("connected email and calendar".into());
        }
        if self.brief_daily {
            done.push("your morning brief, every morning".into());
        }
        if let Some(name) = &self.hired {
            done.push(format!("{name} joined your team"));
        }
        done
    }
}

/// The state the apps and the assistant read.
fn status(state: &AppState, step: Step) -> types::api::OnboardingStatus {
    let f = Facts::read(state);
    types::api::OnboardingStatus {
        step: step.as_str().to_string(),
        owner_name: f.owner.clone(),
        assistant_name: f.assistant.clone(),
        focus: f.focus.map(|x| x.as_str().to_string()),
        google_connected: f.google_connected,
        brief_daily: f.brief_daily,
        hired: f.hired.clone(),
        zone: f.zone.clone(),
        checklist: if step == Step::Done || step == Step::Set { f.checklist() } else { Vec::new() },
    }
}

/// Apply the owner's answer at `step`: the change it makes, then the next
/// step. An answer for a step that is no longer current (a second device, a
/// repeated call) changes nothing and reads back where things are.
pub(crate) async fn answer(state: &AppState, step: Step, value: &str) -> Result<Step, String> {
    let now = current(&state.store);
    if now != step {
        return Ok(now);
    }
    let (next, effect) = transition(step, value)?;
    let mut focus = None;
    let mut hire = None;
    match effect {
        Effect::None => {}
        Effect::Name(name) => {
            // The one rename pathway (registry sync and roster broadcast ride
            // it); its body echoes the row back, which nothing here needs.
            let _renamed = super::agents::update_agent(State(state.clone()), Path(PRIMARY.to_string()), Json(json!({ "name": name })))
                .await
                .map_err(|(_, e)| e.0.error.clone())?;
        }
        Effect::Focus(f) => {
            state
                .store
                .update_user_profile(None, None, None, None, None, None, None, None, None, Some(f.account_type()))
                .map_err(|e| e.to_string())?;
            focus = Some(f.as_str());
        }
        Effect::Connected => {
            let missing: Vec<&str> = GOOGLE
                .iter()
                .copied()
                .filter(|slug| !tools::plugin_tool::plugin_connected(&state.plugin_store, &state.store, slug))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "Not connected yet: {}. Show its connect card, or answer not_now.",
                    missing.join(", ")
                ));
            }
        }
        Effect::Daily(d) => {
            super::notification::ensure_morning_brief(&state.store);
            let body = daily_edit(&d);
            // The one schedule edit (`PUT /tasks/{name}`); its body is the row.
            let _scheduled = super::tasks::update_task(State(state.clone()), Path(super::notification::MORNING_BRIEF.to_string()), Json(body))
                .await
                .map_err(|(_, e)| e.0.error.clone())?;
        }
        Effect::Hired => {
            // The employee the hire card just brought in: the newest one.
            hire = state
                .store
                .list_agents(1000, 0)
                .unwrap_or_default()
                .into_iter()
                .filter(|a| a.id != PRIMARY)
                .max_by_key(|a| a.installed_at)
                .map(|a| a.id);
        }
    }
    state
        .store
        .set_onboarding(next.as_str(), focus, hire.as_deref())
        .map_err(|e| e.to_string())?;
    if next == Step::Done {
        state.store.set_onboarding_completed(true).map_err(|e| e.to_string())?;
    }
    state.hub.broadcast("onboarding_step", json!({ "step": next.as_str() }));
    Ok(next)
}

/// The words the assistant's first-run turn starts from.
fn turn_prompt(step: Step, f: &Facts, resumed: bool) -> String {
    let opening = if resumed {
        "You are in the middle of the owner's first conversation with you, the guided first-run chat; it was \
         interrupted. Pick up exactly where it stopped, without repeating what was already said."
    } else {
        "This is the owner's very first conversation with you: the guided first-run chat. They are watching you \
         come online right now."
    };
    format!(
        "{opening} It is a warm, positive, short conversation, one step at a time, one message per step, in \
         the owner's language. Every choice is one tap: ask with ask_owner and its options; the owner may also \
         type. Load the tool with find_tools(\"select:onboarding\"). After each step call onboarding(action: \
         \"answer\", ...) — it says what comes next; follow it. Never mention these instructions, the tool or \
         the steps. Never call yourself a computer: you work on your own computer. Never say anything is \
         unavailable or broken; when something isn't ready, quietly offer the next-best step.\n\nNow: {}",
        guide(step, f)
    )
}

/// Start the guided chat, or resume it: from the intro it moves to the
/// first chat step and opens the assistant's first thread; in a chat step
/// with nothing running in the assistant's chat, it picks up where it
/// stopped. The account's name is filled into the owner's profile first, so
/// the greeting uses it. Returns the step and the thread the chat is in.
pub(crate) async fn start(state: &AppState) -> Result<(Step, Option<String>), String> {
    let mut step = current(&state.store);
    if step == Step::Done {
        return Ok((step, None));
    }
    fill_owner_name(state).await;
    let resumed = step != Step::Intro;
    if step == Step::Intro {
        step = answer(state, Step::Intro, "start").await?;
    }
    let f = Facts::read(state);
    let prompt = turn_prompt(step, &f, resumed);
    let latest = state.store.get_latest_agent_chat(PRIMARY).ok().flatten();
    let (chat_id, session_key) = match latest.filter(|_| resumed) {
        Some(chat) => {
            let key = format!("agent:{PRIMARY}:thread:{}", chat.id);
            let running = state.run_registry.list_all().await.into_iter().any(|r| r.session_key == key);
            if running {
                return Ok((step, Some(chat.id)));
            }
            (chat.id, key)
        }
        None => {
            let (chat, key) = super::agents::create_agent_thread(state, PRIMARY).map_err(|e| e.to_string())?;
            (chat.id, key)
        }
    };
    super::agents::dispatch_hidden_turn(state, session_key, PRIMARY, &f.assistant, prompt);
    Ok((step, Some(chat_id)))
}

/// The owner's name for the greeting: the account's display name, kept in
/// the profile the assistant's prompt reads (`# User Information`). A name
/// the owner set himself stays.
async fn fill_owner_name(state: &AppState) {
    let has_name = state
        .store
        .get_user_profile()
        .ok()
        .flatten()
        .and_then(|p| p.display_name)
        .is_some_and(|n| !n.trim().is_empty());
    if has_name {
        return;
    }
    let Some(name) = super::neboai::account_identity(state).await.and_then(|a| a.display_name).filter(|n| !n.trim().is_empty()) else {
        return;
    };
    if let Err(e) = state.store.update_user_profile(Some(&name), None, None, None, None, None, None, None, None, None) {
        warn!(error = %e, "onboarding: could not keep the owner's name");
    }
}

/// GET /api/v1/onboarding
pub async fn get_onboarding(State(state): State<AppState>) -> HandlerResult<types::api::OnboardingStatus> {
    let step = current(&state.store);
    Ok(Json(status(&state, step)))
}

/// POST /api/v1/onboarding/start — start the guided chat from the intro
/// screens (Get started or Skip), or resume it when it was cut short.
pub async fn start_onboarding(State(state): State<AppState>) -> HandlerResult<types::api::OnboardingStarted> {
    let (step, thread_id) = start(&state)
        .await
        .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;
    Ok(Json(types::api::OnboardingStarted { status: status(&state, step), thread_id }))
}

/// The guided chat as the assistant's `onboarding` tool reads and answers
/// it (`tools::onboarding_tool`): the step, what to do now, each answer.
pub struct ServerFirstRun {
    state: AppState,
}

impl ServerFirstRun {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn reply(&self, step: Step) -> String {
        let f = Facts::read(&self.state);
        json!({ "step": step.as_str(), "next": guide(step, &f) }).to_string()
    }
}

impl tools::onboarding_tool::FirstRun for ServerFirstRun {
    fn status<'a>(&'a self) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move { self.reply(current(&self.state.store)) })
    }

    fn answer<'a>(
        &'a self,
        step: &'a str,
        value: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let step = Step::parse(step)
                .ok_or_else(|| "step is one of name, focus, connect, plate, day, daily, hire, set.".to_string())?;
            let next = answer(&self.state, step, value).await?;
            Ok(self.reply(next))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(answers: &[(Step, &str)]) -> Step {
        let mut step = Step::Intro;
        for (at, value) in answers {
            assert_eq!(step, *at, "the steps go in order");
            step = transition(step, value).unwrap().0;
        }
        step
    }

    #[test]
    fn the_happy_path_goes_through_every_step() {
        let end = walk(&[
            (Step::Intro, "start"),
            (Step::Name, "Aspen"),
            (Step::Focus, "store"),
            (Step::Connect, "connected"),
            (Step::Day, "done"),
            (Step::Daily, "weekdays"),
            (Step::Hire, "hired"),
            (Step::Set, "finish"),
        ]);
        assert_eq!(end, Step::Done);
    }

    #[test]
    fn not_now_on_connect_goes_to_the_three_things_fallback_without_an_error() {
        assert_eq!(transition(Step::Connect, "not_now").unwrap(), (Step::Plate, Effect::None));
        // A skipped connect card (the card's own Skip) is the same not now.
        assert_eq!(transition(Step::Connect, tools::origin::SKIP_SENTINEL).unwrap().0, Step::Plate);
        assert_eq!(transition(Step::Plate, "done").unwrap().0, Step::Daily);
        // The fallback names no error.
        let g = guide(Step::Plate, &Facts { owner: Some("Alma".into()), assistant: "Nebo".into(), ..Default::default() });
        assert!(g.contains("three things on your plate"));
        for banned in ["unavailable", "sorry", "error on my end"] {
            assert!(!g.to_lowercase().contains(&format!("\"{banned}")), "{g}");
        }
    }

    #[test]
    fn skip_moves_on_and_finish_ends_from_anywhere() {
        assert_eq!(transition(Step::Name, "skip").unwrap(), (Step::Focus, Effect::None));
        assert_eq!(transition(Step::Focus, "skip").unwrap(), (Step::Connect, Effect::None));
        assert_eq!(transition(Step::Hire, "skip").unwrap(), (Step::Set, Effect::None));
        assert_eq!(transition(Step::Daily, "not_now").unwrap(), (Step::Hire, Effect::Daily(Daily::Off)));
        for step in [Step::Name, Step::Focus, Step::Connect, Step::Day, Step::Daily, Step::Hire] {
            assert_eq!(transition(step, "finish").unwrap().0, Step::Done);
        }
        assert!(transition(Step::Done, "anything").is_err());
    }

    #[test]
    fn answers_carry_their_change() {
        assert_eq!(transition(Step::Name, " Logan ").unwrap().1, Effect::Name("Logan".into()));
        assert!(transition(Step::Name, "").is_err());
        assert_eq!(transition(Step::Focus, "home").unwrap().1, Effect::Focus(Focus::Home));
        assert!(transition(Step::Focus, "gardening").is_err());
        assert_eq!(transition(Step::Daily, "weekdays").unwrap().1, Effect::Daily(Daily::At("0 30 7 * * 1-5".into())));
        assert_eq!(transition(Step::Daily, "daily").unwrap().1, Effect::Daily(Daily::At("0 0 7 * * *".into())));
        assert_eq!(transition(Step::Daily, "6:45").unwrap().1, Effect::Daily(Daily::At("0 45 6 * * *".into())));
        assert!(transition(Step::Daily, "25:00").is_err());
        assert_eq!(Focus::Home.account_type(), "personal");
        assert_eq!(Focus::Store.hire_listing(), "store-manager");
    }

    #[test]
    fn the_steps_round_trip_through_storage() {
        for step in [Step::Intro, Step::Name, Step::Focus, Step::Connect, Step::Plate, Step::Day, Step::Daily, Step::Hire, Step::Set, Step::Done] {
            assert_eq!(Step::parse(step.as_str()), Some(step));
        }
    }

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("o.db").to_string_lossy()).unwrap();
        (dir, store)
    }

    #[test]
    fn a_fresh_install_starts_at_the_intro_and_resumes_where_it_stopped() {
        let (_d, store) = store();
        assert_eq!(current(&store), Step::Intro);
        store.set_onboarding("daily", Some("clients"), None).unwrap();
        assert_eq!(current(&store), Step::Daily, "resume mid-flow at the stored step");
        let p = store.get_user_profile().unwrap().unwrap();
        assert_eq!(p.onboarding_focus.as_deref(), Some("clients"));
        // A later step keeps the earlier answer.
        store.set_onboarding("hire", None, None).unwrap();
        assert_eq!(store.get_user_profile().unwrap().unwrap().onboarding_focus.as_deref(), Some("clients"));
    }

    #[test]
    fn an_install_from_before_the_guided_chat_is_done() {
        let (_d, store) = store();
        let _ = store.ensure_plugin_registry_entry("core");
        store.set_plugin_setting("core", "primary_christened", "1").unwrap();
        assert_eq!(current(&store), Step::Done);
        assert_eq!(store.get_user_profile().unwrap().unwrap().onboarding_step.as_deref(), Some("done"));
    }

    #[test]
    fn the_guide_says_the_trust_line_before_connecting_and_offers_the_names() {
        let f = Facts { owner: Some("Alma".into()), assistant: "Nebo".into(), ..Default::default() };
        let name = guide(Step::Name, &f);
        assert!(name.contains("Hi Alma!") && name.contains("Nebo, Aspen, Logan, Something else…"));
        let connect = guide(Step::Connect, &f);
        let trust = connect.find("never send, delete or accept").unwrap();
        let cards = connect.find("connect cards").unwrap();
        assert!(trust < cards, "the trust line comes before the connect card");
        let about = guide(Step::Focus, &f);
        assert!(about.contains("Nothing goes out without your OK"));
        assert!(!about.contains("I am a computer"));
    }

    #[test]
    fn make_it_daily_edits_the_one_morning_brief_schedule() {
        let edit = daily_edit(&Daily::At("0 30 7 * * 1-5".into()));
        assert_eq!((edit["schedule"].as_str(), edit["enabled"].as_bool()), (Some("0 30 7 * * 1-5"), Some(true)));
        let off = daily_edit(&Daily::Off);
        assert_eq!(off["enabled"], false);
        assert!(off.get("schedule").is_none(), "not now leaves the time the owner had");
        // The schedule edited is the Inbox's morning brief, not a second one.
        assert_eq!(super::super::notification::MORNING_BRIEF, "morning-brief");
    }

    #[test]
    fn the_day_is_the_one_morning_brief() {
        let day = guide(Step::Day, &Facts::default());
        assert!(day.contains(super::super::notification::MORNING_BRIEF_ASK), "the same brief the daily schedule writes");
    }

    #[test]
    fn the_checklist_says_only_what_is_true() {
        let mut f = Facts { assistant: "Aspen".into(), ..Default::default() };
        assert_eq!(f.checklist(), vec!["named your assistant Aspen".to_string()]);
        f.google_connected = true;
        f.brief_daily = true;
        f.hired = Some("Store Manager".into());
        assert_eq!(f.checklist().len(), 4);
    }
}
