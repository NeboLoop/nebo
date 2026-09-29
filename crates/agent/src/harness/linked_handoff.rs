//! What a linked employee's fresh agent session is told
//! ([`ai::LinkedContext`]). Nebo keeps the conversation; the agent keeps
//! one working stretch of it in its session. A fresh session starts with a
//! briefing of who the employee is here; one that carries a conversation on
//! (its session stalled, got long, or sat idle) also gets the conversation
//! so far, summarized by the checkpoint's own call over Nebo's copy of it.
//! The linked provider composes the words and decides when; this is the
//! part only the harness can read.

use std::collections::BTreeSet;

use ai::{ChatRequest, RequestTrace};

use super::compact::checkpoint;
use super::events;
use super::model_call::{prefer_non_gateway, resolve_aux};
use super::Harness;

/// The most teammates a briefing names.
const TEAMMATES: usize = 12;
/// The most of the company's own description a briefing carries.
const COMPANY_CHARS: usize = 400;
/// The most of each teammate's or team's line a briefing carries.
const LINE_CHARS: usize = 160;

/// One linked turn's context.
pub struct Handoff {
    harness: Harness,
    session_id: String,
    agent_id: String,
    name: String,
    /// The turn's own briefing (`TurnDelivery::mention_briefing`).
    run_briefing: Option<String>,
    trace: RequestTrace,
}

impl Handoff {
    /// For the turn of the employee `agent_id` (named `name`) on the
    /// session `session_id`, in the run `run_id`, asked with `run_briefing`.
    pub fn new(harness: Harness, session_id: &str, agent_id: &str, name: &str, run_id: &str, run_briefing: Option<String>) -> Self {
        Self {
            harness,
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            name: name.to_string(),
            run_briefing,
            trace: RequestTrace {
                agent_id: agent_id.to_string(),
                run_id: run_id.to_string(),
                ..RequestTrace::new("linked_handoff")
            },
        }
    }
}

#[async_trait::async_trait]
impl ai::LinkedContext for Handoff {
    fn briefing(&self) -> String {
        let packs = config::packs_dir().map(|dir| napp::scan_packs(&dir)).unwrap_or_default();
        briefing(&self.harness.store, &packs, &self.agent_id, &self.name)
    }

    fn run_briefing(&self) -> Option<String> {
        self.run_briefing.clone()
    }

    async fn summary(&self) -> Option<String> {
        let conversation = self.harness.sessions.get_messages_since_checkpoint(&self.session_id).ok()?;
        // Everything before the owner's newest message, which the fresh
        // session is sent after this.
        let upto = conversation.iter().rposition(|m| m.role == "user").unwrap_or(conversation.len());
        let before = &conversation[..upto];
        if !before.iter().any(|m| m.role == "assistant") {
            return None;
        }
        let (provider, model) = {
            let providers = self.harness.providers.read().await;
            resolve_aux(&config::ModelsConfig::load(), &providers).or_else(|| prefer_non_gateway(&providers).map(|p| (p, String::new())))?
        };
        let provider = self.harness.concurrency.background(provider);
        let base = ChatRequest {
            model,
            ..ChatRequest::new(self.trace.clone())
        };
        match checkpoint::summary_of(provider.as_ref(), &base, before, &self.session_id).await {
            Ok(summary) => Some(summary),
            Err(e) => {
                tracing::warn!(session_id = %self.session_id, error = %e, "linked: the conversation could not be summarized for the fresh session");
                None
            }
        }
    }
}

/// Who the employee `agent_id` (named `name`) is here, for its agent's
/// fresh session: the company (the company pack among `packs`), its job, the
/// teams it is on and the teammates it can reach there, and the owner. The
/// same listings the employee's own turns are told (`events`); short, and
/// no ids.
pub fn briefing(store: &db::Store, packs: &[napp::Pack], agent_id: &str, name: &str) -> String {
    let owner = store
        .get_user_profile()
        .ok()
        .flatten()
        .and_then(|p| p.display_name)
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let company = packs.iter().find(|p| p.layer == napp::PackLayer::Company);
    let job = store.get_agent(agent_id).ok().flatten().map(|a| a.description).filter(|d| !d.trim().is_empty());

    let mut out = format!(
        "You are {name}, an AI employee working for {}{} through Nebo. Nebo keeps this conversation; you do the work in it.",
        owner.as_deref().unwrap_or("your owner"),
        company.map(|c| format!(" at {}", c.name)).unwrap_or_default(),
    );
    if let Some(job) = job {
        out.push_str(&format!("\n\nYour job here: {}", cut(job.trim(), LINE_CHARS * 2)));
    }
    if let Some(about) = company.and_then(|c| first_paragraph(&c.body)) {
        out.push_str(&format!("\n\nAbout {}: {}", company.map(|c| c.name.as_str()).unwrap_or_default(), cut(&about, COMPANY_CHARS)));
    }

    let teams: Vec<db::Team> =
        store.list_teams().unwrap_or_default().into_iter().filter(|t| t.members.iter().any(|m| m.agent_id == agent_id)).collect();
    if teams.is_empty() {
        out.push_str("\n\nYou are on no team yet.");
        return out;
    }
    out.push_str("\n\nYour teams:");
    let mut teammates: BTreeSet<String> = BTreeSet::new();
    for team in &teams {
        let (team_name, line) = events::team_entry(store, team);
        out.push_str(&format!("\n- {team_name}: {}", cut(&line, LINE_CHARS)));
        for (id, member) in tools::team::member_roster(store, team) {
            if id != agent_id {
                teammates.insert(member);
            }
        }
    }
    if !teammates.is_empty() {
        let listing = events::employees_listing(store, name);
        out.push_str("\n\nTeammates you can reach:");
        for member in teammates.iter().take(TEAMMATES) {
            match listing.get(member).map(|d| d.trim()).filter(|d| !d.is_empty()) {
                Some(does) => out.push_str(&format!("\n- {member}: {}", cut(does, LINE_CHARS))),
                None => out.push_str(&format!("\n- {member}")),
            }
        }
        out.push_str(&format!(
            "\n\nIn a team conversation, write @Name to hand a step to a teammate, or @everyone to ask the whole team. For anything else a teammate should do, ask {} to pass it on.",
            owner.as_deref().unwrap_or("your owner")
        ));
    }
    out
}

/// The first paragraph of a markdown body, past its headings.
fn first_paragraph(body: &str) -> Option<String> {
    body.split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty() && !p.starts_with('#'))
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `text` cut to `max` characters, the cut marked.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{} …", head.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        (dir, store)
    }

    fn company(dir: &std::path::Path) -> napp::Pack {
        let pack = dir.join("packs").join("sample");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("COMPANY.md"),
            "---\ntype: company\ncompany: Sample Co\nversion: 1.0.0\n---\n\n# Sample Co\n\nWe fix bikes and teach people to ride.\n\nMore below.\n",
        )
        .unwrap();
        napp::load_pack(&pack).unwrap()
    }

    /// A fresh session hears who it is here: the company and what it does,
    /// its job, its teams, the teammates it can reach and how, and the
    /// owner, by name; never an id.
    #[test]
    fn the_briefing_says_who_the_employee_is_here() {
        let (dir, store) = store();
        let packs = vec![company(dir.path())];
        // The owner's profile, as onboarding leaves it.
        let owner = store.ensure_local_user_id().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("t.db")).unwrap();
        conn.execute("INSERT INTO user_profiles (user_id, display_name, created_at, updated_at) VALUES (?1, 'Sam', 0, 0)", rusqlite::params![owner])
            .unwrap();
        for (id, name, job) in [("cc", "Coder", "Keeps the shop's website running."), ("ava", "Ava", "Plans the week."), ("bo", "Bo", "Writes the ads.")] {
            store.create_agent(id, None, name, job, "", "", None, None).unwrap();
        }
        store.create_team("t-web", "Web", "the website", &[db::TeamMember::local("cc"), db::TeamMember::local("ava")], "ava", None).unwrap();
        store.create_team("t-ads", "Ads", "", &[db::TeamMember::local("ava"), db::TeamMember::local("bo")], "", None).unwrap();

        let text = briefing(&store, &packs, "cc", "Coder");
        assert!(text.starts_with("You are Coder, an AI employee working for Sam at Sample Co through Nebo."), "{text}");
        assert!(text.contains("Your job here: Keeps the shop's website running."), "{text}");
        assert!(text.contains("About Sample Co: We fix bikes and teach people to ride."), "{text}");
        assert!(text.contains("- Web: owns the website; lead: Ava; members: Coder, Ava"), "{text}");
        assert!(!text.contains("Ads"), "only its own teams: {text}");
        assert!(text.contains("Teammates you can reach:\n- Ava: Plans the week."), "{text}");
        assert!(!text.contains("- Bo"), "only its teammates: {text}");
        assert!(text.contains("write @Name to hand a step to a teammate, or @everyone"), "{text}");
        assert!(!text.contains("t-web") && !text.contains("cc,"), "no ids: {text}");
        assert!(text.len() < 1_500, "short: {} chars", text.len());
    }

    /// An employee on no team, with no company and no owner's name, still
    /// hears who it is, plainly.
    #[test]
    fn a_briefing_with_nothing_around_the_employee_is_plain() {
        let (_dir, store) = store();
        store.create_agent("cc", None, "Coder", "", "", "", None, None).unwrap();
        let text = briefing(&store, &[], "cc", "Coder");
        assert_eq!(
            text,
            "You are Coder, an AI employee working for your owner through Nebo. Nebo keeps this conversation; you do the work in it.\n\nYou are on no team yet."
        );
    }
}
