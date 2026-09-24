//! `team` tool — teams of AI employees on THIS Nebo. Resource-less: create,
//! list, send, messages, members. Everything works with no hub; a hub loop
//! only adds a mirror. The `loop` tool's `workroom` / `create` and its
//! channel actions on a team id are aliases over these same methods.

use std::sync::Arc;

use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::team;
use comm::CommPlugin;

/// The primary employee's local agent id — a call from the main companion
/// chat carries no agent segment in its session key, and the companion IS
/// the primary employee.
const PRIMARY_AGENT_ID: &str = "assistant";

pub struct TeamTool {
    store: Option<Arc<db::Store>>,
    /// Hub plugin, for the optional mirror only.
    comm: Option<Arc<dyn CommPlugin>>,
    /// ClientHub broadcast — the sidebar learns about a new team live.
    broadcast: Option<crate::web_tool::Broadcaster>,
    /// The coworker rail posts into teams (thread + fan-out + hub mirror).
    rail: crate::coworker::CoworkerRailCell,
}

impl TeamTool {
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

    /// `agents` / `mention`: a string (comma-separated) or an array of
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

    /// One line per team for `list` answers (shared with the loop tool).
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
        if name.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team create",
                "name",
                team::CREATE_USAGE,
            ));
        }
        let mission = input["mission"].as_str().unwrap_or("");

        // The primary employee is the platform, not a teammate: when it
        // creates a team it does so on the owner's behalf — the owner is the
        // organizer and the primary stays outside the roster. Every other
        // employee joins the team it creates, as organizer.
        let caller = Self::caller_agent_id(store, ctx);
        let mut organizer = if caller == PRIMARY_AGENT_ID { String::new() } else { caller };
        let mut member_ids: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for label in Self::labels(&input["agents"]) {
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
                     Start the work by posting the first ask: team(action: \"send\", team: \"{}\", \
                     text: \"...\") — the lead answers and hands steps to teammates by mention; add \
                     mention: [\"Member Name\"] to ask specific members, or write @everyone in the \
                     text to ask the whole team.{}",
                    t.name, t.id, members, mirror, t.name,
                    if t.organizer_agent_id.is_empty() {
                        " This team has no lead yet, so every member answers an owner post; set one \
                         with team(action: \"update\", team: \"...\", lead: \"Employee Name\")."
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
            "{} team(s) on this Nebo\n{}\nPost with team(action: \"send\", team: \"<name>\", text: \"...\").",
            teams.len(),
            lines.join("\n")
        ))
    }

    pub async fn send(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let store = match self.store() {
            Ok(s) => s,
            Err(r) => return r,
        };
        let label = input["team"]
            .as_str()
            .or_else(|| input["team_id"].as_str())
            .or_else(|| input["channel_id"].as_str())
            .unwrap_or("");
        if label.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team send",
                "team",
                "team(action: \"send\", team: \"Operations\", text: \"...\", mention: [\"Executive Assistant\"])",
            ));
        }
        let text = input["text"].as_str().unwrap_or("").trim();
        if text.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team send",
                "text",
                "team(action: \"send\", team: \"Operations\", text: \"...\")",
            ));
        }
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };

        // `mention`: members asked to act, resolved against the team roster.
        let mut mention: Vec<String> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        for m in Self::labels(&input["mention"]) {
            match team::resolve_agent(store, &m) {
                Some(a) if t.members.iter().any(|m| m.agent_id == a.id) => {
                    if !mention.contains(&a.id) {
                        mention.push(a.id);
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
            mention,
            handoff_depth: ctx.handoff_depth,
            provenance: ctx.run_taint.clone(),
            is_reply: false,
        };
        match rail.post_team(post).await {
            Ok(receipt) => {
                let asked = if receipt.asked.is_empty() {
                    "Every member read it as context; nobody was asked to act (mention members to ask them).".to_string()
                } else {
                    format!(
                        "Asked to act: {}. They answer in the team; their replies reach you as team posts.",
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
        let label = input["team"]
            .as_str()
            .or_else(|| input["team_id"].as_str())
            .or_else(|| input["channel_id"].as_str())
            .unwrap_or("");
        if label.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team messages",
                "team",
                "team(action: \"messages\", team: \"Operations\", limit: 20)",
            ));
        }
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        let limit = input["limit"].as_u64().unwrap_or(50) as usize;
        match store.list_team_messages(&t.id, limit) {
            Ok(msgs) if msgs.is_empty() => ToolResult::ok(format!(
                "No messages in team \"{}\" yet. Post the first one with team(action: \"send\", team: \"{}\", text: \"...\").",
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
        let label = input["team"].as_str().or_else(|| input["team_id"].as_str()).unwrap_or("");
        if label.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team update",
                "team",
                "team(action: \"update\", team: \"Operations\", lead: \"Executive Assistant\")",
            ));
        }
        let t = match team::resolve_team(store, label) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };
        let mut unknown: Vec<String> = Vec::new();
        let members: Option<Vec<String>> = if input["agents"].is_array() {
            let mut ids: Vec<String> = Vec::new();
            for l in Self::labels(&input["agents"]) {
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
        let label = input["team"]
            .as_str()
            .or_else(|| input["team_id"].as_str())
            .or_else(|| input["channel_id"].as_str())
            .unwrap_or("");
        if label.is_empty() {
            return ToolResult::error(errors::missing_param(
                "team members",
                "team",
                "team(action: \"members\", team: \"Operations\")",
            ));
        }
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

impl DynTool for TeamTool {
    fn name(&self) -> &str {
        "team"
    }

    fn description(&self) -> String {
        format!(
            "Teams — groups of AI employees on THIS Nebo that share one mission and one conversation. \
             Teams work locally: no hub, no NeboAI connection needed. Every member reads every post; \
             the LEAD answers the owner and hands steps to teammates by mention; members you mention \
             are asked to act; @everyone in the text asks the whole team once.\n\
             USE THIS when: the user wants a team, a group of employees working together on a mission, or asks what teams exist.\n\n\
             - {create} — Create a team. `agents` is REQUIRED: at least one coworker besides you (you are always a member). Add lead: \"Employee Name\" to name who runs the room; without one the owner leads and every member answers. Returns the team id and members.\n\
             - team(action: \"update\", team: \"Operations\", lead: \"Executive Assistant\") — Change the lead (also name, mission, agents); lead: \"owner\" makes it owner-led\n\
             - team(action: \"list\") — The teams on this Nebo with their missions, leads and members\n\
             - team(action: \"send\", team: \"Operations\", text: \"...\") — Post to the team; the lead answers, or every member once if the team has no lead\n\
             - team(action: \"send\", team: \"Operations\", text: \"...\", mention: [\"Executive Assistant\"]) — Ask specific members to act\n\
             - team(action: \"messages\", team: \"Operations\", limit: 20) — Read the team's conversation\n\
             - team(action: \"members\", team: \"Operations\") — Who is in the team\n\n\
             `team` takes the team's name or id. To reach ONE coworker outside any team, use message(resource: \"coworker\").",
            create = team::CREATE_USAGE
        )
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "REQUIRED. What to do.",
                    "enum": ["create", "update", "list", "send", "messages", "members"]
                },
                "name": { "type": "string", "description": "Team name (create; update to rename)" },
                "mission": { "type": "string", "description": "What the team exists to accomplish (create, update)" },
                "agents": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "REQUIRED for create: employee names to bring into the team — at least one besides yourself. A team with nobody to work with is refused. On update: the full new member list."
                },
                "lead": { "type": "string", "description": "Employee name that leads the team — answers the owner and delegates by mention (create, update). \"owner\" on update makes the team owner-led." },
                "team": { "type": "string", "description": "Team name or id (update, send, messages, members)" },
                "text": { "type": "string", "description": "Post text (send)" },
                "mention": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Members asked to act on this post (send). Without it, every member may answer once."
                },
                "limit": { "type": "integer", "description": "Max messages to return (messages)" }
            },
            "required": ["action"]
        })
    }


    fn search_hint(&self) -> &str {
        "teams of employees members posts"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        matches!(input.get("action").and_then(|v| v.as_str()), Some("list" | "messages" | "members"))
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "create" => "create_team",
            "update" => "update_team",
            "send" => "send_message",
            "messages" => "team_messages",
            "members" => "team_members",
            _ => "list_teams",
        }
        .to_string()
    }

    /// Pre-interface: it settles its own call shapes (see
    /// `DynTool::validates_input`).
    fn validates_input(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("").trim();
            match action {
                "create" => self.create(&input, ctx).await,
                "update" | "edit" => self.update(&input),
                "list" => self.list(),
                "send" | "post" => self.send(&input, ctx).await,
                "messages" | "history" => self.messages(&input),
                "members" => self.members(&input),
                "" => ToolResult::error(
                    "Action is required. Available: create, update, list, send, messages, members",
                ),
                other => ToolResult::error(format!(
                    "Unknown team action: {}. Available: create, update, list, send, messages, members",
                    other
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<db::Store> {
        let path = std::env::temp_dir().join(format!("nebo-team-tool-test-{}.db", uuid::Uuid::new_v4()));
        Arc::new(db::Store::new(&path.to_string_lossy()).expect("store"))
    }

    fn install(store: &db::Store, id: &str, name: &str) {
        store
            .create_agent(id, None, name, &format!("{name} description"), "# agent", "", None, None)
            .expect("create agent");
    }

    /// The create path with NO comm plugin: the team exists, with the caller
    /// as organizer plus the named coworkers, and no hub channel.
    #[tokio::test]
    async fn create_works_with_no_comm_plugin() {
        let s = store();
        install(&s, "chief", "Chief of Staff");
        install(&s, "ea", "Executive Assistant");
        let tool = TeamTool::new(Some(s.clone()), None, None, crate::coworker::new_rail_cell());
        let ctx = ToolContext {
            session_key: "agent:chief:web".to_string(),
            ..ToolContext::default()
        };
        let res = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({
                    "action": "create",
                    "name": "Operations",
                    "mission": "Keep the office running",
                    "agents": ["Executive Assistant"]
                }),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("Team \"Operations\" exists"), "{}", res.content);
        let teams = s.list_teams().unwrap();
        assert_eq!(teams.len(), 1);
        let ids: Vec<&str> = teams[0].members.iter().map(|m| m.agent_id.as_str()).collect();
        assert_eq!(ids, vec!["chief", "ea"]);
        assert_eq!(teams[0].hub_channel_id, None);

        let listed = tool.execute_dyn(&ctx, serde_json::json!({"action": "list"})).await;
        assert!(listed.content.contains("Operations"), "{}", listed.content);
        assert!(listed.content.contains("Chief of Staff (lead"), "{}", listed.content);
    }

    /// The primary creates a team on the owner's behalf and names the lead;
    /// later the lead can be changed through update, by name.
    #[tokio::test]
    async fn primary_names_a_lead_and_can_change_it() {
        let s = store();
        install(&s, "ea", "Executive Assistant");
        install(&s, "bk", "Bookkeeper");
        let tool = TeamTool::new(Some(s.clone()), None, None, crate::coworker::new_rail_cell());
        let ctx = ToolContext {
            session_key: format!("agent:{PRIMARY_AGENT_ID}:web"),
            ..ToolContext::default()
        };
        let res = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({
                    "action": "create", "name": "Back Office", "mission": "Books and calendar",
                    "agents": ["Executive Assistant", "Bookkeeper"], "lead": "Bookkeeper"
                }),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        let t = &s.list_teams().unwrap()[0];
        assert_eq!(t.organizer_agent_id, "bk");
        assert!(!res.content.contains("no lead yet"), "{}", res.content);

        let res = tool
            .execute_dyn(&ctx, serde_json::json!({ "action": "update", "team": "Back Office", "lead": "Executive Assistant" }))
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(s.list_teams().unwrap()[0].organizer_agent_id, "ea");

        let res = tool
            .execute_dyn(&ctx, serde_json::json!({ "action": "update", "team": "Back Office", "lead": "Nobody Here" }))
            .await;
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
        let tool = TeamTool::new(Some(s.clone()), Some(comm), None, crate::coworker::new_rail_cell());
        let ctx = ToolContext {
            session_key: "agent:chief:web".to_string(),
            ..ToolContext::default()
        };
        let res = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({"action": "create", "name": "Sales", "agents": ["ea"]}),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(s.list_teams().unwrap()[0].hub_channel_id, None);
    }

    /// Solo teams and unknown employees are refused, and the refusal teaches
    /// the right call.
    #[tokio::test]
    async fn create_refuses_solo_and_unknown_members() {
        let s = store();
        install(&s, "chief", "Chief of Staff");
        let tool = TeamTool::new(Some(s.clone()), None, None, crate::coworker::new_rail_cell());
        let ctx = ToolContext {
            session_key: "agent:chief:web".to_string(),
            ..ToolContext::default()
        };
        let solo = tool
            .execute_dyn(&ctx, serde_json::json!({"action": "create", "name": "Solo"}))
            .await;
        assert!(solo.is_error);
        assert!(solo.content.contains("at least two employees"), "{}", solo.content);

        let unknown = tool
            .execute_dyn(
                &ctx,
                serde_json::json!({"action": "create", "name": "Ops", "agents": ["Nobody"]}),
            )
            .await;
        assert!(unknown.is_error);
        assert!(unknown.content.contains("No employee named \"Nobody\""), "{}", unknown.content);
        assert!(s.list_teams().unwrap().is_empty());

        let empty = tool.execute_dyn(&ctx, serde_json::json!({"action": "list"})).await;
        assert_eq!(empty.content, team::no_teams_hint());
    }
}
