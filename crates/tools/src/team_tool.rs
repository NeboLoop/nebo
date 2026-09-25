//! The team tools — teams of AI employees on THIS Nebo: create, update,
//! list, members and messages, one purpose each over one [`Teams`] core.
//! Posting into a team is `send_message` to the team, through the same core.
//! Everything works with no hub; a hub loop only adds a mirror.

use std::sync::Arc;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::team;
use comm::CommPlugin;

/// The primary employee's local agent id — a call from the main companion
/// chat carries no agent segment in its session key, and the companion IS
/// the primary employee.
const PRIMARY_AGENT_ID: &str = "assistant";

/// The teams on this Nebo: the one core every team tool and the team route
/// of `send_message` share.
pub struct Teams {
    store: Option<Arc<db::Store>>,
    /// Hub plugin, for the optional mirror only.
    comm: Option<Arc<dyn CommPlugin>>,
    /// ClientHub broadcast — the sidebar learns about a new team live.
    broadcast: Option<crate::web_tool::Broadcaster>,
    /// The coworker rail posts into teams (thread + fan-out + hub mirror).
    rail: crate::coworker::CoworkerRailCell,
}

impl Teams {
    pub fn new(
        store: Option<Arc<db::Store>>,
        comm: Option<Arc<dyn CommPlugin>>,
        broadcast: Option<crate::web_tool::Broadcaster>,
        rail: crate::coworker::CoworkerRailCell,
    ) -> Self {
        Self {
            store,
            comm,
            broadcast,
            rail,
        }
    }

    fn store(&self) -> Result<&Arc<db::Store>, ToolResult> {
        self.store.as_ref().ok_or_else(|| {
            ToolResult::error(
                "Teams are not available on this install (no local store). Ask the owner; nothing here can create one.",
            )
        })
    }

    /// The calling employee's local id (the primary employee for the main
    /// companion chat).
    fn caller_agent_id(store: &db::Store, ctx: &ToolContext) -> String {
        let own = types::keyparser::extract_agent_id(&ctx.session_key);
        if !own.is_empty() {
            return own;
        }
        match store.get_agent(PRIMARY_AGENT_ID) {
            Ok(Some(_)) => PRIMARY_AGENT_ID.to_string(),
            _ => String::new(),
        }
    }

    /// `members` / `mention`: a string (comma-separated) or an array of
    /// employee names, handles, or ids.
    fn labels(value: &serde_json::Value) -> Vec<String> {
        match value {
            serde_json::Value::String(s) => s
                .split(',')
                .map(|p| p.trim().trim_start_matches('@').to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|p| p.trim().trim_start_matches('@').to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn roster_line(store: &db::Store, team: &db::Team) -> String {
        team::member_roster(store, team)
            .into_iter()
            .map(|(id, name)| {
                if id == team.organizer_agent_id {
                    format!("{name} (lead, id: {id})")
                } else {
                    format!("{name} (id: {id})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// One line per team for list answers (shared with the NeboAI loop tools).
    pub fn describe(store: &db::Store, team: &db::Team) -> String {
        let mut line = format!("- {} (id: {})", team.name, team.id);
        if !team.mission.is_empty() {
            line.push_str(&format!(" — mission: {}", team.mission));
        }
        line.push_str(&format!("; members: {}", Self::roster_line(store, team)));
        if team.hub_channel_id.is_some() {
            line.push_str("; mirrored to a NeboAI hub channel");
        }
        line
    }

    pub async fn create(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let name = input["name"].as_str().unwrap_or("").trim();
        let mission = input["mission"].as_str().unwrap_or("");

        // The primary employee is the platform, not a teammate: when it
        // creates a team it does so on the owner's behalf — the owner is the
        // organizer and the primary stays outside the roster. Every other
        // employee joins the team it creates, as organizer.
        let caller = Self::caller_agent_id(store, ctx);
        let mut organizer = if caller == PRIMARY_AGENT_ID { String::new() } else { caller };
        let mut member_ids: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for label in Self::labels(&input["members"]) {
            match team::resolve_agent(store, &label) {
                Some(a) => {
                    if !member_ids.contains(&a.id) {
                        member_ids.push(a.id);
                    }
                }
                None => unknown.push(label),
            }
        }
        // A named lead: the employee that answers the owner and hands work to
        // the others by mention. Named by the primary on the owner's behalf,
        // or by any employee that would otherwise lead its own team.
        if let Some(label) = input["lead"].as_str().map(str::trim).filter(|l| !l.is_empty()) {
            match team::resolve_agent(store, label) {
                Some(a) => {
                    if !member_ids.contains(&a.id) {
                        member_ids.push(a.id.clone());
                    }
                    organizer = a.id;
                }
                None => unknown.push(label.to_string()),
            }
        }
        if !unknown.is_empty() {
            let roster: Vec<String> = store
                .list_agents(500, 0)
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.name)
                .collect();
            return ToolResult::error(format!(
                "No employee named {} is installed, so the team was NOT created. Installed employees: {}. \
                 Use those names and create again: {}",
                unknown
                    .iter()
                    .map(|u| format!("\"{u}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                if roster.is_empty() { "none".to_string() } else { roster.join(", ") },
                team::CREATE_USAGE
            ));
        }

        // The tool names employees on THIS machine. A member on another
        // computer is picked from the hub roster, which only the app and the
        // phone can show, so that door adds them and this one keeps managing
        // the local side.
        let member_list: Vec<db::TeamMember> =
            member_ids.iter().map(db::TeamMember::local).collect();
        match team::create(self.comm.as_ref(), store, name, mission, &member_list, &organizer).await {
            Ok(t) => {
                if let Some(bc) = self.broadcast.as_ref() {
                    bc(team::TEAM_CREATED_EVENT, serde_json::json!({ "team": t }));
                }
                let members = Self::roster_line(store, &t);
                let mirror = if t.hub_channel_id.is_some() {
                    " It is also mirrored to a NeboAI hub channel."
                } else {
                    ""
                };
                ToolResult::ok(format!(
                    "Team \"{}\" exists (id: {}). Members: {}.{} It works locally on this Nebo. \
                     Start the work by posting the first ask: send_message(to: \"{}\", \
                     message: \"...\") — the lead answers and hands steps to teammates by mention; add \
                     mention: [\"Member Name\"] to ask specific members, or write @everyone in the \
                     message to ask the whole team.{}",
                    t.name, t.id, members, mirror, t.name,
                    if t.organizer_agent_id.is_empty() {
                        " This team has no lead yet: an owner post reaches every member, and a post \
                         from an employee must name who acts until one is set with \
                         update_team(team: \"...\", lead: \"Employee Name\")."
                    } else {
                        ""
                    }
                ))
                .with_payload(serde_json::json!({
                    "kind": "team_created",
                    "team": t,
                }))
            }
            Err(e) => ToolResult::error(format!("Failed to create team \"{name}\": {e}")),
        }
    }

    pub fn list(&self) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let teams = match store.list_teams() {
            Ok(t) => t,
            Err(e) => return ToolResult::error(format!("Failed to list teams: {e}")),
        };
        if teams.is_empty() {
            return ToolResult::ok(team::no_teams_hint());
        }
        let lines: Vec<String> = teams.iter().map(|t| Self::describe(store, t)).collect();
        ToolResult::ok(format!(
            "{} team(s) on this Nebo\n{}\nPost with send_message(to: \"<team name>\", message: \"...\").",
            teams.len(),
            lines.join("\n")
        ))
    }

    /// Post `text` into the team `label` names (name or id). `mention`
    /// (names, a string or an array) asks those members to act.
    pub async fn post(
        &self,
        ctx: &ToolContext,
        label: &str,
        text: &str,
        mention: &serde_json::Value,
    ) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let text = text.trim();
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };

        // `mention`: members asked to act, resolved against the team roster.
        let mut asked: Vec<String> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        for m in Self::labels(mention) {
            match team::resolve_agent(store, &m) {
                Some(a) if t.members.iter().any(|m| m.agent_id == a.id) => {
                    if !asked.contains(&a.id) {
                        asked.push(a.id);
                    }
                }
                _ => unresolved.push(m),
            }
        }
        if !unresolved.is_empty() {
            return ToolResult::error(format!(
                "{} not in team \"{}\" — members: {}. The message was NOT sent; mention members only.",
                unresolved
                    .iter()
                    .map(|u| format!("\"{u}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                t.name,
                Self::roster_line(store, &t)
            ));
        }

        let rail = self.rail.read().unwrap().clone();
        let Some(rail) = rail else {
            return ToolResult::error(
                "Team posting is not available in this environment (no coworker rail wired).",
            );
        };
        let post = crate::coworker::TeamPost {
            attachments: vec![],
            team_id: t.id.clone(),
            from_agent_id: Self::caller_agent_id(store, ctx),
            text: text.to_string(),
            mention: asked,
            handoff_depth: ctx.handoff_depth,
            provenance: ctx.run_taint.clone(),
            is_reply: false,
            reply_to: Some(ctx.session_key.clone()),
        };
        match rail.post_team(post).await {
            Ok(receipt) => {
                let asked = if receipt.asked.is_empty() {
                    "Every member read it as context; nobody was asked to act (mention members to ask them).".to_string()
                } else {
                    format!(
                        "Asked to act: {}. They answer in the team, and each reply comes to you as a notification.",
                        receipt.asked.join(", ")
                    )
                };
                ToolResult::ok(format!(
                    "Posted to team \"{}\". {}",
                    receipt.team_name, asked
                ))
                .with_payload(serde_json::json!({
                    "kind": "team_post",
                    "teamId": receipt.team_id,
                    "team": receipt.team_name,
                    "messageId": receipt.message_id,
                    "asked": receipt.asked,
                    "text": text,
                }))
            }
            Err(e) => ToolResult::error(e),
        }
    }

    pub fn messages(&self, input: &serde_json::Value) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let label = input["team"].as_str().unwrap_or("");
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        let limit = input["limit"].as_u64().unwrap_or(50) as usize;
        match store.list_team_messages(&t.id, limit) {
            Ok(msgs) if msgs.is_empty() => ToolResult::ok(format!(
                "No messages in team \"{}\" yet. Post the first one with send_message(to: \"{}\", message: \"...\").",
                t.name, t.name
            )),
            Ok(msgs) => {
                let lines: Vec<String> = msgs
                    .iter()
                    .map(|m| {
                        let who = if m.from.is_empty() { m.role.as_str() } else { m.from.as_str() };
                        format!("[{}] {}: {}", m.created_at, who, m.content)
                    })
                    .collect();
                ToolResult::ok(format!(
                    "{} most recent message(s) in team \"{}\" (limit {})\n{}",
                    msgs.len(),
                    t.name,
                    limit,
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to read team messages: {e}")),
        }
    }

    /// Change a team's name, mission, members or lead — through the ONE rule
    /// set the app's edit dialog uses (`team::update`). Fields left out keep
    /// their value; `lead: "owner"` (or "") makes the team owner-led.
    pub fn update(&self, input: &serde_json::Value) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let label = input["team"].as_str().unwrap_or("");
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        let mut unknown: Vec<String> = Vec::new();
        let members: Option<Vec<String>> = if input["members"].is_array() {
            let mut ids: Vec<String> = Vec::new();
            for l in Self::labels(&input["members"]) {
                match team::resolve_agent(store, &l) {
                    Some(a) => {
                        if !ids.contains(&a.id) {
                            ids.push(a.id);
                        }
                    }
                    None => unknown.push(l),
                }
            }
            Some(ids)
        } else {
            None
        };
        let lead: Option<String> = match input["lead"].as_str().map(str::trim) {
            None => None,
            Some("") | Some("owner") | Some("none") => Some(String::new()),
            Some(l) => match team::resolve_agent(store, l) {
                Some(a) => Some(a.id),
                None => {
                    unknown.push(l.to_string());
                    None
                }
            },
        };
        if !unknown.is_empty() {
            return ToolResult::error(format!(
                "No employee named {} is installed, so the team was NOT changed.",
                unknown.iter().map(|u| format!("\"{u}\"")).collect::<Vec<_>>().join(", ")
            ));
        }
        // A lead named without a member list is added to the current members
        // rather than refused: naming a lead is the ask, not a roster edit.
        // Members named here are local ids; a remote member already on the
        // team is carried through untouched rather than resolved again.
        let members: Option<Vec<db::TeamMember>> = match (&members, &lead) {
            (None, Some(id)) if !id.is_empty() && !t.members.iter().any(|m| m.agent_id == *id) => {
                let mut m = t.members.clone();
                m.push(db::TeamMember::local(id));
                Some(m)
            }
            (Some(ids), _) => Some(
                ids.iter()
                    .map(|id| {
                        t.members
                            .iter()
                            .find(|m| m.agent_id == *id)
                            .cloned()
                            .unwrap_or_else(|| db::TeamMember::local(id))
                    })
                    .collect(),
            ),
            _ => None,
        };
        match team::update(
            store,
            &t.id,
            input["name"].as_str(),
            input["mission"].as_str(),
            members.as_deref(),
            lead.as_deref(),
        ) {
            Ok(updated) => {
                if let Some(bc) = self.broadcast.as_ref() {
                    bc(team::TEAM_UPDATED_EVENT, serde_json::json!({ "team": updated }));
                }
                ToolResult::ok(format!(
                    "Team \"{}\" updated. Members: {}. {}",
                    updated.name,
                    Self::roster_line(store, &updated),
                    if updated.organizer_agent_id.is_empty() {
                        "It is owner-led: every member answers an owner post.".to_string()
                    } else {
                        "The lead answers the owner and hands steps to teammates by mention.".to_string()
                    }
                ))
                .with_payload(serde_json::json!({ "kind": "team_updated", "team": updated }))
            }
            Err(e) => ToolResult::error(format!("Failed to update team \"{}\": {e}", t.name)),
        }
    }

    pub fn members(&self, input: &serde_json::Value) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let label = input["team"].as_str().unwrap_or("");
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        ToolResult::ok(format!(
            "{} member(s) in team \"{}\": {}",
            t.members.len(),
            t.name,
            Self::roster_line(store, &t)
        ))
    }
}

/// One tool of the team family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Create,
    Update,
    List,
    Members,
    Messages,
}

const KINDS: &[Kind] = &[Kind::Create, Kind::Update, Kind::List, Kind::Members, Kind::Messages];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Create => "create_team",
            Kind::Update => "update_team",
            Kind::List => "list_teams",
            Kind::Members => "team_members",
            Kind::Messages => "team_messages",
        }
    }

    fn search_hint(self) -> &'static str {
        match self {
            Kind::Create => "make a team of employees",
            Kind::Update => "change a team's lead members mission",
            Kind::List => "list the teams of employees",
            Kind::Members => "who is on a team",
            Kind::Messages => "read a team's conversation",
        }
    }

    fn description(self) -> String {
        match self {
            Kind::Create => "Creates a team: employees on this Nebo who share a mission and one conversation. It needs no hub.\n\
                - `members`: at least one employee besides you (you join the team you create).\n\
                - `lead` answers the owner and hands steps to teammates by mention; without one the owner leads and every member answers.\n\
                - Post the first ask with send_message to the team's name."
                .to_string(),
            Kind::Update => "Changes a team's name, mission, members or lead. Fields left out keep their value; `members` is the full new list; lead: \"owner\" makes the team owner-led."
                .to_string(),
            Kind::List => "Lists the teams on this Nebo with their missions, leads and members.".to_string(),
            Kind::Members => "Lists who is on a team, and which member leads it.".to_string(),
            Kind::Messages => "Reads a team's conversation, most recent last.".to_string(),
        }
    }

    fn schema(self) -> serde_json::Value {
        let team = serde_json::json!({ "type": "string", "description": "The team's name or id." });
        let names = |what: &str| serde_json::json!({ "type": "array", "items": { "type": "string" }, "description": what });
        let lead = serde_json::json!({ "type": "string", "description": "The employee who leads the team." });
        match self {
            Kind::Create => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The team's name." },
                    "members": names("Employee names to bring into the team: at least one besides you."),
                    "mission": { "type": "string", "description": "What the team exists to accomplish." },
                    "lead": lead
                },
                "required": ["name", "members"]
            }),
            Kind::Update => serde_json::json!({
                "type": "object",
                "properties": {
                    "team": team,
                    "name": { "type": "string", "description": "A new name." },
                    "mission": { "type": "string", "description": "A new mission." },
                    "members": names("The full new member list, by employee name."),
                    "lead": { "type": "string", "description": "The employee who leads the team, or \"owner\" to make it owner-led." }
                },
                "required": ["team"]
            }),
            Kind::List => serde_json::json!({ "type": "object", "properties": {} }),
            Kind::Members => serde_json::json!({
                "type": "object",
                "properties": { "team": team },
                "required": ["team"]
            }),
            Kind::Messages => serde_json::json!({
                "type": "object",
                "properties": {
                    "team": team,
                    "limit": { "type": "integer", "description": "How many recent messages (default 50)." }
                },
                "required": ["team"]
            }),
        }
    }

    fn read_only(self) -> bool {
        matches!(self, Kind::List | Kind::Members | Kind::Messages)
    }

    fn labels(self, input: &serde_json::Value) -> (String, String) {
        let named = |key: &str| input[key].as_str().unwrap_or("").trim().to_string();
        match self {
            Kind::Create => (format!("creating the {} team", named("name")), format!("Created the {} team", named("name"))),
            Kind::Update => (format!("updating the {} team", named("team")), format!("Updated the {} team", named("team"))),
            Kind::List => ("checking the teams".into(), "Checked the teams".into()),
            Kind::Members => (format!("checking who is on {}", named("team")), format!("Checked who is on {}", named("team"))),
            Kind::Messages => (format!("reading the {} team", named("team")), format!("Read the {} team", named("team"))),
        }
    }
}

/// One team tool over the shared [`Teams`] core.
pub struct TeamTool {
    teams: Arc<Teams>,
    kind: Kind,
}

/// Every team tool, sharing one core.
pub fn tools(teams: Arc<Teams>) -> Vec<TeamTool> {
    KINDS.iter().map(|&kind| TeamTool { teams: teams.clone(), kind }).collect()
}

impl DynTool for TeamTool {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn description(&self) -> String {
        self.kind.description()
    }

    fn schema(&self) -> serde_json::Value {
        self.kind.schema()
    }

    fn search_hint(&self) -> &str {
        self.kind.search_hint()
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        self.kind.read_only()
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).0
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).1
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.kind {
                Kind::Create => self.teams.create(&input, ctx).await,
                Kind::Update => self.teams.update(&input),
                Kind::List => self.teams.list(),
                Kind::Members => self.teams.members(&input),
                Kind::Messages => self.teams.messages(&input),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Arc<db::Store> {
        let path = std::env::temp_dir().join(format!("nebo-team-tool-test-{}.db", uuid::Uuid::new_v4()));
        Arc::new(db::Store::new(&path.to_string_lossy()).expect("store"))
    }

    fn install(store: &db::Store, id: &str, name: &str) {
        store
            .create_agent(id, None, name, &format!("{name} description"), "# agent", "", None, None)
            .expect("create agent");
    }

    struct Rig(Vec<TeamTool>);

    impl Rig {
        fn new(s: &Arc<db::Store>, comm: Option<Arc<dyn CommPlugin>>) -> Self {
            Rig(tools(Arc::new(Teams::new(Some(s.clone()), comm, None, crate::coworker::new_rail_cell()))))
        }

        async fn call(&self, ctx: &ToolContext, name: &str, input: serde_json::Value) -> ToolResult {
            let tool = self.0.iter().find(|t| t.name() == name).expect("a team tool");
            tool.execute_dyn(ctx, input).await
        }
    }

    fn as_employee(id: &str) -> ToolContext {
        ToolContext { session_key: format!("agent:{id}:web"), ..ToolContext::default() }
    }

    /// The create path with NO comm plugin: the team exists, with the caller
    /// as organizer plus the named coworkers, and no hub channel.
    #[tokio::test]
    async fn create_works_with_no_comm_plugin() {
        let s = store();
        install(&s, "chief", "Chief of Staff");
        install(&s, "ea", "Executive Assistant");
        let rig = Rig::new(&s, None);
        let ctx = as_employee("chief");
        let res = rig
            .call(&ctx, "create_team", json!({"name": "Operations", "mission": "Keep the office running", "members": ["Executive Assistant"]}))
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("Team \"Operations\" exists"), "{}", res.content);
        assert!(res.content.contains("send_message(to: \"Operations\""), "{}", res.content);
        let teams = s.list_teams().unwrap();
        assert_eq!(teams.len(), 1);
        let ids: Vec<&str> = teams[0].members.iter().map(|m| m.agent_id.as_str()).collect();
        assert_eq!(ids, vec!["chief", "ea"]);
        assert_eq!(teams[0].hub_channel_id, None);

        let listed = rig.call(&ctx, "list_teams", json!({})).await;
        assert!(listed.content.contains("Operations"), "{}", listed.content);
        assert!(listed.content.contains("Chief of Staff (lead"), "{}", listed.content);
        let members = rig.call(&ctx, "team_members", json!({"team": "Operations"})).await;
        assert!(members.content.contains("2 member(s)") && members.content.contains("Executive Assistant"), "{}", members.content);
        let messages = rig.call(&ctx, "team_messages", json!({"team": "Operations"})).await;
        assert!(messages.content.contains("No messages in team \"Operations\" yet"), "{}", messages.content);
    }

    /// The primary creates a team on the owner's behalf and names the lead;
    /// later the lead can be changed through update_team, by name.
    #[tokio::test]
    async fn primary_names_a_lead_and_can_change_it() {
        let s = store();
        install(&s, "ea", "Executive Assistant");
        install(&s, "bk", "Bookkeeper");
        let rig = Rig::new(&s, None);
        let ctx = as_employee(PRIMARY_AGENT_ID);
        let res = rig
            .call(
                &ctx,
                "create_team",
                json!({"name": "Back Office", "mission": "Books and calendar", "members": ["Executive Assistant", "Bookkeeper"], "lead": "Bookkeeper"}),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        let t = &s.list_teams().unwrap()[0];
        assert_eq!(t.organizer_agent_id, "bk");
        assert!(!res.content.contains("no lead yet"), "{}", res.content);

        let res = rig.call(&ctx, "update_team", json!({"team": "Back Office", "lead": "Executive Assistant"})).await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(s.list_teams().unwrap()[0].organizer_agent_id, "ea");

        let res = rig.call(&ctx, "update_team", json!({"team": "Back Office", "lead": "Nobody Here"})).await;
        assert!(res.is_error && res.content.contains("NOT changed"), "{}", res.content);
        assert_eq!(s.list_teams().unwrap()[0].organizer_agent_id, "ea");
    }

    /// A comm plugin that reports no loops (loopback) is the same as none.
    #[tokio::test]
    async fn create_works_with_a_plugin_in_no_loop() {
        let s = store();
        install(&s, "chief", "Chief of Staff");
        install(&s, "ea", "Executive Assistant");
        let comm: Arc<dyn CommPlugin> = Arc::new(comm::LoopbackPlugin::new());
        comm.connect(std::collections::HashMap::new()).await.unwrap();
        let rig = Rig::new(&s, Some(comm));
        let res = rig.call(&as_employee("chief"), "create_team", json!({"name": "Sales", "members": ["ea"]})).await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(s.list_teams().unwrap()[0].hub_channel_id, None);
    }

    /// Solo teams and unknown employees are refused, and the refusal teaches
    /// the right call.
    #[tokio::test]
    async fn create_refuses_solo_and_unknown_members() {
        let s = store();
        install(&s, "chief", "Chief of Staff");
        let rig = Rig::new(&s, None);
        let ctx = as_employee("chief");
        let solo = rig.call(&ctx, "create_team", json!({"name": "Solo", "members": []})).await;
        assert!(solo.is_error);
        assert!(solo.content.contains("at least two employees") && solo.content.contains("create_team("), "{}", solo.content);

        let unknown = rig.call(&ctx, "create_team", json!({"name": "Ops", "members": ["Nobody"]})).await;
        assert!(unknown.is_error);
        assert!(unknown.content.contains("No employee named \"Nobody\""), "{}", unknown.content);
        assert!(s.list_teams().unwrap().is_empty());

        let empty = rig.call(&ctx, "list_teams", json!({})).await;
        assert_eq!(empty.content, team::no_teams_hint());
    }

    #[test]
    fn reads_are_read_only_and_changes_are_not() {
        let rig = Rig::new(&store(), None);
        for t in &rig.0 {
            assert_eq!(t.read_only(&json!({})), matches!(t.name(), "list_teams" | "team_members" | "team_messages"), "{}", t.name());
        }
    }
}
