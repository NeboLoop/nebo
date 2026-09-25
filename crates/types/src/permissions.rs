//! What a tool call is, as the permission check reads it. Each tool's spec
//! (`tools::registry::DynTool`) answers these for one call; the registry
//! resolves them into a [`Target`] from the call as it will run. The check
//! itself (modes, rules, asks) builds on this data.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The value of a call that a rule can match, beside its rule key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RuleField {
    /// A shell command, matched by prefix.
    CommandPrefix(String),
    /// A file or folder, matched against folder rules.
    Folder(PathBuf),
    /// A web host.
    Domain(String),
    /// Who a message goes to.
    Recipient(String),
}

/// Whether an effect happens, when the call's input can say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Knowable {
    Yes,
    No,
    /// The input can't tell. Never a guess.
    #[default]
    Unknown,
}

/// What a call does outside its own work, as far as its input shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEffects {
    pub money_cents: Option<i64>,
    pub counterparty: Option<String>,
    pub recipients: Vec<String>,
    pub publishes: Knowable,
    pub deletes: Vec<String>,
    pub overwrites: Vec<String>,
    /// What the call brings into being, named the way a later delete or
    /// overwrite of it names it: once it runs, it is the employee's own work.
    #[serde(default)]
    pub creates: Vec<String>,
    /// The call gives an employee more room (a standing grant, a wider
    /// limit). Only the owner does that: it always asks, in every mode.
    #[serde(default)]
    pub widens: bool,
}

impl CallEffects {
    /// A call that changes nothing outside this process.
    pub fn none() -> Self {
        Self {
            publishes: Knowable::No,
            ..Self::default()
        }
    }

    /// A call whose outward effects the input can't show.
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// One call, resolved for the permission check from the tool's spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The registered tool that runs the call.
    pub tool: String,
    /// The key rules match: a tool name of the current set, or a catalog
    /// operation.
    pub key: String,
    pub operation: Option<String>,
    /// The job capability the call belongs to; `None` is basic work.
    pub capability: Option<String>,
    pub field: Option<RuleField>,
    /// The one thing the call acts on, when its tool names one: the
    /// `resource` of a tool that still dispatches on it, the workflow of a
    /// workflow tool. A restricted run's `tool:subject` allowlist entry
    /// admits only calls on that subject.
    #[serde(default)]
    pub subject: Option<String>,
    pub read_only: bool,
    pub effects: CallEffects,
}

/// How an employee's calls are decided when its rules don't settle them.
/// One per employee, overridable per run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Acts inside its job; asks only for the surfaced cases and ask rules.
    #[default]
    Automatic,
    /// Every call that changes something asks, unless an allow rule covers it.
    Ask,
    /// Read and plan only: a call that changes something doesn't run.
    Plan,
    /// Nothing asks. Deny rules and the hard limits still hold.
    FullAccess,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Automatic => "automatic",
            Mode::Ask => "ask",
            Mode::Plan => "plan",
            Mode::FullAccess => "full_access",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "automatic" => Some(Mode::Automatic),
            "ask" => Some(Mode::Ask),
            "plan" => Some(Mode::Plan),
            "full_access" => Some(Mode::FullAccess),
            _ => None,
        }
    }
}

/// What a rule does to a call it matches. Precedence: deny, then ask, then
/// allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Ask,
    Deny,
}

impl Effect {
    pub fn as_str(&self) -> &'static str {
        match self {
            Effect::Allow => "allow",
            Effect::Ask => "ask",
            Effect::Deny => "deny",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Effect::Allow),
            "ask" => Some(Effect::Ask),
            "deny" => Some(Effect::Deny),
            _ => None,
        }
    }
}

/// What a rule is keyed on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RuleKey {
    /// A rule key of the current tool set (`run_command`, `mail_message_send`,
    /// `mcp__server__tool`). A trailing `*` matches a family
    /// (`browser_*`, `mcp__server__*`).
    Tool(String),
    /// A catalog operation (`mail.message.send`).
    Operation(String),
    /// A job capability: a built-in (file, shell, web, desktop, media,
    /// system, contacts) or an interfaces-catalog term.
    Capability(String),
}

impl RuleKey {
    pub fn kind(&self) -> &'static str {
        match self {
            RuleKey::Tool(_) => "tool",
            RuleKey::Operation(_) => "operation",
            RuleKey::Capability(_) => "capability",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            RuleKey::Tool(v) | RuleKey::Operation(v) | RuleKey::Capability(v) => v,
        }
    }

    pub fn from_parts(kind: &str, value: String) -> Option<Self> {
        match kind {
            "tool" => Some(RuleKey::Tool(value)),
            "operation" => Some(RuleKey::Operation(value)),
            "capability" => Some(RuleKey::Capability(value)),
            _ => None,
        }
    }
}

impl RuleField {
    pub fn kind(&self) -> &'static str {
        match self {
            RuleField::CommandPrefix(_) => "command_prefix",
            RuleField::Folder(_) => "folder",
            RuleField::Domain(_) => "domain",
            RuleField::Recipient(_) => "recipient",
        }
    }

    pub fn value(&self) -> String {
        match self {
            RuleField::CommandPrefix(v) | RuleField::Domain(v) | RuleField::Recipient(v) => v.clone(),
            RuleField::Folder(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn from_parts(kind: &str, value: String) -> Option<Self> {
        match kind {
            "command_prefix" => Some(RuleField::CommandPrefix(value)),
            "folder" => Some(RuleField::Folder(PathBuf::from(value))),
            "domain" => Some(RuleField::Domain(value)),
            "recipient" => Some(RuleField::Recipient(value)),
            _ => None,
        }
    }
}

/// Where a rule lives: company defaults, or one employee's overrides.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "agent_id", rename_all = "snake_case")]
pub enum Scope {
    Company,
    Employee(String),
}

/// Money a standing allow covers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoneyLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_action_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_day_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_counterparty_day_cents: Option<i64>,
}

/// Who or what wrote a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleSource {
    Hire { package: String },
    Created { draft_id: String },
    JobEdit,
    AllowAlways { ask_id: String },
    Owner,
    Package { package: String },
    Law { pack: String },
    Migrated { from: String },
}

/// One permission rule, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub scope: Scope,
    pub key: RuleKey,
    pub field: Option<RuleField>,
    pub effect: Effect,
    pub money: Option<MoneyLimit>,
    pub source: RuleSource,
    /// A law or a package's must-ask: nothing but the pack or package that
    /// wrote it can remove it.
    pub locked: bool,
    pub created_at: i64,
}

/// What an employee holds for one run: its mode, its rules (company and
/// employee scope) and the ceiling it runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub agent_id: String,
    pub mode: Mode,
    pub rules: Vec<Rule>,
    pub ceiling: Option<Ceiling>,
    /// Folders this run works in that join a fenced job's folders (the
    /// project folder the owner opened the chat on). Nothing is stored.
    #[serde(default)]
    pub run_folders: Vec<PathBuf>,
    /// A hard fence for this run only: an isolated helper works in its own
    /// copy and nowhere else. File writes and shell outside are refused.
    #[serde(default)]
    pub fence: Option<Vec<PathBuf>>,
}

/// A grant a run can only narrow: its parent's (a helper) or its creator's
/// (an employee made by an employee, waiting on the owner's card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ceiling {
    Parent { grant: Box<Grant> },
    Creator { creator_id: String, grant: Box<Grant> },
}

impl Ceiling {
    pub fn grant(&self) -> &Grant {
        match self {
            Ceiling::Parent { grant } | Ceiling::Creator { grant, .. } => grant,
        }
    }
}

/// The entry a run came through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Door {
    #[default]
    Chat,
    Helper,
    Workflow,
    Schedule,
    Heartbeat,
    Coworker { from: String },
    Voice,
    Mcp,
    LocalApi,
}

impl Door {
    pub fn label(&self) -> &'static str {
        match self {
            Door::Chat => "chat",
            Door::Helper => "helper",
            Door::Workflow => "workflow",
            Door::Schedule => "schedule",
            Door::Heartbeat => "heartbeat",
            Door::Coworker { .. } => "coworker",
            Door::Voice => "voice",
            Door::Mcp => "mcp",
            Door::LocalApi => "local_api",
        }
    }
}

/// Why a call was allowed or refused, recorded with every decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Why {
    HardLimit { limit: String },
    Ceiling,
    Rule { rule_id: String },
    Mode { mode: Mode },
    BasicWork,
    AnsweredOnce { ask_id: String },
    /// The owner already said no to this same call in this session.
    Declined { ask_id: String },
    Judged { by: String, reason: String },
    Unreviewed { reason: String },
}

/// Which judge answered a question the code could not decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgedBy {
    /// The typed Jev decision: the primary judge.
    Jev,
    /// The classifier call on the turn's aux model route: the fallback.
    AuxClassifier,
}

impl JudgedBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            JudgedBy::Jev => "jev",
            JudgedBy::AuxClassifier => "aux_classifier",
        }
    }
}

/// The judgement's answer for one call whose effects the code could not
/// decide (Automatic mode, §2.12.4). The tool round asks the judges once
/// for all such calls and hands each call its verdict on its context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    Allow { by: JudgedBy, reason: String },
    Ask { case: AskCase, by: JudgedBy, reason: String },
    /// Neither judge answered: the call proceeds, marked unreviewed.
    Unjudged,
}

/// Whether the judgement's verdicts decide calls or are only recorded.
/// A company setting; shadow until its data shows the asks it would add
/// are rare and right.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgementMode {
    /// Recorded in the activity; the call proceeds on the code's answer.
    #[default]
    Shadow,
    /// An Ask verdict asks.
    Enforce,
}

impl JudgementMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            JudgementMode::Shadow => "shadow",
            JudgementMode::Enforce => "enforce",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "shadow" => Some(JudgementMode::Shadow),
            "enforce" => Some(JudgementMode::Enforce),
            _ => None,
        }
    }
}

/// The reason an unreviewed action carries in the activity and the digest.
pub const UNREVIEWED_REASON: &str = "the permission check couldn't run";

/// Someone a message goes to, the way the employee's history keeps them:
/// an email lowercased, a phone number as E.164, anything else trimmed and
/// lowercased. `None` when there is nothing usable.
pub fn address_key(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('@') {
        return Some(s.to_lowercase());
    }
    let phone_like = s.chars().all(|c| c.is_ascii_digit() || " +-().".contains(c));
    if phone_like && let Some(p) = normalize_phone(s) {
        return Some(p);
    }
    Some(s.to_lowercase())
}

/// E.164 from what people type. Digits only; a leading `+` keeps the country
/// code; ten digits are read as North American; eleven digits starting with
/// 1 likewise. Anything else is kept as `+` plus its digits, which is
/// exact-match stable even when not canonical.
pub fn normalize_phone(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return None;
    }
    let plus = s.trim_start().starts_with('+');
    Some(match (plus, digits.len()) {
        (false, 10) => format!("+1{digits}"),
        (false, 11) if digits.starts_with('1') => format!("+{digits}"),
        _ => format!("+{digits}"),
    })
}

/// Why a call asks the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskCase {
    Money { cents: i64, limit_cents: Option<i64> },
    /// A standing allow's call would take the whole workforce past what the
    /// company may spend unattended in a day (the constitution's
    /// company-wide figures). Only the owner's answer runs it; the company's
    /// figures change in the company layer, never by an answer.
    CompanyMoney {
        cents: i64,
        limit_cents: Option<i64>,
    },
    NewCounterparty {
        who: String,
    },
    Irreversible {
        what: String,
    },
    OutsideJob {
        capability: String,
    },
    UntrustedInput {
        source: String,
    },
    AskRule {
        rule_id: String,
    },
    AskMode,
    /// The call gives an employee more room; only the owner does that.
    Widens,
    /// An employee made by an employee needs more than its creator holds:
    /// the one card at creation, listing the extras.
    CreatedExtras { capabilities: Vec<String> },
}

/// What the check decided for one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Allow { why: Why },
    Deny { reason: String, why: Why },
    Ask { case: AskCase },
}

/// The folders file work is fenced to: the folder allow rules of the
/// employee scope when it has any, else the company's, joined by the run's
/// own folders. Empty = the job has no folder fence.
pub fn folders_of(rules: &[Rule], run_folders: &[PathBuf]) -> Vec<PathBuf> {
    let folder_rules = |employee: bool| -> Vec<PathBuf> {
        rules
            .iter()
            .filter(|r| r.effect == Effect::Allow && matches!(r.scope, Scope::Employee(_)) == employee)
            .filter_map(|r| match &r.field {
                Some(RuleField::Folder(p)) => Some(p.clone()),
                _ => None,
            })
            .collect()
    };
    let own = folder_rules(true);
    let mut folders = if own.is_empty() { folder_rules(false) } else { own };
    if !folders.is_empty() {
        for f in run_folders {
            if !folders.contains(f) {
                folders.push(f.clone());
            }
        }
    }
    folders
}

impl Grant {
    /// The folders file work is fenced to (see [`folders_of`]).
    pub fn folders(&self) -> Vec<PathBuf> {
        folders_of(&self.rules, &self.run_folders)
    }

    /// A grant with no rules: the company's and the employee's rules are
    /// loaded into it by the seat.
    pub fn new(agent_id: impl Into<String>, mode: Mode) -> Self {
        Grant {
            agent_id: agent_id.into(),
            mode,
            rules: Vec::new(),
            ceiling: None,
            run_folders: Vec::new(),
            fence: None,
        }
    }
}

/// Who writes or removes a rule. Only the owner widens: an employee can
/// narrow its own or a created employee's rules and never loosen them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writer {
    /// The owner: the Permissions page, an ask answer, consent.
    Owner,
    /// Nebo's one-time conversion of the old settings, and the safe
    /// defaults Nebo writes itself (a newly connected server's tools ask).
    Migration,
    /// A package or pack: its laws and its must-ask operations, locked.
    Package { package: String },
    /// An employee narrowing.
    Employee { agent_id: String },
    /// An employee making another: it hands the new employee part of its
    /// own job, never more (the caller checks each rule against the
    /// creator's grant before writing).
    Creator { creator_id: String },
}

/// Why a rule was not written or removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("that rule is fixed by a law or a package and cannot change")]
    Locked,
    #[error("only the owner can give an employee more room; an employee can only narrow")]
    Widens,
    #[error("no such rule")]
    NotFound,
    #[error("the rules could not be saved: {0}")]
    Store(String),
}

impl MoneyLimit {
    /// Whether `self` allows no more than `outer` on every axis `outer` sets.
    pub fn within(&self, outer: &MoneyLimit) -> bool {
        fn no_looser(inner: Option<i64>, outer: Option<i64>) -> bool {
            match (inner, outer) {
                (_, None) => true,
                (Some(i), Some(o)) => i <= o,
                (None, Some(_)) => false,
            }
        }
        no_looser(self.per_action_cents, outer.per_action_cents)
            && no_looser(self.per_day_cents, outer.per_day_cents)
            && no_looser(self.per_day_count, outer.per_day_count)
            && no_looser(self.per_counterparty_day_cents, outer.per_counterparty_day_cents)
    }
}
