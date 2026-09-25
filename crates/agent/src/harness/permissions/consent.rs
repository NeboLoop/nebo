//! Consent to a job (Turn-Controller-Technical-Design §2.12.6-2.12.7).
//!
//! The owner consents to jobs, not actions. A job is a set of capability
//! allow rules at the employee's scope; this module writes them when the
//! owner says yes: the Hire tap, "yes, create it" after the line in chat,
//! Create in the builder, an approved job edit. An employee that makes
//! another hands over at most what it holds itself: the new employee works
//! under its creator's grant (`Ceiling::Creator`) and anything beyond goes to
//! the owner as one card, listing the extras. Only the owner widens.

use std::sync::Arc;

use tokio::sync::RwLock;
use tools::needs::{CapabilityTerm, DescriptionReader, Granted, JobConsent, JobGrant, Needs};
use tools::ToolContext;
use types::permissions::{
    AskCase, CallEffects, Ceiling, Effect, Grant, Mode, Rule, RuleError, RuleKey, RuleSource, Scope, Target,
    Writer,
};

use super::RuleSet;

/// Grant `needs` to `agent_id` as standing allow rules: the owner's consent
/// (hire, creation, a job edit), named by `source`.
pub fn grant_job(store: &db::Store, agent_id: &str, needs: &Needs, source: RuleSource) -> Result<Vec<Rule>, RuleError> {
    write_capabilities(store, agent_id, needs.capabilities.iter(), source, &Writer::Owner)
}

fn write_capabilities<'a>(
    store: &db::Store,
    agent_id: &str,
    capabilities: impl Iterator<Item = &'a String>,
    source: RuleSource,
    by: &Writer,
) -> Result<Vec<Rule>, RuleError> {
    let now = chrono::Utc::now().timestamp();
    capabilities
        .map(|c| {
            let rule = Rule {
                id: uuid::Uuid::new_v4().to_string(),
                scope: Scope::Employee(agent_id.to_string()),
                key: RuleKey::Capability(c.clone()),
                field: None,
                effect: Effect::Allow,
                money: None,
                source: source.clone(),
                locked: false,
                created_at: now,
            };
            store.write_permission_rule(&rule, by)
        })
        .collect()
}

/// The job an employee holds now: its own capability allow rules.
pub fn job_of(store: &db::Store, agent_id: &str) -> Result<Needs, types::NeboError> {
    let capabilities = store
        .permission_rules_in(&Scope::Employee(agent_id.to_string()))?
        .into_iter()
        .filter(|r| r.effect == Effect::Allow && r.field.is_none())
        .filter_map(|r| match r.key {
            RuleKey::Capability(c) => Some(c),
            _ => None,
        })
        .collect();
    Ok(Needs { capabilities, accounts: Vec::new() })
}

/// Whether the owner said yes to a draft: one of the owner's own messages
/// arrived in the chat its line was shown in, after it was shown.
pub fn owner_consented(store: &db::Store, draft_id: &str) -> Result<bool, types::NeboError> {
    let Some(draft) = store.get_employee_draft(draft_id)? else {
        return Ok(false);
    };
    if draft.chat_id.is_empty() {
        return Ok(false);
    }
    Ok(store.owner_messages_after(&draft.chat_id, draft.shown_at)? > 0)
}

/// Whether `creator` holds `capability`: a call on it would be neither
/// refused nor outside its job, under its own ceiling too.
fn holds(creator: &Grant, capability: &str) -> bool {
    let t = Target {
        tool: String::new(),
        key: capability.to_string(),
        operation: None,
        capability: Some(capability.to_string()),
        field: None,
        read_only: false,
        effects: CallEffects::unknown(),
    };
    let rules = RuleSet::of(creator);
    let own = match rules.decide(&t) {
        Some((_, Effect::Deny)) => false,
        _ if creator.mode == Mode::Plan => false,
        _ if creator.mode == Mode::FullAccess => true,
        _ => rules.in_job(&t, &serde_json::json!({})),
    };
    own && creator.ceiling.as_ref().is_none_or(|c| holds(c.grant(), capability))
}

/// Make a new employee's job when an employee, not the owner, made it: the
/// needs its creator holds become its rules, it works under its creator's
/// grant from now on, and the rest goes to the owner as one card.
pub fn create_under_creator(
    store: &db::Store,
    creator: &Grant,
    agent_id: &str,
    name: &str,
    needs: &Needs,
    draft_id: &str,
    session_key: &str,
) -> Result<Granted, RuleError> {
    let store_err = |e: types::NeboError| RuleError::Store(e.to_string());
    let (held, beyond): (Vec<&String>, Vec<&String>) = needs.capabilities.iter().partition(|c| holds(creator, c));
    let by = Writer::Creator { creator_id: creator.agent_id.clone() };
    write_capabilities(store, agent_id, held.iter().copied(), RuleSource::Created { draft_id: draft_id.to_string() }, &by)?;
    let granted = Needs { capabilities: held.into_iter().cloned().collect(), accounts: Vec::new() };
    let extras = Needs { capabilities: beyond.into_iter().cloned().collect(), accounts: Vec::new() };
    let card = if extras.is_empty() {
        None
    } else {
        Some(raise_extras_card(store, creator, agent_id, name, &extras, session_key).map_err(store_err)?)
    };
    store
        .set_employee_ceiling(&db::EmployeeCeilingRow {
            agent_id: agent_id.to_string(),
            creator_id: creator.agent_id.clone(),
            extras: serde_json::to_string(&extras.capabilities).unwrap_or_default(),
            ask_id: card.clone().unwrap_or_default(),
            created_at: chrono::Utc::now().timestamp(),
        })
        .map_err(store_err)?;
    Ok(Granted { granted, extras, card })
}

/// The one card for needs only the owner can give: it lists them all, and
/// its answer is [`answer_extras`].
pub fn raise_extras_card(
    store: &db::Store,
    asker: &Grant,
    agent_id: &str,
    name: &str,
    extras: &Needs,
    session_key: &str,
) -> Result<String, types::NeboError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let case = AskCase::CreatedExtras { capabilities: extras.capabilities.iter().cloned().collect() };
    store.insert_permission_ask(&db::PermissionAskRow {
        id: id.clone(),
        agent_id: agent_id.to_string(),
        session_key: session_key.to_string(),
        chat_id: None,
        door: serde_json::to_string(&types::permissions::Door::Chat).unwrap_or_default(),
        ask_case: serde_json::to_string(&case).unwrap_or_default(),
        sentence: tools::needs::consent_line(name, extras),
        target: serde_json::json!({ "agent_id": agent_id, "capabilities": extras.capabilities }).to_string(),
        call: "{}".to_string(),
        seat: serde_json::to_string(asker).unwrap_or_default(),
        status: "open".to_string(),
        created_at: now,
        expires_at: now + super::ask::EXPIRES_AFTER_SECS,
    })?;
    Ok(id)
}

/// The owner's answer to an extras card. Allow grants the extras as the
/// owner's and lifts the creator's ceiling; No leaves both as they are.
pub fn answer_extras(store: &db::Store, ask_id: &str, allow: bool) -> Result<(), RuleError> {
    let store_err = |e: types::NeboError| RuleError::Store(e.to_string());
    if !allow {
        return Ok(());
    }
    let ask = store.get_permission_ask(ask_id).map_err(store_err)?.ok_or(RuleError::NotFound)?;
    let Ok(AskCase::CreatedExtras { capabilities }) = serde_json::from_str::<AskCase>(&ask.ask_case) else {
        return Err(RuleError::NotFound);
    };
    write_capabilities(
        store,
        &ask.agent_id,
        capabilities.iter(),
        RuleSource::AllowAlways { ask_id: ask_id.to_string() },
        &Writer::Owner,
    )?;
    if store.employee_ceiling_by_ask(ask_id).map_err(store_err)?.is_some() {
        store.clear_employee_ceiling(&ask.agent_id).map_err(store_err)?;
    }
    Ok(())
}

/// The ceiling an employee made by an employee works under until the owner
/// answers its card: its creator's grant, as it stands now.
pub fn creator_ceiling(store: &db::Store, agent_id: &str) -> Option<Ceiling> {
    let row = store.employee_ceiling(agent_id).ok().flatten()?;
    let creator = super::resolve_grant(store, &row.creator_id, None);
    Some(Ceiling::Creator { creator_id: row.creator_id, grant: Box::new(creator) })
}

/// The grant a tool call's run holds: the seat's, or the one its session's
/// employee holds.
fn run_grant(store: &db::Store, ctx: &ToolContext) -> Grant {
    match &ctx.grant {
        Some(g) => (**g).clone(),
        None => super::resolve_grant(store, &types::keyparser::extract_agent_id(&ctx.session_key), None),
    }
}

/// The permission system's side of making and changing jobs, for the create
/// tool: the description reader, the owner's consent and the grant.
pub struct Consent {
    store: Arc<db::Store>,
    reader: Arc<dyn DescriptionReader>,
}

impl Consent {
    pub fn new(store: Arc<db::Store>, reader: Arc<dyn DescriptionReader>) -> Self {
        Self { store, reader }
    }
}

#[async_trait::async_trait]
impl DescriptionReader for Consent {
    async fn capabilities_in(&self, description: &str, vocabulary: &[CapabilityTerm]) -> Vec<String> {
        self.reader.capabilities_in(description, vocabulary).await
    }
}

#[async_trait::async_trait]
impl JobConsent for Consent {
    fn owner_consented(&self, draft_id: &str) -> bool {
        owner_consented(&self.store, draft_id).unwrap_or_else(|e| {
            tracing::warn!(draft = draft_id, error = %e, "consent unreadable; treated as not given");
            false
        })
    }

    fn grant(&self, ctx: &ToolContext, job: &JobGrant<'_>) -> Result<Granted, String> {
        let source = if job.created {
            RuleSource::Created { draft_id: job.draft_id.to_string() }
        } else {
            RuleSource::JobEdit
        };
        if job.consented {
            grant_job(&self.store, job.agent_id, job.needs, source).map_err(|e| e.to_string())?;
            return Ok(Granted { granted: job.needs.clone(), ..Granted::default() });
        }
        let asker = run_grant(&self.store, ctx);
        if job.created {
            return create_under_creator(
                &self.store,
                &asker,
                job.agent_id,
                job.name,
                job.needs,
                job.draft_id,
                &ctx.session_key,
            )
            .map_err(|e| e.to_string());
        }
        // An employee never widens another: the whole addition is the card.
        if job.needs.is_empty() {
            return Ok(Granted::default());
        }
        let card = raise_extras_card(&self.store, &asker, job.agent_id, job.name, job.needs, &ctx.session_key)
            .map_err(|e| e.to_string())?;
        Ok(Granted { extras: job.needs.clone(), card: Some(card), ..Granted::default() })
    }

    fn job_of(&self, agent_id: &str) -> Needs {
        job_of(&self.store, agent_id).unwrap_or_default()
    }
}

/// Reads a job description onto the capability vocabulary with one call on
/// the aux route (the cheapest model when none is set). It has no tools and
/// answers JSON; anything outside the vocabulary is dropped by the caller.
/// A failed or late call reads nothing: the job then asks when it first
/// needs a capability (outside its job), never less.
pub struct AuxReader {
    providers: Arc<RwLock<Vec<Arc<dyn ai::Provider>>>>,
}

impl AuxReader {
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);
    const SYSTEM: &'static str = "You read a job description and say which capabilities the job needs. \
        Use only keys from the list you are given. Include a key only when the description says the job \
        does that work; never add one because it might be useful. Answer with JSON only: \
        {\"capabilities\": [\"key\", ...]}.";

    pub fn new(providers: Arc<RwLock<Vec<Arc<dyn ai::Provider>>>>) -> Self {
        Self { providers }
    }

    async fn ask(&self, prompt: String) -> Option<String> {
        let (provider, model) = {
            let providers = self.providers.read().await;
            match crate::harness::model_call::resolve_aux(&config::ModelsConfig::load(), &providers) {
                Some(routed) => routed,
                None => (crate::summarizer::pick_cheapest(&providers)?, String::new()),
            }
        };
        let req = ai::ChatRequest {
            tool_credential: None,
            tool_choice: Default::default(),
            messages: vec![ai::Message { role: "user".to_string(), content: prompt, ..Default::default() }],
            tools: vec![],
            max_tokens: 400,
            temperature: 0.0,
            system: Self::SYSTEM.to_string(),
            static_system: String::new(),
            model,
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
            trace: ai::RequestTrace::new("job_needs"),
        };
        let mut rx = provider.stream(&req).await.ok()?;
        let mut text = String::new();
        while let Some(event) = rx.recv().await {
            match event.event_type {
                ai::StreamEventType::Text => text.push_str(&event.text),
                ai::StreamEventType::Error => return None,
                ai::StreamEventType::Done => break,
                _ => {}
            }
        }
        Some(text)
    }
}

/// The keys in a reader's JSON answer.
fn parse_capabilities(text: &str) -> Vec<String> {
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => return Vec::new(),
    };
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("capabilities").and_then(|c| c.as_array()).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

#[async_trait::async_trait]
impl DescriptionReader for AuxReader {
    async fn capabilities_in(&self, description: &str, vocabulary: &[CapabilityTerm]) -> Vec<String> {
        let list: String = vocabulary.iter().map(|t| format!("- {}: {}\n", t.key, t.words)).collect();
        let prompt = format!("Capabilities:\n{list}\nJob description:\n{description}");
        match tokio::time::timeout(Self::DEADLINE, self.ask(prompt)).await {
            Ok(Some(text)) => parse_capabilities(&text),
            Ok(None) => {
                tracing::warn!("job needs: the description reader got no answer; the description adds nothing");
                Vec::new()
            }
            Err(_) => {
                tracing::warn!("job needs: the description reader timed out; the description adds nothing");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests;
