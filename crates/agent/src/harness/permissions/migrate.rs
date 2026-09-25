//! The one-time conversion of the old permission settings into rules and
//! modes (Turn-Controller-Technical-Design §2.12.10). It runs once per
//! install, at the first start of a build that carries it, before the
//! server serves; afterwards nothing reads the old shapes.
//!
//! | Old shape | Becomes |
//! |---|---|
//! | Capability toggles (company `user_profiles.tool_permissions`, employee `entity_config.permissions`) | allow / deny rules on the capability (the job); a company "off" that some employee turned on becomes a deny for each employee that didn't, since a company deny now binds every employee |
//! | Screen and browser grants (`entity_config.resource_grants`) | deny rules on the screen and browser keys |
//! | Path fence (`entity_config.allowed_paths`) | folder rules |
//! | Saved "always allow" commands (`user_profiles.approved_commands`) | allow rules on `run_command` with a command prefix |
//! | Full Access (`settings.full_access`) | the company's mode |
//! | Operation policy (`entity_config.operation_policy`: laws, package ceilings, standing grants, the owner's settings) | deny (laws locked), ask (package ceilings; money and publishing), allow (with money limits for standing grants) |
//! | Operations nobody configured that move money | company ask rules |
//! | Operations the company reserves to the owner (`company_policy.reserved`) | locked company ask rules |
//! | MCP tool permissions (`mcp_integrations.tool_permissions`) | rules on the server's proxy tool names |
//! | Interfaces an employee binds (`agents.frontmatter` `requires.interfaces`) | allow rules on each interface (the job) |

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use types::permissions::{
    Effect, Mode, MoneyLimit, Rule, RuleField, RuleKey, RuleSource, Scope, Writer,
};

/// The name the conversion is recorded under.
pub const MIGRATION: &str = "legacy_settings_v1";

/// What the conversion wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MigrationReport {
    pub rules: usize,
    pub company_mode: Option<String>,
    /// Old settings that could not be read, by where they were.
    pub unreadable: Vec<String>,
}

/// Keys of the screen: what a "screen: deny" grant kept an employee from.
const SCREEN_KEYS: &[&str] = &[
    "desktop_click",
    "desktop_move_mouse",
    "desktop_key",
    "desktop_type",
    "desktop_scroll",
    "desktop_drag",
    "desktop_paste",
    "window*",
    "ui*",
    "menu*",
    "dialog*",
    "space*",
    "shortcut*",
];

/// Convert the old settings once. Returns `None` when it already ran.
pub fn migrate_legacy(store: &db::Store) -> Result<Option<MigrationReport>, types::NeboError> {
    if store.permission_migration_done(MIGRATION)? {
        return Ok(None);
    }
    let mut w = Writes { store, report: MigrationReport::default() };

    // Company capability toggles and saved commands.
    let profile = store.get_user_profile()?;
    let company_toggles: HashMap<String, bool> = profile
        .as_ref()
        .and_then(|p| p.tool_permissions.as_deref())
        .and_then(|j| w.parse("user_profiles.tool_permissions", j))
        .unwrap_or_default();
    // Each employee's own toggles, read once.
    let configs = store.list_entity_configs("agent")?;
    let employee_toggles: Vec<HashMap<String, bool>> = configs
        .iter()
        .map(|ec| {
            ec.permissions
                .as_deref()
                .and_then(|j| w.parse(&format!("entity_config.permissions[{}]", ec.entity_id), j))
                .unwrap_or_default()
        })
        .collect();
    // A capability no toggle names was on: the old gate only refused an
    // explicit `false`. A company "off" was a default an employee could
    // turn back on; a company deny now binds every employee, so where one
    // did, the "off" stays with each employee that didn't, and the company
    // leaves it out of the job (asked, not refused, for anyone new).
    let company_cap = |w: &mut Writes<'_>, cap: &str, on: bool| {
        let turned_on: Vec<bool> = employee_toggles.iter().map(|t| t.get(cap) == Some(&true)).collect();
        if on || !turned_on.contains(&true) {
            w.rule(Scope::Company, RuleKey::Capability(cap.into()), None, if on { Effect::Allow } else { Effect::Deny }, None, "user_profiles.tool_permissions");
            return;
        }
        for (ec, _) in configs.iter().zip(&turned_on).filter(|(_, on)| !**on) {
            w.rule(Scope::Employee(ec.entity_id.clone()), RuleKey::Capability(cap.into()), None, Effect::Deny, None, "user_profiles.tool_permissions");
        }
    };
    for cap in tools::capabilities::CAPABILITIES.iter().map(|c| c.key).filter(|k| *k != "chat") {
        company_cap(&mut w, cap, company_toggles.get(cap).copied().unwrap_or(true));
    }
    for (cap, on) in company_toggles.iter().filter(|(k, _)| is_extra_capability(k)) {
        company_cap(&mut w, cap, *on);
    }
    let commands: Vec<String> = profile
        .as_ref()
        .and_then(|p| p.approved_commands.as_deref())
        .and_then(|j| w.parse("user_profiles.approved_commands", j))
        .unwrap_or_default();
    for prefix in commands {
        w.rule(
            Scope::Company,
            RuleKey::Tool("run_command".into()),
            Some(RuleField::CommandPrefix(prefix)),
            Effect::Allow,
            None,
            "user_profiles.approved_commands",
        );
    }

    // Full Access is the company's mode; every other install runs Automatic.
    let full_access = store.get_settings()?.is_some_and(|s| s.full_access == 1);
    let mode = if full_access { Mode::FullAccess } else { Mode::Automatic };
    store.set_permission_mode(&Scope::Company, mode)?;
    w.report.company_mode = Some(mode.as_str().to_string());

    // Operations that move money or form contracts asked for an employee
    // nobody had configured; they still do.
    for op in tools::interface_catalog::gated_operations()
        .iter()
        .filter(|op| tools::interface_catalog::is_critical(op))
    {
        w.rule(Scope::Company, RuleKey::Operation((*op).to_string()), None, Effect::Ask, None, "operation_policy.critical");
    }
    // Operations the company reserves to the owner's own hand.
    if let Some(json) = store.get_company_policy()? {
        let company = tools::policy::CompanyPolicy::from_json(Some(&json));
        for op in &company.reserved {
            w.packaged(Scope::Company, RuleKey::Operation(op.clone()), Effect::Ask, RuleSource::Law { pack: "company".into() }, true);
        }
    }

    // Each employee's own settings.
    for (ec, employee_toggles) in configs.iter().zip(&employee_toggles) {
        let scope = Scope::Employee(ec.entity_id.clone());
        for (cap, on) in employee_toggles {
            if cap == "chat" {
                continue;
            }
            w.rule(scope.clone(), RuleKey::Capability(cap.clone()), None, if *on { Effect::Allow } else { Effect::Deny }, None, "entity_config.permissions");
        }
        let grants: HashMap<String, String> = ec
            .resource_grants
            .as_deref()
            .and_then(|j| w.parse(&format!("entity_config.resource_grants[{}]", ec.entity_id), j))
            .unwrap_or_default();
        if grants.get("screen").is_some_and(|g| g == "deny") {
            for key in SCREEN_KEYS {
                w.rule(scope.clone(), RuleKey::Tool((*key).to_string()), None, Effect::Deny, None, "entity_config.resource_grants");
            }
        }
        if grants.get("browser").is_some_and(|g| g == "deny") {
            w.rule(scope.clone(), RuleKey::Tool("browser_*".into()), None, Effect::Deny, None, "entity_config.resource_grants");
        }
        let folders: Vec<String> = ec
            .allowed_paths
            .as_deref()
            .and_then(|j| w.parse(&format!("entity_config.allowed_paths[{}]", ec.entity_id), j))
            .unwrap_or_default();
        if !folders.is_empty() {
            // The fence restricted file work; it never granted it. Where
            // File was off the employee keeps it off, above its folders.
            let file_on = employee_toggles
                .get("file")
                .or_else(|| company_toggles.get("file"))
                .copied()
                .unwrap_or(true);
            if !file_on {
                w.rule(scope.clone(), RuleKey::Capability("file".into()), None, Effect::Deny, None, "entity_config.allowed_paths");
            }
            for folder in folders {
                w.rule(
                    scope.clone(),
                    RuleKey::Capability("file".into()),
                    Some(RuleField::Folder(folder.into())),
                    Effect::Allow,
                    None,
                    "entity_config.allowed_paths",
                );
            }
        }
        if let Some(json) = ec.operation_policy.as_deref() {
            match serde_json::from_str::<LegacyPolicy>(json) {
                Ok(policy) => w.operations(&scope, &policy),
                Err(_) => w.report.unreadable.push(format!("entity_config.operation_policy[{}]", ec.entity_id)),
            }
        }
    }

    // Each employee's bound interfaces: their operations ran before there
    // was a job to be inside, so the interfaces it declares are its job. An
    // employee's own rule on the capability stands.
    for agent in store.list_agents(-1, 0)? {
        let json = if agent.frontmatter.trim().is_empty() { "{}" } else { agent.frontmatter.as_str() };
        let Ok(config) = napp::agent::parse_agent_config(json) else {
            w.report.unreadable.push(format!("agents.frontmatter[{}]", agent.id));
            continue;
        };
        let scope = Scope::Employee(agent.id.clone());
        let own = store.permission_rules_in(&scope)?;
        for interface in config
            .requires
            .interfaces
            .iter()
            .filter(|i| tools::interface_catalog::capabilities().contains(&i.as_str()))
        {
            let key = RuleKey::Capability(interface.clone());
            if !own.iter().any(|r| r.key == key && r.field.is_none()) {
                w.rule(scope.clone(), key, None, Effect::Allow, None, "agents.requires.interfaces");
            }
        }
    }

    // MCP servers: the server's default on all its tools, and each override.
    for integration in store.list_mcp_integrations()? {
        if integration.auth_type == "neboai" {
            continue; // Company Memory is governed on the KB page.
        }
        let prefix = mcp::bridge::server_slug(&mcp::bridge::tool_name_prefix(&integration.name));
        let perms: LegacyMcp = store
            .get_mcp_tool_permissions(&integration.id)?
            .as_deref()
            .and_then(|j| w.parse(&format!("mcp_integrations.tool_permissions[{}]", integration.id), j))
            .unwrap_or_default();
        let effect = |access: &str| match access {
            "allow" => Effect::Allow,
            "deny" => Effect::Deny,
            _ => Effect::Ask,
        };
        w.rule(Scope::Company, RuleKey::Tool(format!("mcp__{prefix}__*")), None, effect(&perms.default), None, "mcp_integrations.tool_permissions");
        for (tool, access) in &perms.tools {
            w.rule(
                Scope::Company,
                RuleKey::Tool(mcp::bridge::make_tool_name(&prefix, tool)),
                None,
                effect(access),
                None,
                "mcp_integrations.tool_permissions",
            );
        }
    }

    let report = w.report;
    store.record_permission_migration(MIGRATION, &serde_json::to_string(&report).unwrap_or_default())?;
    tracing::info!(rules = report.rules, mode = ?report.company_mode, unreadable = report.unreadable.len(), "permissions: old settings converted to rules");
    Ok(Some(report))
}

/// A toggle key that is not one of the listed capabilities (an older
/// vocabulary) still becomes a rule on that capability.
fn is_extra_capability(key: &str) -> bool {
    key != "chat" && !tools::capabilities::CAPABILITIES.iter().any(|c| c.key == key)
}

/// Whether an operation is one Automatic mode surfaces (money, publishing),
/// so an "ask first" setting on it stays an ask.
fn surfaced(op: &str) -> bool {
    tools::interface_catalog::is_critical(op) || op.ends_with(".publish")
}

/// The old per-employee operation policy, as stored.
#[derive(Deserialize, Default)]
struct LegacyPolicy {
    #[serde(default)]
    operations: HashMap<String, LegacyRule>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LegacyRule {
    Bare(String),
    Object(LegacyRuleObject),
}

#[derive(Deserialize)]
struct LegacyRuleObject {
    access: String,
    #[serde(default)]
    bounds: Option<LegacyBounds>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    locked: bool,
}

#[derive(Deserialize)]
struct LegacyBounds {
    #[serde(default)]
    max_amount_cents: Option<i64>,
    #[serde(default)]
    per_day_cents: Option<i64>,
    #[serde(default)]
    per_day_count: Option<i64>,
    #[serde(default)]
    per_counterparty_day_cents: Option<i64>,
}

#[derive(Deserialize)]
struct LegacyMcp {
    #[serde(default = "ask")]
    default: String,
    #[serde(default)]
    tools: HashMap<String, String>,
}

impl Default for LegacyMcp {
    fn default() -> Self {
        LegacyMcp { default: ask(), tools: HashMap::new() }
    }
}

fn ask() -> String {
    "ask".to_string()
}

struct Writes<'a> {
    store: &'a db::Store,
    report: MigrationReport,
}

impl Writes<'_> {
    fn parse<T: serde::de::DeserializeOwned>(&mut self, place: &str, json: &str) -> Option<T> {
        if json.trim().is_empty() {
            return None;
        }
        match serde_json::from_str(json) {
            Ok(v) => Some(v),
            Err(_) => {
                self.report.unreadable.push(place.to_string());
                None
            }
        }
    }

    fn put(&mut self, rule: Rule, by: &Writer) {
        match self.store.write_permission_rule(&rule, by) {
            Ok(_) => self.report.rules += 1,
            Err(e) => self.report.unreadable.push(format!("{}: {e}", rule.key.value())),
        }
    }

    fn rule(&mut self, scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect, money: Option<MoneyLimit>, from: &str) {
        self.put(
            Rule {
                id: uuid::Uuid::new_v4().to_string(),
                scope,
                key,
                field,
                effect,
                money,
                source: RuleSource::Migrated { from: from.to_string() },
                locked: false,
                created_at: chrono::Utc::now().timestamp(),
            },
            &Writer::Migration,
        );
    }

    /// A rule a law or a package wrote: laws are locked; a package's
    /// must-ask is not, so the owner can still rule on it.
    fn packaged(&mut self, scope: Scope, key: RuleKey, effect: Effect, source: RuleSource, locked: bool) {
        let package = match &source {
            RuleSource::Law { pack } => pack.clone(),
            RuleSource::Package { package } => package.clone(),
            _ => String::new(),
        };
        self.put(
            Rule {
                id: uuid::Uuid::new_v4().to_string(),
                scope,
                key,
                field: None,
                effect,
                money: None,
                source,
                locked,
                created_at: chrono::Utc::now().timestamp(),
            },
            &Writer::Package { package },
        );
    }

    fn operations(&mut self, scope: &Scope, policy: &LegacyPolicy) {
        const FROM: &str = "entity_config.operation_policy";
        for (op, rule) in &policy.operations {
            let (access, bounds, source, locked) = match rule {
                LegacyRule::Bare(access) => (access.as_str(), None, None, false),
                LegacyRule::Object(o) => (o.access.as_str(), o.bounds.as_ref(), o.source.as_deref(), o.locked),
            };
            let key = RuleKey::Operation(op.clone());
            let law = source.and_then(|s| s.strip_prefix("law:"));
            // A package or pack declares; it never grants itself anything.
            let declared = locked || source.is_some_and(|s| s == "seat" || s.starts_with("pack:") || s.starts_with("law:"));
            match access {
                "blocked" => match law {
                    Some(pack) => self.packaged(scope.clone(), key, Effect::Deny, RuleSource::Law { pack: pack.to_string() }, true),
                    None if locked => self.packaged(
                        scope.clone(),
                        key,
                        Effect::Deny,
                        RuleSource::Package { package: source.unwrap_or("seat").to_string() },
                        true,
                    ),
                    None => self.rule(scope.clone(), key, None, Effect::Deny, None, FROM),
                },
                "always" if !declared => {
                    let money = bounds.map(|b| MoneyLimit {
                        per_action_cents: b.max_amount_cents,
                        per_day_cents: b.per_day_cents,
                        per_day_count: b.per_day_count,
                        per_counterparty_day_cents: b.per_counterparty_day_cents,
                    });
                    self.rule(scope.clone(), key, None, Effect::Allow, money, FROM);
                }
                _ if declared => self.packaged(
                    scope.clone(),
                    key,
                    Effect::Ask,
                    RuleSource::Package { package: source.unwrap_or("seat").to_string() },
                    locked,
                ),
                // The owner's "ask first": still an ask where Automatic mode
                // surfaces it (money, publishing); otherwise it runs inside
                // the job.
                _ if surfaced(op) => self.rule(scope.clone(), key, None, Effect::Ask, None, FROM),
                _ => self.rule(scope.clone(), key, None, Effect::Allow, None, FROM),
            }
        }
    }
}

#[cfg(test)]
mod tests;
