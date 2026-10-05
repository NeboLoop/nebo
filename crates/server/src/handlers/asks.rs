//! What waits on the owner's answer, from every employee and helper: the
//! questions runs are parked on (`ask_owner`, a linked agent's own
//! permission question, an install or sign-in card) and the permission asks.
//! This is the ONE list every surface reads — the pinned bar in the app and
//! on the phone, the owner's live call, the voice `status` — and the ONE
//! place the owner's own words, typed or spoken, become an answer.
//!
//! Words become an answer through one typed decision (Jev through Janus,
//! [`ai::DecideClient`]): which of the question's options the owner's words
//! are, or an answer of their own (the card's "Other"), or not an answer at
//! all (a new request, another question). The owner may speak any language;
//! the decision reads the words in the language of the conversation they came
//! from. Words that repeat an option exactly are that option without asking.
//! When no decision can be had, the words are not an answer: the question
//! stays open and visible, and the words are handled as a new message. A
//! guessed answer is the costlier mistake.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::extract::State;
use axum::response::Json;
use serde::Serialize;
use tracing::{info, warn};

use agent::harness::permissions::{Answer, AnsweredVia, Ask, AskError};
use ai::{DecideClient, Question};

use super::HandlerResult;
use crate::handlers::chat::PendingAsk;
use crate::handlers::permissions::PermissionAskCard;
use crate::state::AppState;

/// One thing waiting on the owner's answer, as every surface shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WaitingAsk {
    /// The id its answer goes to: the parked question's request id, or the
    /// permission ask's id.
    pub id: String,
    /// question (a run is parked on it) | permission (a permission ask)
    pub kind: String,
    pub agent_id: String,
    /// The employee who is waiting.
    pub employee: String,
    pub session_key: String,
    /// The conversation it waits in; empty when the session holds none.
    pub chat_id: String,
    /// What the owner is asked, in the employee's words.
    pub question: String,
    /// The answers it offers, in order.
    pub options: Vec<String>,
    /// What a tap on each option sends: a question's `ask_response` value,
    /// a permission ask's `answer`.
    pub values: Vec<String>,
    /// Whether an answer in the owner's own words is taken (the card's
    /// "Other", or a question with no options).
    pub free_text: bool,
    /// Whether it can be answered in words at all. An install or sign-in
    /// card is done in the app.
    pub answerable: bool,
    /// A run is parked on the answer (a question always; a permission ask
    /// when a workflow step waits on it). The owner is told at once,
    /// wherever he is — a push, the desktop's notice — and it is decided on
    /// its card, never by the chat he happens to be in.
    pub blocking: bool,
    /// Unix seconds when it started waiting.
    pub created_at: i64,
}

/// The waiting asks, oldest first.
#[derive(Debug, Serialize)]
pub struct WaitingAsksResponse {
    pub asks: Vec<WaitingAsk>,
    pub total: usize,
}

/// How an ask is answered.
#[derive(Debug, Clone, PartialEq)]
enum Answers {
    /// A parked question: its call receives the option's value (the
    /// option's own words, or the owner's answer a linked agent's card
    /// names for it) or the owner's own words.
    Question,
    /// A permission ask: the answer each option is.
    Permission(Vec<Answer>),
}

/// A waiting ask and what its options answer with.
#[derive(Debug, Clone)]
pub(crate) struct Waiting {
    pub card: WaitingAsk,
    answers: Answers,
}

/// GET /api/v1/asks — everything waiting on the owner's answer, from every
/// employee, oldest first.
pub async fn list_waiting_asks(
    State(state): State<AppState>,
) -> HandlerResult<WaitingAsksResponse> {
    let asks: Vec<WaitingAsk> = waiting(&state, None)
        .await
        .into_iter()
        .map(|w| w.card)
        .collect();
    let total = asks.len();
    Ok(Json(WaitingAsksResponse { asks, total }))
}

/// Everything waiting on the owner, or only what waits in `session`: the
/// questions runs are parked on and the open permission asks, oldest first.
pub(crate) async fn waiting(state: &AppState, session: Option<&str>) -> Vec<Waiting> {
    let mut out: Vec<Waiting> = state
        .run_registry
        .pending_asks()
        .await
        .into_iter()
        .filter(|(key, _)| session.is_none_or(|s| s == key))
        .map(|(key, ask)| from_parked(state, &key, &ask))
        .collect();
    match state.permission_asks.open(session) {
        Ok(asks) => out.extend(asks.iter().map(|a| from_permission(state, a))),
        Err(e) => warn!(error = ?e, "the open permission asks could not be read"),
    }
    out.sort_by_key(|w| w.card.created_at);
    out
}

/// How many blocking asks wait on the owner, by employee id: what the
/// roster's "Waiting for you" marker counts.
pub(crate) async fn blocking_by_employee(state: &AppState) -> std::collections::HashMap<String, usize> {
    count_blocking(waiting(state, None).await.iter().map(|w| &w.card))
}

fn count_blocking<'a>(asks: impl Iterator<Item = &'a WaitingAsk>) -> std::collections::HashMap<String, usize> {
    let mut by = std::collections::HashMap::new();
    for w in asks.filter(|w| w.blocking) {
        *by.entry(w.agent_id.clone()).or_insert(0) += 1;
    }
    by
}

fn employee_of(state: &AppState, session_key: &str) -> (String, String) {
    let agent_id = types::keyparser::extract_agent_id(session_key);
    let employee = crate::handlers::permissions::employee_name(state, &agent_id);
    (agent_id, employee)
}

/// A parked question as a waiting ask.
fn from_parked(state: &AppState, session_key: &str, ask: &PendingAsk) -> Waiting {
    let (agent_id, employee) = employee_of(state, session_key);
    let chat_id = tools::owner_notify::link::conversation(&state.store, session_key);
    let shape = parked_shape(ask);
    Waiting {
        card: WaitingAsk {
            id: ask.request_id.clone(),
            kind: "question".into(),
            agent_id,
            employee,
            session_key: session_key.to_string(),
            chat_id,
            question: shape.question,
            options: shape.options,
            values: shape.values,
            free_text: shape.free_text,
            answerable: shape.answerable,
            blocking: true,
            created_at: ask.created_at,
        },
        answers: Answers::Question,
    }
}

/// What a parked question's card offers.
struct ParkedShape {
    question: String,
    options: Vec<String>,
    values: Vec<String>,
    free_text: bool,
    answerable: bool,
}

fn parked_shape(ask: &PendingAsk) -> ParkedShape {
    let widget = ask.widgets.as_ref().and_then(|w| w.get(0));
    let field = |key: &str| widget.and_then(|w| w.get(key)).and_then(|v| v.as_str());
    let kind = field("type").unwrap_or("");
    let name = field("name")
        .or_else(|| field("label"))
        .or_else(|| field("plugin"))
        .unwrap_or("that app");
    let in_app = |question: String| ParkedShape {
        question,
        options: Vec::new(),
        values: Vec::new(),
        free_text: false,
        answerable: false,
    };
    match kind {
        "install_plugin" => {
            return in_app(format!(
                "Install {name} in the Nebo app so the work can go on."
            ));
        }
        "connect_account" => {
            return in_app(format!(
                "Sign in to {name} in the Nebo app so the work can go on."
            ));
        }
        "hire_employee" => return in_app(ask.prompt.trim().to_string()),
        _ => {}
    }
    let options: Vec<String> = widget
        .and_then(|w| w.get("options"))
        .and_then(|o| o.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    o.as_str()
                        .or_else(|| o.get("label").and_then(|l| l.as_str()))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // A linked agent's own permission question names the owner's answer
    // each option is; its asker maps that answer to its option.
    let named: Option<Vec<Option<String>>> = widget
        .and_then(|w| w.get("answers"))
        .and_then(|a| a.as_array())
        .map(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
    let values = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            named
                .as_ref()
                .and_then(|n| n.get(i).cloned().flatten())
                .unwrap_or_else(|| label.clone())
        })
        .collect();
    ParkedShape {
        question: ask.prompt.trim().to_string(),
        options,
        values,
        // Every option card takes an answer of the owner's own ("Other"),
        // except a linked agent's permission question: its asker takes only
        // the answers it names.
        free_text: named.is_none(),
        answerable: true,
    }
}

/// A permission ask as a waiting ask.
fn from_permission(state: &AppState, ask: &Ask) -> Waiting {
    permission_waiting(&crate::handlers::permissions::card(state, ask))
}

fn permission_waiting(c: &PermissionAskCard) -> Waiting {
    let offered = crate::permission_asks::offered(c);
    let chat_id = c.chat_id.clone();
    Waiting {
        card: WaitingAsk {
            id: c.id.clone(),
            kind: "permission".into(),
            agent_id: c.agent_id.clone(),
            employee: c.employee.clone(),
            session_key: c.session_key.clone(),
            chat_id,
            question: crate::permission_asks::question(c),
            options: offered.iter().map(|(label, _)| label.to_string()).collect(),
            values: offered
                .iter()
                .map(|(_, a)| a.as_str().to_string())
                .collect(),
            free_text: false,
            answerable: true,
            blocking: c.blocking,
            created_at: c.created_at,
        },
        answers: Answers::Permission(offered.into_iter().map(|(_, a)| a).collect()),
    }
}

/// "Nanna is waiting on your answer: …" — how every surface says it.
pub(crate) fn waiting_line(w: &WaitingAsk) -> String {
    if !w.answerable {
        return format!("{} is waiting on you: {}", w.employee, w.question);
    }
    format!(
        "{} is waiting on your answer: {}",
        w.employee,
        w.question.trim()
    )
}

/// The waiting asks as the owner's call is told of them: who asks, what,
/// the answers it takes, and the id the answer goes to. Spoken in the
/// language of the call, never read out as written.
pub(crate) fn on_call(w: &WaitingAsk) -> String {
    if !w.answerable {
        return format!(
            "({} is waiting on the owner: {} Tell the owner in one short sentence, in the language they are \
             speaking on this call, naming {}. It is done in the app, never on this call.)",
            w.employee,
            w.question.trim(),
            w.employee
        );
    }
    let choices = match w.options.as_slice() {
        [] => String::new(),
        options => format!(" The answers it takes: {}.", join_options(options)),
    };
    format!(
        "({employee} is waiting on the owner's answer, ask {id}: \"{question}\"{choices} Put it to the owner now in one \
         short question, in the language they are speaking on this call, and say who is asking, like \"{employee} \
         asks: … Yes or no?\". When they answer, call answer_ask with ask_id \"{id}\" and their words exactly as they \
         said them.)",
        employee = w.employee,
        id = w.id,
        question = w.question.trim(),
    )
}

/// The most of a question a notice in another conversation says.
const NOTICE_CHARS: usize = 160;

/// A blocking ask from ANOTHER conversation, as a call is told of it: a
/// notice and nothing more. The call names who waits and on what, in one
/// short sentence; it never asks for the answer, never answers it, and
/// carries no id to answer it with. The ask is decided on its own card —
/// the notification, the phone, the Inbox, or the conversation that raised
/// it — never by the employee the owner is talking to (live 2026-10-02: on
/// a call with Flip-Flap the owner was put Bookkeeper's workflow ask, said
/// "No.", and Flip-Flap answered it: "Told Bookkeeper no.").
pub(crate) fn notice_on_call(w: &WaitingAsk) -> String {
    format!(
        "(A notice, not a request, and not this conversation's: {employee} is waiting on the owner elsewhere. Tell \
         the owner once, in one short sentence, in the language they are speaking on this call: \"{headline}: \
         {question}\". Nothing more: don't ask them for the answer and never answer it yourself. It is answered on \
         its card (the notification, the mobile app or the Inbox), never on this call.)",
        employee = w.employee,
        headline = waiting_on_you(&w.employee),
        question = notice_question(w),
    )
}

/// The question as a notice says it, clipped to [`NOTICE_CHARS`].
pub(crate) fn notice_question(w: &WaitingAsk) -> String {
    let q = w.question.trim();
    match q.char_indices().nth(NOTICE_CHARS) {
        Some((cut, _)) => format!("{}…", q[..cut].trim_end()),
        None => q.to_string(),
    }
}

/// "A", "A or B", "A, B, or C".
pub(crate) fn join_options<S: AsRef<str>>(options: &[S]) -> String {
    match options {
        [] => String::new(),
        [one] => one.as_ref().to_string(),
        [first, second] => format!("{} or {}", first.as_ref(), second.as_ref()),
        [head @ .., last] => format!(
            "{}, or {}",
            head.iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(", "),
            last.as_ref()
        ),
    }
}

// ── The owner's words as an answer ─────────────────────────────────────────

/// UNTUNED. The chance the words are not an answer at or over which they are
/// not taken as one, whatever option was picked: a near tie keeps the
/// question open, because a guessed answer is the costlier mistake.
const NOT_AN_ANSWER_AT: f64 = 0.3;
/// Bounds a decision that hangs. Jev answers in about 200 ms.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// The most of the owner's words and of the question the decision reads.
const WORDS_CAP: usize = 2_000;
/// The decision's one question.
const REPLY: &str = "reply";
const NOT_AN_ANSWER: &str = "not_an_answer";
const OWN_WORDS: &str = "own_words";

/// What the owner's words are, read against one waiting ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reading {
    /// That option, by its place in the ask's options.
    Option(usize),
    /// An answer in the owner's own words (the card's "Other").
    OwnWords,
    /// Not an answer: a new message.
    NotAnAnswer,
}

/// The option the words repeat exactly (case, spacing and closing
/// punctuation aside): what a tap on it would send.
fn exact_option(options: &[String], words: &str) -> Option<usize> {
    let norm = |s: &str| {
        s.trim()
            .trim_end_matches(['.', '!', '?', '。', '！', '？'])
            .trim()
            .to_lowercase()
    };
    let said = norm(words);
    options.iter().position(|o| norm(o) == said)
}

/// The permission answer a short word names, beside the card's own labels
/// ("Allow always", "This once", "No"): "always", "once" and "deny" (the
/// words the app's buttons read for a day) are the same answers.
fn permission_word(w: &Waiting, words: &str) -> Option<usize> {
    let Answers::Permission(answers) = &w.answers else { return None };
    let said = words.trim().trim_end_matches(['.', '!', '?']).trim().to_lowercase();
    let meant = match said.as_str() {
        "always" => Answer::AllowAlways,
        "once" => Answer::ThisOnce,
        "deny" => Answer::No,
        _ => return None,
    };
    answers.iter().position(|a| *a == meant)
}

/// What a permission answer means, for the decision: the card's labels are
/// short, and the owner says "yes" or "go ahead" far more often.
fn permission_meaning(answer: Answer) -> &'static str {
    match answer {
        Answer::AllowAlways => "Yes, and don't ask again for this in future",
        Answer::ThisOnce => "Yes: go ahead this time (a plain yes, okay, go ahead, do it)",
        Answer::No => "No: don't do it",
        Answer::Sent => "Yes, it went out",
        Answer::NotSent => "No, it didn't go out",
    }
}

fn reply_question(w: &Waiting) -> Question {
    let mut criteria: BTreeMap<String, String> = BTreeMap::new();
    for (i, label) in w.card.options.iter().enumerate() {
        let meaning = match &w.answers {
            Answers::Permission(answers) => format!("{label}: {}", permission_meaning(answers[i])),
            Answers::Question => label.clone(),
        };
        criteria.insert(format!("option_{}", i + 1), meaning);
    }
    if w.card.free_text {
        criteria.insert(
            OWN_WORDS.into(),
            if w.card.options.is_empty() {
                "An answer to the question".into()
            } else {
                "An answer to the question in the owner's own words that is none of the listed answers".into()
            },
        );
    }
    criteria.insert(
        NOT_AN_ANSWER.into(),
        "Not an answer to this question: a new request or instruction, a different question, or a remark about \
         something else"
            .into(),
    );
    Question::Choice {
        instructions: "The owner was asked `question`; `options` are the answers it offers. Then the owner said \
                       `reply`. `conversation` is what the owner has been saying, in the language they speak, and \
                       `reply` is in that language. What `reply` is."
            .into(),
        criteria,
    }
}

/// The state the decision reads. The owner's words are data, never part of
/// an instruction.
fn reply_state(w: &Waiting, words: &str, conversation: &str) -> serde_json::Value {
    serde_json::json!({
        "question": ai::decide::clip(w.card.question.trim(), WORDS_CAP),
        "options": w.card.options,
        "reply": ai::decide::clip(words.trim(), WORDS_CAP),
        "conversation": ai::decide::clip(conversation.trim(), WORDS_CAP),
    })
}

/// The reading a decision states: not an answer when that is picked or its
/// chance reaches [`NOT_AN_ANSWER_AT`]; else the option picked. A missing or
/// unknown answer is not an answer.
fn reading_from(decision: &ai::Decision, w: &Waiting) -> Reading {
    let Some(answer) = decision.answer(REPLY) else {
        return Reading::NotAnAnswer;
    };
    if answer
        .probabilities
        .get(NOT_AN_ANSWER)
        .is_some_and(|p| *p >= NOT_AN_ANSWER_AT)
    {
        return Reading::NotAnAnswer;
    }
    let picked = answer.picked();
    if picked == OWN_WORDS && w.card.free_text {
        return Reading::OwnWords;
    }
    picked
        .strip_prefix("option_")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=w.card.options.len()).contains(n))
        .map_or(Reading::NotAnAnswer, |n| Reading::Option(n - 1))
}

/// Read the owner's `words` against `w`. `conversation` is what the owner
/// has been saying there, the language the words are read in. Words that
/// repeat an option are that option; anything else is Jev's to read, and
/// with no decision the words are not an answer.
pub(crate) async fn read_reply(
    decide: Option<&DecideClient>,
    w: &Waiting,
    words: &str,
    conversation: &str,
) -> Reading {
    if words.trim().is_empty() {
        return Reading::NotAnAnswer;
    }
    if let Some(i) = exact_option(&w.card.options, words).or_else(|| permission_word(w, words)) {
        return Reading::Option(i);
    }
    let Some(client) = decide else {
        warn!(site = "ask_reply", ask = %w.card.id, reason = "no_client", "the owner's words are not taken as an answer");
        return Reading::NotAnAnswer;
    };
    let trace = ai::RequestTrace {
        agent_id: w.card.agent_id.clone(),
        ..ai::RequestTrace::new("ask_reply")
    };
    let questions = BTreeMap::from([(REPLY, reply_question(w))]);
    let decision = match tokio::time::timeout(
        READ_TIMEOUT,
        client.decide(&trace, &reply_state(w, words, conversation), &questions),
    )
    .await
    {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            warn!(site = "ask_reply", ask = %w.card.id, error = %e, "the owner's words are not taken as an answer");
            return Reading::NotAnAnswer;
        }
        Err(_) => {
            warn!(site = "ask_reply", ask = %w.card.id, reason = "timeout", "the owner's words are not taken as an answer");
            return Reading::NotAnAnswer;
        }
    };
    let reading = reading_from(&decision, w);
    let answer = decision.answer(REPLY);
    info!(
        site = "ask_reply",
        ask = %w.card.id,
        agent = %w.card.agent_id,
        outcome = ?reading,
        choice = answer.map(|a| a.picked()).unwrap_or(""),
        confidence = answer.and_then(|a| a.confidence).unwrap_or(-1.0),
        p_not_an_answer = answer.and_then(|a| a.probabilities.get(NOT_AN_ANSWER).copied()).unwrap_or(-1.0),
        model = %decision.model,
        cost_micro = decision.usage.cost_micro,
        "the owner's words read against a waiting ask"
    );
    reading
}

/// The owner's words turn a card down: "skip" anywhere, or a short reply
/// that is "no", "not now", "cancel", "never mind". A sentence that only
/// starts with "no" is about something else. ponytail: English only; read
/// them with the decide model, as an answerable question is, if calls in
/// other languages need it.
fn dismisses(words: &str) -> bool {
    let said: String = words
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect();
    let said: Vec<&str> = said.split_whitespace().collect();
    if said.contains(&"skip") {
        return true;
    }
    const PHRASES: &[&str] = &["no", "nope", "not now", "cancel", "never mind", "nevermind", "forget it", "no thanks"];
    let short = said.len() <= 4;
    let text = said.join(" ");
    short && PHRASES.iter().any(|p| text == *p || text.starts_with(&format!("{p} ")))
}

/// Why the owner's words answered nothing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NotAnswered {
    /// The words are not an answer to it.
    NotAnAnswer,
    /// It is done in the app, never in words.
    InApp,
    /// Someone answered it first, or the work it was for ended.
    Settled,
    /// The answer could not be recorded.
    Failed(String),
}

/// Answer `w` with the owner's `words`, through the ask's one answer path:
/// the parked call's oneshot, or the permission ask's answer. Returns the
/// answer given, as the owner would read it on the card.
pub(crate) async fn answer_in_words(
    state: &AppState,
    decide: Option<&DecideClient>,
    w: &Waiting,
    words: &str,
    conversation: &str,
    via: AnsweredVia,
) -> Result<String, NotAnswered> {
    if !w.card.answerable {
        // A card done in the app (install, connect) is still the owner's to
        // turn down in words: "skip" on his call or in the chat dismisses it
        // and the work goes on without it. Live 2026-10-04: a Shopify
        // connect card held a design session on a call until he found the
        // Skip button on his phone.
        if matches!(w.answers, Answers::Question) && dismisses(words) {
            if !crate::chat_dispatch::answer_ask(state, &w.card.id, "skipped".to_string()).await {
                return Err(NotAnswered::Settled);
            }
            info!(ask = %w.card.id, via = via.as_str(), "the owner's words skipped an in-app card");
            return Ok("Skip".to_string());
        }
        return Err(NotAnswered::InApp);
    }
    let reading = read_reply(decide, w, words, conversation).await;
    let given = match reading {
        Reading::NotAnAnswer => return Err(NotAnswered::NotAnAnswer),
        Reading::Option(i) => w.card.options[i].clone(),
        Reading::OwnWords => words.trim().to_string(),
    };
    match (&w.answers, reading) {
        (Answers::Question, _) => {
            let value = match reading {
                Reading::Option(i) => w.card.values[i].clone(),
                _ => given.clone(),
            };
            if !crate::chat_dispatch::answer_ask(state, &w.card.id, value).await {
                return Err(NotAnswered::Settled);
            }
        }
        (Answers::Permission(answers), Reading::Option(i)) => {
            match state.permission_asks.answer(&w.card.id, answers[i], via) {
                Ok(_) => {}
                Err(AskError::Settled(_)) | Err(AskError::NotFound) => {
                    return Err(NotAnswered::Settled);
                }
                Err(e) => return Err(NotAnswered::Failed(format!("{e:?}"))),
            }
        }
        (Answers::Permission(_), _) => return Err(NotAnswered::NotAnAnswer),
    }
    info!(ask = %w.card.id, via = via.as_str(), answer = %given, "the owner's words answered a waiting ask");
    Ok(given)
}

/// The owner's message in conversation `session_key`, read against the
/// newest ask waiting there that words can answer. True when it answered
/// it: the message is that answer and starts nothing of its own. False
/// leaves the question open and visible, and the message is a new one.
pub(crate) async fn answered_by_message(
    state: &AppState,
    session_key: &str,
    words: &str,
    via: AnsweredVia,
) -> bool {
    let here = waiting(state, Some(session_key)).await;
    let Some(w) = here
        .iter()
        .rev()
        .find(|w| w.card.answerable || (matches!(w.answers, Answers::Question) && dismisses(words)))
    else {
        return false;
    };
    match answer_in_words(state, state.decide.as_deref(), w, words, words, via).await {
        Ok(_) => true,
        Err(why) => {
            info!(session = %session_key, ask = %w.card.id, ?why, "the owner's message is a new one; the question stays open");
            false
        }
    }
}

// ── Where a waiting ask is surfaced ────────────────────────────────────────

/// The hub Inbox id a parked question is pushed to the phone under.
pub(crate) fn push_id(request_id: &str) -> String {
    format!("question-ask:{request_id}")
}

/// "Ava is waiting on you": the headline of a blocking ask on every
/// surface that tells the owner of it (the push, the desktop's notice, a
/// call in another conversation).
pub(crate) fn waiting_on_you(employee: &str) -> String {
    format!("{employee} is waiting on you")
}

/// What a blocking ask's hub item carries beside its words: it is pushed on
/// its own, at once (never held for a batch), and the phone presents it
/// even in the foreground, at iOS's time-sensitive level. The hub passes
/// both on to the push (`data.blocking`, `aps.interruption-level`).
pub(crate) fn blocking_fields() -> serde_json::Value {
    serde_json::json!({ "blocking": true, "interruptionLevel": "time-sensitive" })
}

/// A parked question as the owner's hub item, which pushes it to his phone:
/// who is waiting, on what, and a link into the conversation where it waits
/// (the pinned bar there answers it). `askId` names the ask. A parked
/// question always blocks its run.
pub(crate) fn push_item(w: &WaitingAsk) -> serde_json::Value {
    let id = push_id(&w.id);
    let title = waiting_on_you(&w.employee);
    let link = tools::owner_notify::link::chat(&w.agent_id, &w.chat_id);
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "question_ask",
        title: &title,
        body: Some(w.question.trim()),
        action_url: Some(&link),
        agent_id: (!w.agent_id.is_empty()).then_some(w.agent_id.as_str()),
        loud: true,
    };
    let mut extra = blocking_fields();
    extra["chatId"] = serde_json::json!(w.chat_id);
    extra["askId"] = serde_json::json!(w.id);
    n.hub_item(extra)
}

/// A run parked on a question: the owner's live calls are told now, the
/// phone is pushed a link to it, and every app's pinned bar is updated.
pub(crate) async fn question_raised(state: &AppState, session_key: &str, ask: &PendingAsk) {
    let w = from_parked(state, session_key, ask);
    crate::handlers::voice::tell_calls(&state.live_calls, &w.card);
    crate::codes::push_inbox(state, push_item(&w.card));
    changed(state).await;
}

/// A parked question was answered, or the run it held ended: its push is
/// resolved and every pinned bar updated.
pub(crate) async fn question_settled(state: &AppState, request_id: &str) {
    crate::codes::push_inbox(
        state,
        serde_json::json!({ "id": push_id(request_id), "resolved": true }),
    );
    changed(state).await;
}

/// A permission ask was raised, or the owner is reminded of it: his live
/// calls are told, and every pinned bar updated. (Its Inbox row and push
/// are the permission surfaces' own.)
pub(crate) fn permission_raised(state: &AppState, card: &PermissionAskCard) {
    let w = permission_waiting(card);
    crate::handlers::voice::tell_calls(&state.live_calls, &w.card);
    spawn_changed(state);
}

/// What waits on the owner changed from a place that can't wait on it.
pub(crate) fn spawn_changed(state: &AppState) {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let state = state.clone();
        rt.spawn(async move { changed(&state).await });
    }
}

/// What waits on the owner changed: every app gets the whole list
/// (`asks_waiting`), the same one `GET /asks` returns.
pub(crate) async fn changed(state: &AppState) {
    let asks: Vec<WaitingAsk> = waiting(state, None)
        .await
        .into_iter()
        .map(|w| w.card)
        .collect();
    let total = asks.len();
    state.hub.broadcast(
        "asks_waiting",
        serde_json::json!({ "asks": asks, "total": total }),
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub(crate) fn question(id: &str, prompt: &str, options: &[&str]) -> Waiting {
        let ask = PendingAsk {
            request_id: id.into(),
            prompt: prompt.into(),
            widgets: Some(
                serde_json::json!([{ "type": "options", "multiSelect": false, "options": options }]),
            ),
            created_at: 100,
        };
        let shape = parked_shape(&ask);
        Waiting {
            card: WaitingAsk {
                id: id.into(),
                kind: "question".into(),
                agent_id: "nanna".into(),
                employee: "Nanna".into(),
                session_key: "agent:nanna:thread:t1".into(),
                chat_id: "t1".into(),
                question: shape.question,
                options: shape.options,
                values: shape.values,
                free_text: shape.free_text,
                answerable: shape.answerable,
                blocking: true,
                created_at: 100,
            },
            answers: Answers::Question,
        }
    }

    /// A stand-in for Jev that answers the `reply` question from a table of
    /// what the owner said, and keeps every state it was sent.
    pub(crate) async fn table_jev(
        table: &'static [(&'static str, &'static str)],
    ) -> (DecideClient, Arc<Mutex<Vec<serde_json::Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let kept = kept.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let start = loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break end + 4;
                            }
                        }
                    };
                    let req: serde_json::Value =
                        serde_json::from_slice(&buf[start..]).unwrap_or_default();
                    kept.lock().unwrap().push(req.clone());
                    let reply = req["state"]["reply"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let choice = table
                        .iter()
                        .find(|(said, _)| *said == reply)
                        .map_or(NOT_AN_ANSWER, |(_, c)| *c);
                    let body = serde_json::json!({
                        "model": "jev-test",
                        "answers": { REPLY: { "type": "choice", "choice": choice, "confidence": 0.95,
                            "probabilities": { choice: 0.95 } } },
                        "usage": {}
                    })
                    .to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        let client = DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer {
                token: "t".into(),
                bot_id: None,
            })
        });
        (client, seen)
    }

    const INCIDENT: &str = "Rather than creating dozens of new sub-agents, I'll set up a single daily schedule at 7 AM. Want me to do that?";
    const YES: &str = "Yes, set up the daily schedule as described";
    const NO: &str = "No, let me explain differently";

    /// The owner's "yes" in English, Japanese and Mandarin, and his
    /// "go ahead" and "sí", are the Yes option; the decision reads each in
    /// the language of its conversation. A new request is no answer.
    #[tokio::test]
    async fn the_owners_yes_in_any_language_is_the_yes_option() {
        let (jev, seen) = table_jev(&[
            ("yes", "option_1"),
            ("go ahead", "option_1"),
            ("sí", "option_1"),
            ("はい", "option_1"),
            ("好的", "option_1"),
            ("no thanks", "option_2"),
            ("make it at 6 instead", "own_words"),
        ])
        .await;
        let w = question("q1", INCIDENT, &[YES, NO]);
        for (words, conversation) in [
            ("yes", "Set up a daily stand-up across all employees."),
            ("go ahead", "Set up a daily stand-up across all employees."),
            (
                "sí",
                "Configura una reunión diaria con todos los empleados.",
            ),
            ("はい", "全員で毎日のスタンドアップを設定して。"),
            ("好的", "为所有员工设置每日站会。"),
        ] {
            assert_eq!(
                read_reply(Some(&jev), &w, words, conversation).await,
                Reading::Option(0),
                "{words}"
            );
        }
        assert_eq!(
            read_reply(Some(&jev), &w, "no thanks", "").await,
            Reading::Option(1)
        );
        assert_eq!(
            read_reply(Some(&jev), &w, "make it at 6 instead", "").await,
            Reading::OwnWords,
            "the card's Other"
        );
        assert_eq!(
            read_reply(Some(&jev), &w, "Which conversation is the AI", "").await,
            Reading::NotAnAnswer,
            "a new question is not the answer (live 2026-09-29)"
        );
        let seen = seen.lock().unwrap().clone();
        let japanese = seen
            .iter()
            .find(|r| r["state"]["reply"] == "はい")
            .expect("asked");
        assert_eq!(
            japanese["state"]["conversation"], "全員で毎日のスタンドアップを設定して。",
            "read in its conversation's language"
        );
        assert_eq!(japanese["state"]["question"], INCIDENT);
        assert_eq!(japanese["state"]["options"], serde_json::json!([YES, NO]));
        let criteria = &japanese["questions"][REPLY]["criteria"];
        assert_eq!(criteria["option_1"], YES);
        assert!(
            criteria[NOT_AN_ANSWER].is_string() && criteria[OWN_WORDS].is_string(),
            "{criteria}"
        );
        let instructions = japanese["questions"][REPLY]["instructions"]
            .as_str()
            .unwrap();
        assert!(
            !instructions.contains("はい") && !instructions.contains(INCIDENT),
            "the words are state, never instructions"
        );
    }

    /// Words that repeat an option are that option with no decision; with
    /// no decision anything else is not an answer, and nothing is guessed.
    /// The card's words, and the short ones, answer a permission ask
    /// without a decision: "Allow always" / "always", "This once" / "once",
    /// "No" / "deny".
    #[tokio::test]
    async fn the_cards_words_and_the_short_ones_answer_a_permission() {
        let w = Waiting {
            card: WaitingAsk {
                id: "p1".into(),
                kind: "permission".into(),
                agent_id: "a".into(),
                employee: "Ava".into(),
                session_key: String::new(),
                chat_id: String::new(),
                question: "OK to go ahead?".into(),
                options: vec!["Allow always".into(), "This once".into(), "No".into()],
                values: vec!["allow_always".into(), "this_once".into(), "no".into()],
                free_text: false,
                answerable: true,
                blocking: false,
                created_at: 0,
            },
            answers: Answers::Permission(vec![Answer::AllowAlways, Answer::ThisOnce, Answer::No]),
        };
        for (words, i) in [
            ("Allow always", 0),
            ("allow always", 0),
            ("always", 0),
            ("Always.", 0),
            ("This once", 1),
            ("this once", 1),
            ("once", 1),
            ("No", 2),
            ("no", 2),
            ("deny", 2),
            ("Deny!", 2),
        ] {
            assert_eq!(read_reply(None, &w, words, "").await, Reading::Option(i), "{words}");
        }
        assert_eq!(read_reply(None, &w, "maybe later", "").await, Reading::NotAnAnswer);
    }

    #[tokio::test]
    async fn with_no_decision_only_an_exact_option_answers() {
        let w = question("q1", INCIDENT, &[YES, NO]);
        assert_eq!(
            read_reply(None, &w, "yes, set up the daily schedule as described.", "").await,
            Reading::Option(0)
        );
        assert_eq!(read_reply(None, &w, "yes", "").await, Reading::NotAnAnswer);
        let dead = DecideClient::new("http://127.0.0.1:9", || {
            Some(ai::Bearer {
                token: "t".into(),
                bot_id: None,
            })
        });
        assert_eq!(
            read_reply(Some(&dead), &w, "yes", "").await,
            Reading::NotAnAnswer,
            "an unreachable Jev"
        );
        assert_eq!(
            read_reply(Some(&dead), &w, "  ", "").await,
            Reading::NotAnAnswer
        );
    }

    /// A near tie with "not an answer" keeps the question open.
    #[test]
    fn a_near_not_an_answer_is_no_answer() {
        let w = question("q1", INCIDENT, &[YES, NO]);
        let decision = |choice: &str, p_not: f64| ai::Decision {
            model: "jev-test".into(),
            answers: std::collections::HashMap::from([(
                REPLY.to_string(),
                ai::Answer {
                    kind: "choice".into(),
                    choice: Some(choice.into()),
                    score: None,
                    noul: None,
                    confidence: Some(0.6),
                    probabilities: BTreeMap::from([(NOT_AN_ANSWER.to_string(), p_not)]),
                },
            )]),
            usage: Default::default(),
        };
        assert_eq!(
            reading_from(&decision("option_1", 0.05), &w),
            Reading::Option(0)
        );
        assert_eq!(
            reading_from(&decision("option_1", NOT_AN_ANSWER_AT), &w),
            Reading::NotAnAnswer
        );
        assert_eq!(
            reading_from(&decision("option_9", 0.0), &w),
            Reading::NotAnAnswer,
            "no such option"
        );
        let linked = Waiting {
            card: WaitingAsk {
                free_text: false,
                ..w.card.clone()
            },
            answers: w.answers.clone(),
        };
        assert_eq!(
            reading_from(&decision(OWN_WORDS, 0.0), &linked),
            Reading::NotAnAnswer,
            "a card with no Other"
        );
    }

    /// A linked agent's permission question answers with the owner's answer
    /// its option names; an install card is done in the app.
    #[test]
    fn a_parked_card_says_what_it_takes() {
        let linked = PendingAsk {
            request_id: "toolu_1".into(),
            prompt: "Run git status?".into(),
            widgets: Some(
                serde_json::json!([{ "type": "options", "options": ["Allow once", "Always allow", "Deny"],
                "answers": ["this_once", "allow_always", "no"] }]),
            ),
            created_at: 1,
        };
        let shape = parked_shape(&linked);
        assert_eq!(shape.options, ["Allow once", "Always allow", "Deny"]);
        assert_eq!(shape.values, ["this_once", "allow_always", "no"]);
        assert!(shape.answerable && !shape.free_text);
        let install = PendingAsk {
            request_id: "r2".into(),
            prompt: "Install QuickBooks".into(),
            widgets: Some(serde_json::json!([{ "type": "install_plugin", "name": "QuickBooks" }])),
            created_at: 1,
        };
        let shape = parked_shape(&install);
        assert!(!shape.answerable && shape.options.is_empty());
        assert!(
            shape
                .question
                .contains("Install QuickBooks in the Nebo app"),
            "{}",
            shape.question
        );
    }

    /// What the call is told names who asks, the question, the answers and
    /// the real id its answer goes to.
    #[test]
    fn the_call_is_told_who_asks_what_and_the_id() {
        let w = question("bf8e2c67", INCIDENT, &[YES, NO]);
        let told = on_call(&w.card);
        assert!(
            told.contains("Nanna is waiting on the owner's answer, ask bf8e2c67"),
            "{told}"
        );
        assert!(
            told.contains(INCIDENT) && told.contains(&format!("{YES} or {NO}")),
            "{told}"
        );
        assert!(
            told.contains("Nanna asks:") && told.contains("in the language they are speaking"),
            "{told}"
        );
        assert!(
            told.contains("call answer_ask with ask_id \"bf8e2c67\""),
            "{told}"
        );
        assert_eq!(
            waiting_line(&w.card),
            format!("Nanna is waiting on your answer: {INCIDENT}")
        );
    }

    /// The phone's push opens the conversation the question waits in, where
    /// the pinned bar answers it, and names the ask.
    #[test]
    fn the_push_deep_links_to_the_ask() {
        let w = question("bf8e2c67", INCIDENT, &[YES, NO]);
        let item = push_item(&w.card);
        assert_eq!(item["id"], "question-ask:bf8e2c67");
        assert_eq!(item["type"], "question_ask");
        assert_eq!(item["link"], "/nanna/threads/t1");
        assert_eq!(item["chatId"], "t1");
        assert_eq!(item["askId"], "bf8e2c67");
        assert_eq!(item["agentId"], "nanna");
        assert_eq!(item["title"], "Nanna is waiting on you");
        assert_eq!(item["body"], INCIDENT);
        assert_eq!(item["blocking"], true, "a parked question blocks its run");
        assert_eq!(item["interruptionLevel"], "time-sensitive");
    }
}
