//! The permission system's store: rules, modes, asks, the activity record,
//! money spent against limits, and the one-time conversion marker. The rules
//! engine (`agent::harness::permissions`) owns every decision; this module
//! only reads and writes rows.

use rusqlite::{params, OptionalExtension};
use types::permissions::{Effect, MoneyLimit, Mode, Rule, RuleError, RuleField, RuleKey, RuleSource, Scope, Writer};
use types::NeboError;

use crate::Store;

fn db_err(e: impl std::fmt::Display) -> NeboError {
    NeboError::Database(e.to_string())
}

fn scope_parts(scope: &Scope) -> (&'static str, &str) {
    match scope {
        Scope::Company => ("company", ""),
        Scope::Employee(agent_id) => ("employee", agent_id.as_str()),
    }
}

const RULE_COLUMNS: &str =
    "id, scope, agent_id, key_kind, key, field_kind, field, effect, money, source, locked, created_at";

fn row_to_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Rule>> {
    let scope: String = row.get(1)?;
    let agent_id: String = row.get(2)?;
    let key_kind: String = row.get(3)?;
    let key: String = row.get(4)?;
    let field_kind: String = row.get(5)?;
    let field: String = row.get(6)?;
    let effect: String = row.get(7)?;
    let money: Option<String> = row.get(8)?;
    let source: String = row.get(9)?;
    let scope = match scope.as_str() {
        "company" => Scope::Company,
        _ => Scope::Employee(agent_id),
    };
    let (Some(key), Some(effect), Ok(source)) = (
        RuleKey::from_parts(&key_kind, key),
        Effect::parse(&effect),
        serde_json::from_str::<RuleSource>(&source),
    ) else {
        // A row this build can't read decides nothing; it stays for a
        // build that can.
        return Ok(None);
    };
    let field = if field_kind.is_empty() {
        None
    } else {
        RuleField::from_parts(&field_kind, field)
    };
    Ok(Some(Rule {
        id: row.get(0)?,
        scope,
        key,
        field,
        effect,
        money: money.and_then(|m| serde_json::from_str::<MoneyLimit>(&m).ok()),
        source,
        locked: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
    }))
}

/// One decision, as the activity record keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionActivityRow {
    pub agent_id: String,
    pub session_key: String,
    pub door: String,
    pub tool: String,
    pub rule_key: String,
    pub activity: String,
    /// allow | ask | deny
    pub decision: String,
    /// The `Why` (or the ask case) as JSON.
    pub why: String,
    pub ask_id: Option<String>,
    pub created_at: i64,
}

/// A parked call, as `permission_asks` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionAskRow {
    pub id: String,
    pub agent_id: String,
    pub session_key: String,
    pub chat_id: Option<String>,
    pub door: String,
    pub ask_case: String,
    pub sentence: String,
    pub target: String,
    pub call: String,
    pub seat: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Today's spend against one key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PermissionSpend {
    pub count: i64,
    pub cents: i64,
    pub counterparty_cents: i64,
}

impl Store {
    /// The rules that decide for one employee: the company defaults and the
    /// employee's own. An empty `agent_id` (the main assistant, an outside
    /// client) gets the company rules only.
    pub fn permission_rules(&self, agent_id: &str) -> Result<Vec<Rule>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM permission_rules
                 WHERE scope = 'company' OR (scope = 'employee' AND agent_id = ?1 AND ?1 != '')
                 ORDER BY created_at, id"
            ))
            .map_err(db_err)?;
        let rows = stmt.query_map(params![agent_id], row_to_rule).map_err(db_err)?;
        let mut rules = Vec::new();
        for r in rows {
            if let Some(rule) = r.map_err(db_err)? {
                rules.push(rule);
            }
        }
        Ok(rules)
    }

    /// The rules written at exactly one scope.
    pub fn permission_rules_in(&self, scope: &Scope) -> Result<Vec<Rule>, NeboError> {
        let (scope_s, agent_id) = scope_parts(scope);
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM permission_rules
                 WHERE scope = ?1 AND agent_id = ?2 ORDER BY created_at, id"
            ))
            .map_err(db_err)?;
        let rows = stmt.query_map(params![scope_s, agent_id], row_to_rule).map_err(db_err)?;
        let mut rules = Vec::new();
        for r in rows {
            if let Some(rule) = r.map_err(db_err)? {
                rules.push(rule);
            }
        }
        Ok(rules)
    }

    pub fn get_permission_rule(&self, id: &str) -> Result<Option<Rule>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {RULE_COLUMNS} FROM permission_rules WHERE id = ?1"),
            params![id],
            row_to_rule,
        )
        .optional()
        .map(Option::flatten)
        .map_err(db_err)
    }

    /// Write a rule: the ONE writer every door uses (the owner's pages, an
    /// ask answer, consent, packages, the conversion, an employee narrowing).
    /// A rule on the same scope, key and field is replaced (one rule per key
    /// and field per scope); the returned rule carries the stored id.
    ///
    /// A locked rule changes only by its package. A package's unlocked
    /// must-ask never replaces the owner's own rule on the same key; a law
    /// replaces anything. An employee only narrows: it
    /// writes deny and ask, and an allow only to tighten an existing allow's
    /// money.
    pub fn write_permission_rule(&self, rule: &Rule, by: &Writer) -> Result<Rule, RuleError> {
        let store_err = |e: NeboError| RuleError::Store(e.to_string());
        let existing = self
            .permission_rules_in(&rule.scope)
            .map_err(store_err)?
            .into_iter()
            .find(|r| r.key == rule.key && r.field == rule.field);
        match (&existing, by) {
            // A package's must-ask never replaces a rule the owner wrote; a
            // law (locked) ends whatever stood.
            (Some(e), Writer::Package { .. }) if !e.locked && !rule.locked => return Ok(e.clone()),
            (Some(e), Writer::Owner | Writer::Migration | Writer::Employee { .. }) if e.locked => {
                return Err(RuleError::Locked);
            }
            _ => {}
        }
        if let Writer::Employee { .. } = by {
            let narrows = match rule.effect {
                Effect::Deny | Effect::Ask => true,
                Effect::Allow => existing.as_ref().is_some_and(|e| {
                    e.effect == Effect::Allow
                        && match (&rule.money, &e.money) {
                            (Some(new), Some(old)) => new.within(old),
                            (Some(_), None) => true,
                            (None, None) => true,
                            (None, Some(_)) => false,
                        }
                }),
            };
            if !narrows {
                return Err(RuleError::Widens);
            }
        }
        self.put_permission_rule(rule).map_err(store_err)
    }

    /// Remove a rule. A locked rule is removed only by its package; an
    /// employee removes only an allow (removing a deny or an ask widens).
    pub fn remove_permission_rule(&self, id: &str, by: &Writer) -> Result<(), RuleError> {
        let store_err = |e: NeboError| RuleError::Store(e.to_string());
        let rule = self.get_permission_rule(id).map_err(store_err)?.ok_or(RuleError::NotFound)?;
        if rule.locked && !matches!(by, Writer::Package { .. }) {
            return Err(RuleError::Locked);
        }
        if matches!(by, Writer::Employee { .. }) && rule.effect != Effect::Allow {
            return Err(RuleError::Widens);
        }
        self.delete_permission_rule(id).map_err(store_err)
    }

    fn put_permission_rule(&self, rule: &Rule) -> Result<Rule, NeboError> {
        let (scope_s, agent_id) = scope_parts(&rule.scope);
        let (field_kind, field) = match &rule.field {
            Some(f) => (f.kind().to_string(), f.value()),
            None => (String::new(), String::new()),
        };
        let money = rule.money.as_ref().map(|m| serde_json::to_string(m).unwrap_or_default());
        let source = serde_json::to_string(&rule.source).map_err(db_err)?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO permission_rules
               (id, scope, agent_id, key_kind, key, field_kind, field, effect, money, source, locked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(scope, agent_id, key_kind, key, field_kind, field) DO UPDATE SET
               effect = excluded.effect, money = excluded.money, source = excluded.source,
               locked = excluded.locked",
            params![
                rule.id,
                scope_s,
                agent_id,
                rule.key.kind(),
                rule.key.value(),
                field_kind,
                field,
                rule.effect.as_str(),
                money,
                source,
                rule.locked as i64,
                rule.created_at,
            ],
        )
        .map_err(db_err)?;
        let id: String = conn
            .query_row(
                "SELECT id FROM permission_rules
                 WHERE scope = ?1 AND agent_id = ?2 AND key_kind = ?3 AND key = ?4
                   AND field_kind = ?5 AND field = ?6",
                params![scope_s, agent_id, rule.key.kind(), rule.key.value(), field_kind, field],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(Rule { id, ..rule.clone() })
    }

    fn delete_permission_rule(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM permission_rules WHERE id = ?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    /// The mode stored at one scope, if any.
    pub fn permission_mode(&self, scope: &Scope) -> Result<Option<Mode>, NeboError> {
        let (scope_s, agent_id) = scope_parts(scope);
        let conn = self.conn()?;
        let mode: Option<String> = conn
            .query_row(
                "SELECT mode FROM permission_modes WHERE scope = ?1 AND agent_id = ?2",
                params![scope_s, agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(mode.as_deref().and_then(Mode::parse))
    }

    pub fn set_permission_mode(&self, scope: &Scope, mode: Mode) -> Result<(), NeboError> {
        let (scope_s, agent_id) = scope_parts(scope);
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO permission_modes (scope, agent_id, mode, updated_at)
             VALUES (?1, ?2, ?3, unixepoch())
             ON CONFLICT(scope, agent_id) DO UPDATE SET mode = excluded.mode, updated_at = excluded.updated_at",
            params![scope_s, agent_id, mode.as_str()],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn record_permission_activity(&self, row: &PermissionActivityRow) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO permission_activity
               (agent_id, session_key, door, tool, rule_key, activity, decision, why, ask_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                row.agent_id,
                row.session_key,
                row.door,
                row.tool,
                row.rule_key,
                row.activity,
                row.decision,
                row.why,
                row.ask_id,
                row.created_at,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// An employee's recorded decisions, newest first.
    pub fn permission_activity(&self, agent_id: &str, limit: i64) -> Result<Vec<PermissionActivityRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, session_key, door, tool, rule_key, activity, decision, why, ask_id, created_at
                 FROM permission_activity WHERE agent_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![agent_id, limit], |row| {
                Ok(PermissionActivityRow {
                    agent_id: row.get(0)?,
                    session_key: row.get(1)?,
                    door: row.get(2)?,
                    tool: row.get(3)?,
                    rule_key: row.get(4)?,
                    activity: row.get(5)?,
                    decision: row.get(6)?,
                    why: row.get(7)?,
                    ask_id: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn insert_permission_ask(&self, row: &PermissionAskRow) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO permission_asks
               (id, agent_id, session_key, chat_id, door, ask_case, sentence, target, call, seat,
                status, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.id,
                row.agent_id,
                row.session_key,
                row.chat_id,
                row.door,
                row.ask_case,
                row.sentence,
                row.target,
                row.call,
                row.seat,
                row.status,
                row.created_at,
                row.expires_at,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get_permission_ask(&self, id: &str) -> Result<Option<PermissionAskRow>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, agent_id, session_key, chat_id, door, ask_case, sentence, target, call, seat,
                    status, created_at, expires_at
             FROM permission_asks WHERE id = ?1",
            params![id],
            |row| {
                Ok(PermissionAskRow {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    session_key: row.get(2)?,
                    chat_id: row.get(3)?,
                    door: row.get(4)?,
                    ask_case: row.get(5)?,
                    sentence: row.get(6)?,
                    target: row.get(7)?,
                    call: row.get(8)?,
                    seat: row.get(9)?,
                    status: row.get(10)?,
                    created_at: row.get(11)?,
                    expires_at: row.get(12)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
    }

    /// Today's spend for an employee on one key: the key's own totals and,
    /// when named, what went to one counterparty.
    pub fn permission_spend(
        &self,
        agent_id: &str,
        day: &str,
        key: &str,
        counterparty: &str,
    ) -> Result<PermissionSpend, NeboError> {
        let conn = self.conn()?;
        let (count, cents): (i64, i64) = conn
            .query_row(
                "SELECT count, cents FROM permission_spend
                 WHERE agent_id = ?1 AND day = ?2 AND key = ?3 AND counterparty = ''",
                params![agent_id, day, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_err)?
            .unwrap_or((0, 0));
        let counterparty_cents: i64 = if counterparty.is_empty() {
            0
        } else {
            conn.query_row(
                "SELECT cents FROM permission_spend
                 WHERE agent_id = ?1 AND day = ?2 AND key = ?3 AND counterparty = ?4",
                params![agent_id, day, key, counterparty],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?
            .unwrap_or(0)
        };
        Ok(PermissionSpend { count, cents, counterparty_cents })
    }

    /// Count one action and its cents against the day, before it runs, so a
    /// crash between the decision and the call can never under-count.
    pub fn add_permission_spend(
        &self,
        agent_id: &str,
        day: &str,
        key: &str,
        counterparty: &str,
        cents: i64,
    ) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute(
            "INSERT INTO permission_spend (agent_id, day, key, counterparty, cents, count)
             VALUES (?1, ?2, ?3, '', ?4, 1)
             ON CONFLICT(agent_id, day, key, counterparty)
             DO UPDATE SET count = count + 1, cents = cents + excluded.cents",
            params![agent_id, day, key, cents],
        )
        .map_err(db_err)?;
        if !counterparty.is_empty() {
            tx.execute(
                "INSERT INTO permission_spend (agent_id, day, key, counterparty, cents, count)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)
                 ON CONFLICT(agent_id, day, key, counterparty)
                 DO UPDATE SET count = count + 1, cents = cents + excluded.cents",
                params![agent_id, day, key, counterparty, cents],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)
    }

    /// Whether the named one-time conversion has run on this install.
    pub fn permission_migration_done(&self, name: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT 1 FROM permission_migrations WHERE name = ?1",
            params![name],
            |_| Ok(()),
        )
        .optional()
        .map(|r| r.is_some())
        .map_err(db_err)
    }

    pub fn record_permission_migration(&self, name: &str, report: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO permission_migrations (name, report, applied_at)
             VALUES (?1, ?2, unixepoch())",
            params![name, report],
        )
        .map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-permissions-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    fn rule(scope: Scope, key: RuleKey, field: Option<RuleField>, effect: Effect) -> Rule {
        Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope,
            key,
            field,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 1,
        }
    }

    #[test]
    fn an_employee_reads_the_company_rules_and_only_its_own() {
        let (_d, store) = store();
        store
            .write_permission_rule(&rule(Scope::Company, RuleKey::Capability("shell".into()), None, Effect::Allow), &Writer::Owner)
            .unwrap();
        store
            .write_permission_rule(&rule(
                Scope::Employee("a".into()),
                RuleKey::Capability("shell".into()),
                None,
                Effect::Deny,
            ), &Writer::Owner)
            .unwrap();
        store
            .write_permission_rule(&rule(
                Scope::Employee("b".into()),
                RuleKey::Tool("run_command".into()),
                Some(RuleField::CommandPrefix("git status".into())),
                Effect::Allow,
            ), &Writer::Owner)
            .unwrap();
        assert_eq!(store.permission_rules("a").unwrap().len(), 2);
        assert_eq!(store.permission_rules("b").unwrap().len(), 2);
        assert_eq!(store.permission_rules("").unwrap().len(), 1, "no agent: company only");
        let b = store.permission_rules_in(&Scope::Employee("b".into())).unwrap();
        assert_eq!(b[0].field, Some(RuleField::CommandPrefix("git status".into())));
    }

    #[test]
    fn the_same_key_and_field_is_one_rule() {
        let (_d, store) = store();
        let first = store
            .write_permission_rule(&rule(Scope::Company, RuleKey::Capability("web".into()), None, Effect::Allow), &Writer::Owner)
            .unwrap();
        let second = store
            .write_permission_rule(&rule(Scope::Company, RuleKey::Capability("web".into()), None, Effect::Deny), &Writer::Owner)
            .unwrap();
        assert_eq!(first.id, second.id);
        let rules = store.permission_rules("").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].effect, Effect::Deny);
    }

    #[test]
    fn locked_rules_change_only_by_their_package_and_employees_only_narrow() {
        let (_d, store) = store();
        let law = Rule {
            locked: true,
            source: RuleSource::Law { pack: "acme".into() },
            ..rule(Scope::Employee("a".into()), RuleKey::Operation("esign.document.send".into()), None, Effect::Deny)
        };
        let law = store.write_permission_rule(&law, &Writer::Package { package: "acme".into() }).unwrap();
        let loosen = rule(Scope::Employee("a".into()), RuleKey::Operation("esign.document.send".into()), None, Effect::Allow);
        assert_eq!(store.write_permission_rule(&loosen, &Writer::Owner), Err(RuleError::Locked));
        assert_eq!(store.remove_permission_rule(&law.id, &Writer::Owner), Err(RuleError::Locked));

        let employee = Writer::Employee { agent_id: "cos".into() };
        let widen = rule(Scope::Employee("a".into()), RuleKey::Capability("shell".into()), None, Effect::Allow);
        assert_eq!(store.write_permission_rule(&widen, &employee), Err(RuleError::Widens));
        let narrow = rule(Scope::Employee("a".into()), RuleKey::Capability("shell".into()), None, Effect::Deny);
        let narrowed = store.write_permission_rule(&narrow, &employee).unwrap();
        assert_eq!(store.remove_permission_rule(&narrowed.id, &employee), Err(RuleError::Widens));
        assert!(store.remove_permission_rule(&narrowed.id, &Writer::Owner).is_ok());

        // A package's must-ask never replaces the owner's own rule on the same key.
        let owners = store
            .write_permission_rule(&rule(Scope::Company, RuleKey::Operation("mail.message.send".into()), None, Effect::Allow), &Writer::Owner)
            .unwrap();
        let declared = rule(Scope::Company, RuleKey::Operation("mail.message.send".into()), None, Effect::Ask);
        let kept = store.write_permission_rule(&declared, &Writer::Package { package: "p".into() }).unwrap();
        assert_eq!((kept.id, kept.effect), (owners.id.clone(), Effect::Allow));
        // A law ends it.
        let law = Rule { locked: true, ..rule(Scope::Company, RuleKey::Operation("mail.message.send".into()), None, Effect::Deny) };
        let landed = store.write_permission_rule(&law, &Writer::Package { package: "p".into() }).unwrap();
        assert_eq!((landed.id, landed.effect, landed.locked), (owners.id, Effect::Deny, true));
    }

    #[test]
    fn spend_counts_the_key_and_the_counterparty() {
        let (_d, store) = store();
        store.add_permission_spend("a", "2026-09-24", "pay", "v1", 500).unwrap();
        store.add_permission_spend("a", "2026-09-24", "pay", "v2", 700).unwrap();
        let s = store.permission_spend("a", "2026-09-24", "pay", "v1").unwrap();
        assert_eq!(s, PermissionSpend { count: 2, cents: 1200, counterparty_cents: 500 });
        assert_eq!(store.permission_spend("a", "2026-09-25", "pay", "").unwrap(), PermissionSpend::default());
    }

    #[test]
    fn modes_are_stored_per_scope() {
        let (_d, store) = store();
        assert_eq!(store.permission_mode(&Scope::Company).unwrap(), None);
        store.set_permission_mode(&Scope::Company, Mode::FullAccess).unwrap();
        store.set_permission_mode(&Scope::Employee("a".into()), Mode::Plan).unwrap();
        assert_eq!(store.permission_mode(&Scope::Company).unwrap(), Some(Mode::FullAccess));
        assert_eq!(store.permission_mode(&Scope::Employee("a".into())).unwrap(), Some(Mode::Plan));
    }
}
