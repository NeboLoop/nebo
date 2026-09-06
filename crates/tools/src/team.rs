//! Teams — the ONE creation core and the helpers every door shares.
//!
//! A team is a LOCAL object on this Nebo: a name, a mission, the employees
//! in it, and its own local thread (session key `team:<id>`). Creating one
//! never touches the hub; posting into one appends to the local thread and
//! fans out to the members through the coworker rail. If this Nebo is in a
//! NeboAI hub loop, the team is ALSO mirrored to a hub channel
//! (`hub_channel_id`) and posts are forwarded there — additive, never
//! required, never an error when the hub is away.

use std::sync::Arc;

use comm::CommPlugin;
use db::{Store, Team};

/// WS event announcing a new team; the sidebar refreshes its list on it.
pub const TEAM_CREATED_EVENT: &str = "team_created";
/// WS event for every post in a team's thread (the open team view is
/// event-driven, never polling).
pub const TEAM_MESSAGE_EVENT: &str = "team_message";
/// WS event after a team's name, mission, or members change; carries the team.
pub const TEAM_UPDATED_EVENT: &str = "team_updated";
/// WS event when a member starts working on a post ("X is working").
pub const TEAM_ACTIVITY_EVENT: &str = "team_activity";

/// The one create call, as the model should write it.
pub const CREATE_USAGE: &str = "team(action: \"create\", name: \"Operations\", mission: \"Keep the office running\", agents: [\"Chief of Staff\", \"Executive Assistant\"])";

/// Wording for the empty state: teams are local, here is how to make one.
/// Also the tail of the loop tool's no-loop answer — a Nebo outside every
/// hub loop still has teams.
pub fn no_teams_hint() -> String {
    format!(
        "No teams yet on this Nebo; teams work locally and need no hub. Create one with {CREATE_USAGE}."
    )
}

/// Create a team. `member_agent_ids` are LOCAL agent ids — resolve names
/// before calling; `organizer_agent_id` is the creating employee (empty when
/// the owner created it) and is always a member.
///
/// Policies enforced HERE, for every door:
/// - A team is a collaboration: at least two employees — the organizer plus
///   the coworkers it names. No solo teams.
/// - Names are unique: a repeated name is an error, never an overwrite (the
///   team is addressable by name).
///
/// Hub mirroring is optional: with a connected comm plugin that reports at
/// least one hub loop, a hub channel is created and recorded as
/// `hub_channel_id`; any hub failure leaves the team local and is only
/// logged.
pub async fn create(
    comm: Option<&Arc<dyn CommPlugin>>,
    store: &Store,
    name: &str,
    mission: &str,
    member_agent_ids: &[String],
    organizer_agent_id: &str,
) -> Result<Team, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("team name required".to_string());
    }
    let mission = mission.trim();

    let mut members: Vec<String> = Vec::new();
    if !organizer_agent_id.is_empty() {
        members.push(organizer_agent_id.to_string());
    }
    for id in member_agent_ids {
        if !id.is_empty() && !members.iter().any(|m| m == id) {
            members.push(id.clone());
        }
    }
    if members.len() < 2 {
        return Err(format!(
            "A team needs at least two employees — the organizer plus the coworkers it \
             names. Name the coworkers and create again: {CREATE_USAGE}"
        ));
    }

    if let Some(existing) = store
        .get_team_by_name(name)
        .map_err(|e| format!("check team name: {e}"))?
    {
        return Err(format!(
            "A team named \"{}\" already exists (id: {}). Post into it with \
             team(action: \"send\", team: \"{}\", text: \"...\"), or create a team with a \
             new, distinct name.",
            existing.name, existing.id, existing.name
        ));
    }

    let hub_channel_id = mirror_to_hub(comm, name, mission).await;

    let id = uuid::Uuid::new_v4().to_string();
    let team = store
        .create_team(&id, name, mission, &members, organizer_agent_id, hub_channel_id.as_deref())
        .map_err(|e| format!("create team: {e}"))?;
    // The team's own thread exists from day one — an empty team still has a
    // place to read.
    store
        .ensure_team_thread(&team.id, &team.name)
        .map_err(|e| format!("open team thread: {e}"))?;
    Ok(team)
}

/// Change a team's name, mission, members, or lead — the ONE rule set both
/// doors (the app's edit picker, the tool) go through. `None` keeps a field.
/// Rules: a team never drops below two employees; the lead (organizer) must
/// be a member or empty (= the owner leads); remove the lead without naming
/// a new one and the team becomes owner-led.
pub fn update(
    store: &Store,
    team_id: &str,
    name: Option<&str>,
    mission: Option<&str>,
    member_agent_ids: Option<&[String]>,
    organizer_agent_id: Option<&str>,
) -> Result<Team, String> {
    let current = store
        .get_team(team_id)
        .map_err(|e| format!("load team: {e}"))?
        .ok_or_else(|| format!("No team with id {team_id}"))?;

    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&current.name);
    if !name.eq_ignore_ascii_case(&current.name) {
        if let Some(existing) = store
            .get_team_by_name(name)
            .map_err(|e| format!("check team name: {e}"))?
        {
            if existing.id != current.id {
                return Err(format!("A team named \"{}\" already exists.", existing.name));
            }
        }
    }
    let mission = mission.map(str::trim).unwrap_or(&current.mission);

    let mut members: Vec<String> = Vec::new();
    for id in member_agent_ids.unwrap_or(&current.member_agent_ids) {
        if !id.is_empty() && !members.iter().any(|m| m == id) {
            members.push(id.clone());
        }
    }
    if members.len() < 2 {
        return Err("A team needs at least two employees. Keep two or remove the team.".to_string());
    }
    let organizer = match organizer_agent_id {
        Some("") => String::new(),
        Some(id) if members.iter().any(|m| m == id) => id.to_string(),
        Some(id) => {
            return Err(format!(
                "The lead must be on the team; \"{id}\" is not a member. Add them first or pick a member."
            ))
        }
        None if members.iter().any(|m| m == &current.organizer_agent_id) => {
            current.organizer_agent_id.clone()
        }
        None => String::new(),
    };

    store
        .update_team(&current.id, name, mission, &members, &organizer)
        .map_err(|e| format!("update team: {e}"))?
        .ok_or_else(|| "team vanished during update".to_string())
}

/// Best-effort hub mirror: a channel in the bot's hub loop, only when the
/// plugin is connected AND reports a loop. Every failure is None + a log
/// line — the hub is never on the team's critical path.
async fn mirror_to_hub(
    comm: Option<&Arc<dyn CommPlugin>>,
    name: &str,
    mission: &str,
) -> Option<String> {
    let comm = comm?;
    if !comm.is_connected() {
        return None;
    }
    match comm.list_loops().await {
        Ok(loops) if !loops.is_empty() => {}
        Ok(_) => return None,
        Err(e) => {
            tracing::debug!(error = %e, "team: hub loops unavailable; team stays local");
            return None;
        }
    }
    match comm
        .ensure_channel(name, (!mission.is_empty()).then_some(mission))
        .await
    {
        Ok(channel_id) => Some(channel_id),
        Err(e) => {
            tracing::warn!(error = %e, team = name, "team: hub mirror failed; team stays local");
            None
        }
    }
}

/// Resolve a team reference — id first, then (case-insensitive) name.
pub fn resolve_team(store: &Store, label: &str) -> Result<Team, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("team required: the team's name or id".to_string());
    }
    if let Some(team) = store.get_team(label).map_err(|e| format!("load team: {e}"))? {
        return Ok(team);
    }
    if let Some(team) = store
        .get_team_by_name(label)
        .map_err(|e| format!("load team: {e}"))?
    {
        return Ok(team);
    }
    let known: Vec<String> = store
        .list_teams()
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect();
    Err(if known.is_empty() {
        format!("No team named \"{label}\". {}", no_teams_hint())
    } else {
        format!(
            "No team named \"{label}\". Teams on this Nebo: {}. Use one of those names, or team(action: \"list\").",
            known.join(", ")
        )
    })
}

/// Resolve an employee label (local id, exact name, or handle) against the
/// installed roster. Returns the local agent id.
pub fn resolve_agent(store: &Store, label: &str) -> Option<db::models::Agent> {
    let label = label.trim().trim_start_matches('@');
    if label.is_empty() {
        return None;
    }
    if let Ok(Some(a)) = store.get_agent(label) {
        return Some(a);
    }
    if let Ok(Some(a)) = store.get_agent_by_name(label) {
        return Some(a);
    }
    let wanted = comm::handle::slugify(label);
    store
        .list_agents(500, 0)
        .unwrap_or_default()
        .into_iter()
        .find(|a| {
            comm::handle::slugify(&a.name) == wanted
                || a.handle.as_deref().map(comm::handle::slugify) == Some(wanted.clone())
        })
}

/// The team's members as `(id, name)`, in team order; a departed employee
/// keeps its id as the name rather than vanishing from the roster.
pub fn member_roster(store: &Store, team: &Team) -> Vec<(String, String)> {
    team.member_agent_ids
        .iter()
        .map(|id| {
            let name = store
                .get_agent(id)
                .ok()
                .flatten()
                .map(|a| a.name)
                .unwrap_or_else(|| id.clone());
            (id.clone(), name)
        })
        .collect()
}

/// Inside a team, an exact member name after '@' normalizes to that member's
/// mention token. Employees naturally write "@Executive Assistant"; the
/// owner's composer writes tokens. One token grammar for dispatch.
pub fn normalize_mentions(text: &str, roster: &[(String, String)]) -> String {
    let mut t = text.to_string();
    for (id, name) in roster {
        if name.is_empty() {
            continue;
        }
        let needle = format!("@{}", name).to_lowercase();
        loop {
            let lower = t.to_lowercase();
            let Some(pos) = lower.find(&needle) else { break };
            t = format!("{}<@{}>{}", &t[..pos], id, &t[pos + needle.len()..]);
        }
    }
    t
}

/// The team members addressed by `<@id>` tokens in a (normalized) post, in
/// order of first appearance, deduplicated.
pub fn mentioned_members(text: &str, member_ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in comm::handle::parse_mention_tokens(text) {
        if member_ids.iter().any(|m| *m == id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-team-tool-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// A team is created with NO comm plugin at all: local row, local
    /// thread, no hub channel.
    #[tokio::test]
    async fn create_without_a_hub() {
        let s = store();
        let team = create(None, &s, "Operations", "Run the office", &["ea".to_string()], "chief")
            .await
            .unwrap();
        assert_eq!(team.member_agent_ids, vec!["chief".to_string(), "ea".to_string()]);
        assert_eq!(team.organizer_agent_id, "chief");
        assert_eq!(team.hub_channel_id, None);
        assert!(s.get_session_by_name(&db::team_thread_key(&team.id)).unwrap().is_some());
        assert_eq!(s.list_teams().unwrap().len(), 1);
    }

    /// A connected comm plugin that reports no hub loops (the loopback
    /// plugin has none) changes nothing: the team is still local.
    #[tokio::test]
    async fn create_with_a_plugin_in_no_loop() {
        let s = store();
        let comm: Arc<dyn CommPlugin> = Arc::new(comm::LoopbackPlugin::new());
        comm.connect(std::collections::HashMap::new()).await.unwrap();
        assert!(comm.is_connected());
        let team = create(Some(&comm), &s, "Sales", "", &["ea".to_string()], "chief")
            .await
            .unwrap();
        assert_eq!(team.hub_channel_id, None);
        assert_eq!(team.name, "Sales");
    }

    /// The two policies: no solo teams, no repeated names.
    #[tokio::test]
    async fn create_refuses_solo_and_repeated_names() {
        let s = store();
        let err = create(None, &s, "Solo", "", &[], "chief").await.unwrap_err();
        assert!(err.contains("at least two employees"), "{err}");
        let err = create(None, &s, "Solo", "", &["chief".to_string()], "chief")
            .await
            .unwrap_err();
        assert!(err.contains("at least two employees"), "{err}");

        create(None, &s, "Ops", "", &["ea".to_string()], "chief").await.unwrap();
        let err = create(None, &s, "ops", "", &["ea".to_string()], "chief")
            .await
            .unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(s.list_teams().unwrap().len(), 1);
    }

    #[test]
    fn mentions_normalize_and_resolve_against_the_roster() {
        let roster = vec![
            ("chief".to_string(), "Chief of Staff".to_string()),
            ("ea".to_string(), "Executive Assistant".to_string()),
        ];
        let text = normalize_mentions("@executive assistant book it; <@chief> fyi", &roster);
        assert_eq!(text, "<@ea> book it; <@chief> fyi");
        let ids = mentioned_members(&text, &["chief".to_string(), "ea".to_string()]);
        assert_eq!(ids, vec!["ea".to_string(), "chief".to_string()]);
        assert!(mentioned_members("nobody here", &["chief".to_string()]).is_empty());
    }

    #[test]
    fn team_resolves_by_id_or_name() {
        let s = store();
        let team = s.create_team("t-1", "Ops", "", &["a".into(), "b".into()], "a", None).unwrap();
        assert_eq!(resolve_team(&s, "t-1").unwrap().id, team.id);
        assert_eq!(resolve_team(&s, "OPS").unwrap().id, team.id);
        let err = resolve_team(&s, "Nope").unwrap_err();
        assert!(err.contains("Teams on this Nebo: Ops"), "{err}");
    }
}
