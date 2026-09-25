//! The one-time conversion: an install's old settings, written as they were
//! stored, become rules and modes, and the decisions the new check makes on
//! them equal what the old gates decided.

use std::sync::Arc;

use serde_json::json;
use types::permissions::{Decision, Effect, Mode, RuleField, RuleKey, RuleSource, Scope, Target};

use super::*;

/// A store at the current schema, and a raw connection to write the old
/// shapes into it exactly as an older build stored them.
fn legacy() -> (tempfile::TempDir, Arc<db::Store>, rusqlite::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let store = Arc::new(db::Store::new(&path.to_string_lossy()).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    (dir, store, conn)
}

fn profile(store: &db::Store, conn: &rusqlite::Connection, tool_permissions: &str, approved_commands: &str) {
    let owner = store.ensure_local_user_id().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO user_profiles (user_id, created_at, updated_at) VALUES (?1, 0, 0)",
        rusqlite::params![owner],
    )
    .unwrap();
    conn.execute(
        "UPDATE user_profiles SET tool_permissions = ?1, approved_commands = ?2 WHERE user_id = ?3",
        rusqlite::params![tool_permissions, approved_commands, owner],
    )
    .unwrap();
}

fn employee(conn: &rusqlite::Connection, id: &str, column: &str, json: &str) {
    conn.execute(
        "INSERT INTO entity_config (entity_type, entity_id, created_at, updated_at) VALUES ('agent', ?1, 0, 0)
         ON CONFLICT DO NOTHING",
        rusqlite::params![id],
    )
    .unwrap();
    conn.execute(
        &format!("UPDATE entity_config SET {column} = ?1 WHERE entity_type = 'agent' AND entity_id = ?2"),
        rusqlite::params![json, id],
    )
    .unwrap();
}

fn rules_in(store: &db::Store, scope: Scope) -> Vec<types::permissions::Rule> {
    store.permission_rules_in(&scope).unwrap()
}

fn effect_of(store: &db::Store, scope: Scope, key: RuleKey, field: Option<RuleField>) -> Option<Effect> {
    rules_in(store, scope).into_iter().find(|r| r.key == key && r.field == field).map(|r| r.effect)
}

#[test]
fn toggles_become_capability_rules() {
    let (_d, store, conn) = legacy();
    profile(&store, &conn, r#"{"shell": false, "web": true, "media": false, "chat": true}"#, "[]");
    employee(&conn, "emp", "permissions", r#"{"shell": true, "desktop": false}"#);
    employee(&conn, "other", "permissions", r#"{"web": false}"#);
    migrate_legacy(&store).unwrap().expect("ran");
    let cap = |c: &str| RuleKey::Capability(c.into());
    // Off company-wide and nobody turned it back on: a company deny.
    assert_eq!(effect_of(&store, Scope::Company, cap("media"), None), Some(Effect::Deny));
    // Off company-wide but one employee turned it on: a company deny would
    // bind that employee too, so the "off" stays with each employee that
    // didn't turn it on.
    assert_eq!(effect_of(&store, Scope::Company, cap("shell"), None), None);
    assert_eq!(effect_of(&store, Scope::Employee("other".into()), cap("shell"), None), Some(Effect::Deny));
    assert_eq!(effect_of(&store, Scope::Company, cap("web"), None), Some(Effect::Allow));
    // A capability the toggles never named was on.
    assert_eq!(effect_of(&store, Scope::Company, cap("file"), None), Some(Effect::Allow));
    assert_eq!(effect_of(&store, Scope::Company, cap("chat"), None), None, "chat is no tool capability");
    let emp = Scope::Employee("emp".into());
    assert_eq!(effect_of(&store, emp.clone(), cap("shell"), None), Some(Effect::Allow));
    assert_eq!(effect_of(&store, emp, cap("desktop"), None), Some(Effect::Deny));
    // It runs once.
    assert!(migrate_legacy(&store).unwrap().is_none());
}

#[test]
fn approved_commands_become_prefix_rules() {
    let (_d, store, conn) = legacy();
    profile(&store, &conn, "{}", r#"["git status", "npm test"]"#);
    migrate_legacy(&store).unwrap();
    let mut prefixes: Vec<String> = rules_in(&store, Scope::Company)
        .into_iter()
        .filter(|r| r.key == RuleKey::Tool("run_command".into()) && r.effect == Effect::Allow)
        .filter_map(|r| match r.field {
            Some(RuleField::CommandPrefix(p)) => Some(p),
            _ => None,
        })
        .collect();
    prefixes.sort();
    assert_eq!(prefixes, vec!["git status".to_string(), "npm test".to_string()]);
}

#[test]
fn full_access_becomes_mode() {
    let (_d, store, conn) = legacy();
    conn.execute("INSERT OR IGNORE INTO settings (id) VALUES (1)", []).unwrap();
    conn.execute("UPDATE settings SET full_access = 1 WHERE id = 1", []).unwrap();
    migrate_legacy(&store).unwrap();
    assert_eq!(store.permission_mode(&Scope::Company).unwrap(), Some(Mode::FullAccess));

    let (_d2, fresh, _c) = legacy();
    migrate_legacy(&fresh).unwrap();
    assert_eq!(fresh.permission_mode(&Scope::Company).unwrap(), Some(Mode::Automatic));
}

#[test]
fn fence_becomes_folder_rules() {
    let (_d, store, conn) = legacy();
    profile(&store, &conn, r#"{"file": false}"#, "[]");
    employee(&conn, "fenced", "allowed_paths", r#"["/work/a", "/work/b"]"#);
    migrate_legacy(&store).unwrap();
    let emp = Scope::Employee("fenced".into());
    let mut folders: Vec<std::path::PathBuf> = rules_in(&store, emp.clone())
        .into_iter()
        .filter_map(|r| match r.field {
            Some(RuleField::Folder(p)) => Some(p),
            _ => None,
        })
        .collect();
    folders.sort();
    assert_eq!(folders, vec![std::path::PathBuf::from("/work/a"), "/work/b".into()]);
    // The fence never granted file work: with File off it stays off above the folders.
    assert_eq!(effect_of(&store, emp, RuleKey::Capability("file".into()), None), Some(Effect::Deny));
}

#[test]
fn resource_grants_become_deny_rules() {
    let (_d, store, conn) = legacy();
    employee(&conn, "emp", "resource_grants", r#"{"browser": "deny", "screen": "inherit"}"#);
    migrate_legacy(&store).unwrap();
    let emp = Scope::Employee("emp".into());
    assert_eq!(effect_of(&store, emp.clone(), RuleKey::Tool("browser_*".into()), None), Some(Effect::Deny));
    assert_eq!(effect_of(&store, emp, RuleKey::Tool("desktop_click".into()), None), None, "inherit is no rule");
}

#[test]
fn gated_ops_map_per_the_five_cases() {
    let (_d, store, conn) = legacy();
    employee(
        &conn,
        "ap",
        "operation_policy",
        &json!({
            "default": "approval",
            "operations": {
                "ledger.invoice.update": "approval",
                "social.post.publish": "approval",
                "ledger.payment.apply": "approval",
                "mail.message.send": "always",
                "esign.document.send": "blocked",
                "ledger.bill.create": { "access": "blocked", "source": "law:Payments", "locked": true },
                "ledger.invoice.void": { "access": "approval", "source": "seat" },
                "crm.record.merge": { "access": "always", "source": "pack:acme" }
            }
        })
        .to_string(),
    );
    migrate_legacy(&store).unwrap();
    let emp = Scope::Employee("ap".into());
    let rule = |op: &str| {
        rules_in(&store, emp.clone()).into_iter().find(|r| r.key == RuleKey::Operation(op.into())).unwrap_or_else(|| panic!("{op}"))
    };
    // "Ask first" on an operation Automatic mode doesn't surface runs in the job.
    assert_eq!(rule("ledger.invoice.update").effect, Effect::Allow);
    // Publishing and money stay asks.
    assert_eq!(rule("social.post.publish").effect, Effect::Ask);
    assert_eq!(rule("ledger.payment.apply").effect, Effect::Ask);
    assert_eq!(rule("mail.message.send").effect, Effect::Allow);
    assert_eq!(rule("esign.document.send").effect, Effect::Deny);
    let law = rule("ledger.bill.create");
    assert_eq!((law.effect, law.locked, law.source), (Effect::Deny, true, RuleSource::Law { pack: "Payments".into() }));
    // A package's declaration asks, and the owner can still rule on it.
    let declared = rule("ledger.invoice.void");
    assert_eq!((declared.effect, declared.locked), (Effect::Ask, false));
    assert!(matches!(declared.source, RuleSource::Package { .. }));
    // A declaration never grants itself anything.
    assert_eq!(rule("crm.record.merge").effect, Effect::Ask);
    // Money that nobody configured asks, company-wide.
    assert_eq!(
        effect_of(&store, Scope::Company, RuleKey::Operation("ledger.billpayment.create".into()), None),
        Some(Effect::Ask)
    );
}

#[test]
fn bounds_become_money_limits() {
    let (_d, store, conn) = legacy();
    employee(
        &conn,
        "bk",
        "operation_policy",
        &json!({ "operations": { "ledger.billpayment.create": {
            "access": "always", "source": "general_manager",
            "bounds": { "max_amount_cents": 250000, "per_day_count": 20, "per_counterparty_day_cents": 500000 }
        } } })
        .to_string(),
    );
    migrate_legacy(&store).unwrap();
    let grant = rules_in(&store, Scope::Employee("bk".into()))
        .into_iter()
        .find(|r| r.key == RuleKey::Operation("ledger.billpayment.create".into()))
        .unwrap();
    assert_eq!(grant.effect, Effect::Allow);
    assert_eq!(
        grant.money,
        Some(types::permissions::MoneyLimit {
            per_action_cents: Some(250_000),
            per_day_count: Some(20),
            per_counterparty_day_cents: Some(500_000),
            per_day_cents: None,
        })
    );
}

#[test]
fn mcp_tri_state_becomes_rules() {
    let (_d, store, conn) = legacy();
    conn.execute(
        "INSERT INTO mcp_integrations (id, name, server_type, auth_type, tool_permissions, created_at, updated_at)
         VALUES ('m1', 'Acme CRM', 'acme', 'none', ?1, 0, 0)",
        rusqlite::params![r#"{"default": "allow", "tools": {"Delete Record": "deny"}, "known": ["search", "Delete Record"]}"#],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO mcp_integrations (id, name, server_type, auth_type, created_at, updated_at)
         VALUES ('m2', 'Other', 'other', 'none', 0, 0)",
        [],
    )
    .unwrap();
    migrate_legacy(&store).unwrap();
    let key = |k: &str| RuleKey::Tool(k.into());
    assert_eq!(effect_of(&store, Scope::Company, key("mcp__acme_crm__*"), None), Some(Effect::Allow));
    assert_eq!(effect_of(&store, Scope::Company, key("mcp__acme_crm__delete_record"), None), Some(Effect::Deny));
    assert_eq!(effect_of(&store, Scope::Company, key("mcp__other__*"), None), Some(Effect::Ask), "no map: everything asked");
}

/// The table: calls × seats, decided by the old gates (as they were
/// written) and by the new check on the converted rules.
#[test]
fn migrated_decisions_equal_todays() {
    let (_d, store, conn) = legacy();
    profile(&store, &conn, r#"{"shell": false, "web": true}"#, r#"["git status"]"#);
    employee(&conn, "dev", "permissions", r#"{"shell": true}"#);
    employee(&conn, "ap", "operation_policy", r#"{"operations": {"esign.document.send": "blocked", "ledger.payment.apply": "always"}}"#);
    employee(&conn, "nobrowser", "resource_grants", r#"{"browser": "deny"}"#);
    migrate_legacy(&store).unwrap();

    let target = |key: &str, capability: Option<&str>, operation: Option<&str>| Target {
        tool: "t".into(),
        key: key.into(),
        operation: operation.map(str::to_string),
        capability: capability.map(str::to_string),
        field: None,
        subject: None,
        read_only: false,
        effects: types::permissions::CallEffects::unknown(),
    };
    let decide_for = |agent: &str, origin: tools::Origin, t: &Target| {
        let grant = crate::harness::permissions::resolve_grant(&store, agent, None);
        let ctx = tools::ToolContext { origin, ..Default::default() };
        let input = json!({});
        let cx = crate::harness::permissions::CheckCx { ctx: &ctx, input: &input, grant: &grant, store: &store };
        match crate::harness::permissions::decide(&cx, t) {
            Decision::Allow { .. } => "run",
            Decision::Ask { .. } => "ask",
            Decision::Deny { .. } => "refuse",
        }
    };
    use tools::Origin::{Comm, User, Workflow};
    // (seat, origin, call, what the old gates did unattended or in chat)
    let table: &[(&str, tools::Origin, Target, &str)] = &[
        // Shell off company-wide: refused (unattended, the old registry
        // block) for every employee that left it off; the employee who has
        // it on runs it. A company deny would bind that employee too, so
        // the company leaves shell out of the job: the main seat, which has
        // no employee rules, is asked instead of refused.
        ("ap", Workflow, target("run_command", Some("shell"), None), "refuse"),
        ("dev", Workflow, target("run_command", Some("shell"), None), "run"),
        ("", Workflow, target("run_command", Some("shell"), None), "ask"),
        // Web on: runs.
        ("", User, target("fetch_url", Some("web"), None), "run"),
        // A capability no toggle named was on.
        ("", User, target("write_file", Some("file"), None), "run"),
        // The browser denied for one employee only.
        ("nobrowser", User, target("browser_open", Some("web"), None), "refuse"),
        ("dev", User, target("browser_open", Some("web"), None), "run"),
        // Blocked operation, from every origin.
        ("ap", User, target("plugin__esign", None, Some("esign.document.send")), "refuse"),
        ("ap", Comm, target("plugin__esign", None, Some("esign.document.send")), "refuse"),
        // The owner's Always on a money operation: runs for the owner's own
        // runs, asks when another program's words are in the run.
        ("ap", Workflow, target("plugin__ledger", None, Some("ledger.payment.apply")), "run"),
        ("ap", Comm, target("plugin__ledger", None, Some("ledger.payment.apply")), "ask"),
        // A seat nobody configured: money asks, ordinary gated work runs,
        // untrusted words ask for it.
        ("", Workflow, target("plugin__ledger", None, Some("ledger.billpayment.create")), "ask"),
        ("", Workflow, target("plugin__ledger", None, Some("ledger.invoice.update")), "run"),
        ("", Comm, target("plugin__ledger", None, Some("ledger.invoice.update")), "ask"),
        // An ungated read: runs anywhere.
        ("", Comm, target("plugin__ledger", None, Some("ledger.balance.get")), "run"),
    ];
    for (seat, origin, t, today) in table {
        assert_eq!(decide_for(seat, *origin, t), *today, "{seat:?} {origin:?} {}", t.key);
    }
}

/// The interfaces an employee binds become its job: its operations ran
/// before the upgrade and still run; an employee that binds none is asked
/// (outside its job), and an employee's own "off" on the interface stands.
#[test]
fn bound_interfaces_become_the_job() {
    let (_d, store, conn) = legacy();
    let hire = |id: &str, config: &str| {
        store.create_agent(id, None, id, "", "", config, None, None).unwrap();
    };
    hire("clerk", r#"{"requires": {"interfaces": ["ledger", "not-a-catalog-term"]}}"#);
    hire("other", "{}");
    hire("off", r#"{"requires": {"interfaces": ["ledger"]}}"#);
    employee(&conn, "off", "permissions", r#"{"ledger": false}"#);
    migrate_legacy(&store).unwrap().expect("ran");

    let ledger = RuleKey::Capability("ledger".into());
    assert_eq!(effect_of(&store, Scope::Employee("clerk".into()), ledger.clone(), None), Some(Effect::Allow));
    assert!(
        rules_in(&store, Scope::Employee("clerk".into()))
            .iter()
            .all(|r| r.key != RuleKey::Capability("not-a-catalog-term".into())),
        "only the catalog's terms are capabilities"
    );
    assert_eq!(effect_of(&store, Scope::Employee("other".into()), ledger.clone(), None), None);
    assert_eq!(effect_of(&store, Scope::Employee("off".into()), ledger, None), Some(Effect::Deny));

    let t = Target {
        tool: "ledger_invoice_update".into(),
        key: "ledger.invoice.update".into(),
        operation: Some("ledger.invoice.update".into()),
        capability: Some("ledger".into()),
        field: None,
        subject: None,
        read_only: false,
        effects: types::permissions::CallEffects::unknown(),
    };
    let decide_for = |agent: &str| {
        let grant = crate::harness::permissions::resolve_grant(&store, agent, None);
        let ctx = tools::ToolContext { origin: tools::Origin::Workflow, ..Default::default() };
        let input = json!({});
        let cx = crate::harness::permissions::CheckCx { ctx: &ctx, input: &input, grant: &grant, store: &store };
        crate::harness::permissions::decide(&cx, &t)
    };
    assert!(matches!(decide_for("clerk"), Decision::Allow { .. }), "{:?}", decide_for("clerk"));
    assert!(matches!(decide_for("other"), Decision::Ask { .. }), "{:?}", decide_for("other"));
    assert!(matches!(decide_for("off"), Decision::Deny { .. }), "{:?}", decide_for("off"));
}

/// Nothing outside the conversion reads the old permission shapes: the
/// capability toggles, saved commands, Full Access, per-employee grants,
/// fence and policy, and the MCP tool map.
#[test]
fn nothing_reads_the_old_columns() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let forbidden = [
        ".tool_permissions",
        ".approved_commands",
        ".operation_policy",
        "full_access == 1",
        "get_mcp_tool_permissions",
        "resource_grants.as_deref()",
        "allowed_paths.as_deref()",
        "c.permissions.as_deref()",
        "e.permissions.as_deref()",
    ];
    let mut hits = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }
    for krate in ["agent", "tools", "server", "workflow"] {
        let mut files = Vec::new();
        walk(&root.join(krate).join("src"), &mut files);
        for f in files {
            let path = f.to_string_lossy().into_owned();
            if path.contains("permissions/migrate") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
                for token in forbidden {
                    if line.contains(token) {
                        hits.push(format!("{path}: {token}"));
                    }
                }
            }
        }
    }
    assert!(hits.is_empty(), "the old permission shapes are read outside the conversion:\n{}", hits.join("\n"));
}
