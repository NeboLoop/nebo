//! Addressing — THE routing model for team posts and messages between
//! employees (owner rule, 2026-09-28): "if you're asked something, you have
//! to be addressed in order to answer, and you answer once."
//!
//! Addressed means exactly one of:
//! - the owner's plain post in a team addresses the LEAD (every member, when
//!   the team has none);
//! - @Name addresses that employee; a message to a colleague addresses it;
//! - @everyone addresses every member;
//! - a hand-off: the lead, answering in its team, names a member (or an
//!   employee answering a message names a colleague).
//!
//! Nothing else addresses anyone: an answer, an acknowledgement, a status
//! line. An answer never addresses the one it answers — a question back to
//! whoever asked is the answer.
//!
//! [`who_is_addressed`] is the one decision, for every door: a team post
//! (the owner's, a tool's, a member's reply), a message between employees.
//! Each addressing is a row (`db::Addressing`, keyed post × employee), and
//! answering is keyed on it: a conversation that was addressed answers once
//! nothing it started is still out ([`turn_ended`]) — its hand-offs
//! answered, its helpers done — and whoever asked hears every answer it
//! waited for once, together, when the last one lands ([`hear`]). A seat
//! that has heard its answers is `Collected`: what it says next is its
//! answer, and it addresses no one.

use crate::state::AppState;
use db::{Addressing, AddressingState};
use types::provenance::ProvenanceClass;

/// How a post is said: what decides whether its names address anyone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Said {
    /// Not in the course of an answer: the owner's post, an employee's
    /// from its own conversation, a first message to a colleague.
    Deliberate,
    /// In the course of answering `askers` (agent ids; "" is the owner),
    /// before the answers it waits for are in. `in_room`: the addressing it
    /// answers is in the same team as the post.
    Working { askers: Vec<String>, in_room: bool },
    /// The answer, or anything said after it: it addresses no one.
    Answer,
}

/// The team a post is in.
pub(crate) struct Room<'a> {
    pub lead: Option<&'a str>,
    pub members: &'a [String],
}

/// One post, as the decision reads it.
pub(crate) struct Post<'a> {
    /// The posting employee; "" is the owner.
    pub author: &'a str,
    pub said: &'a Said,
    /// `None` for a message between employees.
    pub room: Option<Room<'a>>,
    /// The employees it names (ids): its @mentions, or a message's
    /// recipient.
    pub named: &'a [String],
    /// It names @everyone.
    pub everyone: bool,
}

/// An employee's plain post to a team with no lead: nobody would take it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NoLead;

/// Who `post` addresses. Pure — the whole routing policy.
pub(crate) fn who_is_addressed(post: &Post<'_>) -> Result<Vec<String>, NoLead> {
    let (askers, in_room): (&[String], bool) = match post.said {
        Said::Answer => return Ok(Vec::new()),
        Said::Working { askers, in_room } => (askers, *in_room),
        Said::Deliberate => (&[], false),
    };
    let lead = post.room.as_ref().and_then(|r| r.lead);
    // In its own team the lead runs the room: a member answering there
    // hands nothing off.
    if in_room && lead != Some(post.author) {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    let mut add = |id: &String| {
        if id != post.author && !askers.contains(id) && !out.contains(id) {
            out.push(id.clone());
        }
    };
    match &post.room {
        Some(room) => {
            if post.everyone {
                room.members.iter().for_each(&mut add);
            }
            post.named.iter().filter(|n| room.members.contains(n)).for_each(&mut add);
        }
        None => post.named.iter().for_each(&mut add),
    }
    if post.everyone || !post.named.is_empty() {
        return Ok(out);
    }
    // A plain post to a team goes to its lead.
    let Some(room) = &post.room else {
        return Ok(out);
    };
    match lead {
        Some(lead) if lead == post.author || askers.iter().any(|a| a == lead) => Ok(Vec::new()),
        Some(lead) => Ok(vec![lead.to_string()]),
        None if post.author.is_empty() => Ok(room.members.to_vec()),
        None => Err(NoLead),
    }
}

/// How conversation `session_key` speaks now, for a post into team
/// `team_id` ("" for a message): answering what addressed it and is not
/// answered yet, after its answer, or deliberately when nothing ever
/// addressed it.
pub(crate) fn said_in(state: &AppState, session_key: &str, team_id: &str) -> Said {
    if session_key.is_empty() {
        return Said::Deliberate;
    }
    let open = state.store.unanswered_in_seat(session_key).unwrap_or_else(|e| {
        tracing::warn!(error = %e, session = %session_key, "addressing: could not read what the conversation answers");
        Vec::new()
    });
    if open.is_empty() {
        return match state.store.seat_was_addressed(session_key) {
            Ok(true) => Said::Answer,
            _ => Said::Deliberate,
        };
    }
    if open.iter().all(|a| a.state == AddressingState::Collected) {
        return Said::Answer;
    }
    Said::Working {
        askers: open.iter().map(|a| a.asker_agent.clone()).collect(),
        in_room: !team_id.is_empty() && open.iter().any(|a| a.team_id == team_id),
    }
}

/// `post_id` reaches `agent_id`, who answers in `seat_session`: recorded
/// before its turn can end. `asker_session` hears the answer ("" = nobody:
/// the owner reads the team thread).
pub(crate) fn open(
    state: &AppState,
    post_id: &str,
    agent_id: &str,
    team_id: &str,
    seat_session: &str,
    asker_session: &str,
    asker_agent: &str,
) -> Result<(), String> {
    state
        .store
        .open_addressing(post_id, agent_id, team_id, seat_session, asker_session, asker_agent, now())
        .map_err(|e| format!("record who was asked: {e}"))
}

/// The post never reached `agent_id`: nobody waits for its answer.
pub(crate) fn withdraw(state: &AppState, post_id: &str, agent_id: &str) {
    if let Err(e) = state.store.withdraw_addressing(post_id, agent_id) {
        tracing::warn!(error = %e, post = %post_id, agent = %agent_id, "addressing: failed delivery not withdrawn");
    }
}

/// A turn in `seat_session` ended, saying `reply`. It is the seat's answer
/// once nothing the seat started is still out — no hand-off unanswered, no
/// helper running; until then it is progress and reaches no one. The answer
/// is given once: a later turn (a helper's result, a notification) finds
/// nothing left to answer.
pub(crate) fn turn_ended(state: &AppState, seat_session: &str, reply: &str, provenance: &[ProvenanceClass], depth: u8) {
    let out = state.store.open_asks(seat_session).unwrap_or(0);
    let helping = state.helpers.list(seat_session).iter().any(|h| h.running);
    if out > 0 || helping {
        return;
    }
    let prov = serde_json::to_string(provenance).unwrap_or_else(|_| "[]".to_string());
    let answered = match state.store.answer_seat(seat_session, reply, &prov, depth, now()) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, seat = %seat_session, "addressing: answer not recorded");
            return;
        }
    };
    let mut askers: Vec<&str> = Vec::new();
    for a in &answered {
        if !a.asker_session.is_empty() && !askers.contains(&a.asker_session.as_str()) {
            askers.push(&a.asker_session);
        }
    }
    for asker in askers {
        hear(state, asker);
    }
}

/// Wake kind of an answer reaching the conversation that asked.
pub(crate) const ANSWER: &str = "answer";

/// `asker_session` hears the answers it has been waiting for — once, all
/// together, when none of its asks is still unanswered. A seat that asked
/// has then collected: its next answer is final.
fn hear(state: &AppState, asker_session: &str) {
    let answers = match state.store.take_answers(asker_session) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, asker = %asker_session, "addressing: answers not taken");
            return;
        }
    };
    if answers.is_empty() {
        return;
    }
    if let Err(e) = state.store.collect_seat(asker_session) {
        tracing::warn!(error = %e, asker = %asker_session, "addressing: collection not recorded");
    }
    // An answer is delivered whole, capped the way a helper's result is:
    // past the cap it is saved through the one spill path, in the asker's
    // own results, and the notification carries its start and where it is.
    let spill_dir = state
        .harness
        .sessions()
        .resolve_session_id_by_key(asker_session)
        .map(|id| tools::result_shape::results_dir(&id))
        .unwrap_or_else(|_| tools::result_shape::results_dir(&asker_session.replace(':', "_")));
    let updates: Vec<crate::wake::Update> = answers.iter().map(|a| update(state, a, &spill_dir)).collect();
    crate::wake::enqueue_all(state, asker_session, &updates);
}

/// One answer as the asker's notification reads it.
fn update(state: &AppState, a: &Addressing, spill_dir: &std::path::Path) -> crate::wake::Update {
    let name = state
        .store
        .get_agent(&a.agent_id)
        .ok()
        .flatten()
        .map(|agent| agent.name)
        .unwrap_or_else(|| a.agent_id.clone());
    let team = if a.team_id.is_empty() {
        None
    } else {
        state.store.get_team(&a.team_id).ok().flatten().map(|t| t.name)
    };
    let who = match team {
        Some(team) => format!("{name}, in team \"{team}\""),
        None => name,
    };
    let mut provenance: Vec<ProvenanceClass> = serde_json::from_str(&a.provenance).unwrap_or_default();
    if !provenance.contains(&ProvenanceClass::Coworker) {
        provenance.push(ProvenanceClass::Coworker);
    }
    let payload = answer_text(&who, &provenance, &a.answer, spill_dir);
    crate::wake::Update { kind: ANSWER.to_string(), payload, provenance, handoff_depth: a.handoff_depth }
}

/// What the asker's model reads of one answer: whole, up to the cap a
/// helper's result has — past it, saved through the one spill path and
/// previewed — marked when it holds untrusted content. A notification is
/// the model's, never shown to a person.
fn answer_text(who: &str, provenance: &[ProvenanceClass], answer: &str, spill_dir: &std::path::Path) -> String {
    if answer.trim().is_empty() {
        return format!("{who} finished without an answer.");
    }
    let body = agent::harness::delegation::collect::spill_if_long(answer, spill_dir);
    match types::labels::provenance_mark(provenance) {
        Some(mark) => format!("{who}:\n{mark}\n{body}"),
        None => format!("{who}:\n{body}"),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::{NoLead, Post, Room, Said, answer_text, who_is_addressed};
    use types::provenance::ProvenanceClass;

    /// A teammate's 5,000-character answer reaches whoever asked intact; one
    /// past the helper-result cap is saved, and the asker reads its start
    /// and where the whole of it is — never a silent cut.
    #[test]
    fn an_answer_is_whole_or_saved() {
        let dir = std::env::temp_dir().join(format!("nebo-answer-{}", uuid::Uuid::new_v4()));
        let icp = "The ideal customer is a two-agent brokerage. ".repeat(120);
        assert!(icp.chars().count() > 5000);
        let read = answer_text("Business Brainstormer", &[], &icp, &dir);
        assert_eq!(read, format!("Business Brainstormer:\n{icp}"));
        let read = answer_text("Scout", &[ProvenanceClass::Coworker, ProvenanceClass::Web], "Rivals charge $40.", &dir);
        assert_eq!(read, "Scout:\n[Contains content from: web]\nRivals charge $40.");
        let huge = "x".repeat(agent::harness::delegation::collect::RESULT_CAP + 1);
        let read = answer_text("Business Brainstormer", &[], &huge, &dir);
        assert!(read.contains("Saved in full at:"), "{}", &read[..300.min(read.len())]);
        assert!(read.len() < huge.len());
        let saved = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read_to_string(saved).unwrap(), huge);
        assert_eq!(answer_text("Clerk", &[], "  ", &dir), "Clerk finished without an answer.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn members(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("m{i}")).collect()
    }

    fn team_post(author: &str, said: &Said, lead: Option<&str>, members: &[String], named: &[&str], everyone: bool) -> Result<Vec<String>, NoLead> {
        let named: Vec<String> = named.iter().map(|s| s.to_string()).collect();
        who_is_addressed(&Post { author, said, room: Some(Room { lead, members }), named: &named, everyone })
    }

    fn working(askers: &[&str], in_room: bool) -> Said {
        Said::Working { askers: askers.iter().map(|s| s.to_string()).collect(), in_room }
    }

    /// The owner's plain post addresses the lead alone; with no lead, every
    /// member. An employee's plain post from outside the team goes to the
    /// lead too; to a team with no lead it is refused.
    #[test]
    fn a_plain_post_goes_to_the_lead() {
        let team = members(4);
        let d = Said::Deliberate;
        assert_eq!(team_post("", &d, Some("m1"), &team, &[], false), Ok(vec!["m1".to_string()]));
        assert_eq!(team_post("", &d, None, &team, &[], false), Ok(team.clone()));
        assert_eq!(team_post("assistant", &d, Some("m2"), &team, &[], false), Ok(vec!["m2".to_string()]));
        assert_eq!(team_post("assistant", &d, None, &team, &[], false), Err(NoLead));
        assert_eq!(team_post("m1", &d, Some("m1"), &team, &[], false), Ok(vec![]), "the lead's own plain post asks no one");
    }

    /// @Name addresses the named members of the team, never the poster; a
    /// stranger is ignored. @everyone addresses every other member.
    #[test]
    fn names_and_everyone_address_whom_they_name() {
        let team = members(5);
        let d = Said::Deliberate;
        assert_eq!(team_post("", &d, Some("m1"), &team, &["m2", "m4"], false), Ok(vec!["m2".to_string(), "m4".to_string()]));
        assert_eq!(team_post("m3", &d, Some("m1"), &team, &["m3", "m5", "zed"], false), Ok(vec!["m5".to_string()]));
        assert_eq!(team_post("", &d, Some("m1"), &team, &[], true), Ok(team.clone()));
        assert_eq!(team_post("m1", &d, Some("m1"), &team, &[], true).unwrap().len(), 4);
    }

    /// The lead, answering the owner in its team, hands off by name or
    /// @everyone; a member answering there addresses no one, whatever it
    /// names — its answer goes to whoever asked, never back as a question.
    #[test]
    fn only_the_lead_hands_off_in_its_team() {
        let team = members(4);
        let owner_asked = working(&[""], true);
        assert_eq!(team_post("m1", &owner_asked, Some("m1"), &team, &[], true).unwrap().len(), 3);
        assert_eq!(team_post("m1", &owner_asked, Some("m1"), &team, &["m3"], false), Ok(vec!["m3".to_string()]));
        let lead_asked = working(&["m1"], true);
        assert_eq!(team_post("m2", &lead_asked, Some("m1"), &team, &["m1", "m3"], true), Ok(vec![]));
        assert_eq!(team_post("m2", &lead_asked, Some("m1"), &team, &[], false), Ok(vec![]));
    }

    /// Once answered — or once the answers it waited for are in — nothing a
    /// post names addresses anyone.
    #[test]
    fn an_answer_addresses_no_one() {
        let team = members(3);
        assert_eq!(team_post("m1", &Said::Answer, Some("m1"), &team, &["m2"], true), Ok(vec![]));
        assert_eq!(team_post("", &Said::Answer, Some("m1"), &team, &[], false), Ok(vec![]));
    }

    /// A message addresses its recipient — unless the sender is answering
    /// that recipient: a question back to whoever asked is the answer.
    #[test]
    fn a_message_addresses_its_recipient_but_never_the_asker() {
        let to = vec!["bk".to_string()];
        let msg = |author: &str, said: &Said| who_is_addressed(&Post { author, said, room: None, named: &to, everyone: false });
        assert_eq!(msg("ea", &Said::Deliberate), Ok(to.clone()));
        assert_eq!(msg("ea", &working(&["mk"], false)), Ok(to.clone()), "a hand-off to a third colleague");
        assert_eq!(msg("ea", &working(&["bk"], false)), Ok(vec![]));
        assert_eq!(msg("ea", &Said::Answer), Ok(vec![]));
    }

    /// An employee answering a colleague that posts into a team it is not
    /// answering in is outside that room: a plain post goes to the lead,
    /// unless the lead is the one it answers.
    #[test]
    fn a_post_from_outside_the_room_while_answering() {
        let team = members(3);
        assert_eq!(team_post("bk", &working(&["ea"], false), Some("m1"), &team, &[], false), Ok(vec!["m1".to_string()]));
        assert_eq!(team_post("bk", &working(&["m1"], false), Some("m1"), &team, &[], false), Ok(vec![]));
    }

    /// The live loop (2026-09-29), played out by the rule alone: the owner
    /// tells the lead to stop everybody; the lead's @everyone asks the
    /// members; every member answers naming the lead and @everyone; the lead
    /// hears them together and its answer names @everyone again. Each is
    /// addressed exactly once, and the thread goes quiet.
    #[test]
    fn the_live_loop_ends_after_one_round() {
        let team = members(4);
        let lead = Some("m1");
        // Who is working, on whose ask: (employee, askers). The owner's post:
        let mut asked = team_post("", &Said::Deliberate, lead, &team, &[], false).unwrap();
        assert_eq!(asked, vec!["m1".to_string()]);
        let mut addressed: Vec<String> = asked.clone();
        // The lead answers the owner with @everyone: a hand-off.
        asked = team_post("m1", &working(&[""], true), lead, &team, &["m1"], true).unwrap();
        assert_eq!(asked.len(), 3);
        addressed.extend(asked.iter().cloned());
        // Each member answers naming the lead and everyone: nobody.
        for m in &asked {
            assert!(team_post(m, &working(&["m1"], true), lead, &team, &["m1"], true).unwrap().is_empty());
        }
        // The lead has collected: its answer names everyone again: nobody.
        assert!(team_post("m1", &Said::Answer, lead, &team, &[], true).unwrap().is_empty());
        let mut once = addressed.clone();
        once.sort();
        once.dedup();
        assert_eq!(once.len(), addressed.len(), "every employee was addressed once: {addressed:?}");
    }
}
