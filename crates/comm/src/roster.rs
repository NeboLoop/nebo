//! Employee roster reconcile: which of this bot's local employees must be
//! registered on, updated on, or removed from its personal loop.
//!
//! Pure planning only. The executor (nebo-server `codes::reconcile_agents`)
//! gathers the local and remote sets, runs `plan_reconcile`, and performs the
//! API calls. Keeping the diff here means the rules are unit-testable without a
//! server: one planner, one set of rules, for every trigger (connect, employee
//! create/rename/enable/disable/delete).
//!
//! Rules (mirroring the hub's agent identity model, see `handle`):
//! - The PRIMARY agent (`bot_<id8>`, the bot's own "Nebo" identity) is owned by
//!   the gateway and never registered, updated, or deregistered here.
//! - Secondaries are keyed by their bot-scoped slug. Local not remote →
//!   register. Remote not local → deregister. Both present with a different
//!   display name → update (the hub upserts by slug, so an update is a
//!   re-register). A rename changes the slug, so it surfaces as deregister of
//!   the old slug plus register of the new one.
//! - Two local employees that slugify to the same handle cannot both exist on
//!   the loop; the first wins and the rest are reported as collisions.

use std::collections::{HashMap, HashSet};

use crate::api_types::AgentInfo;
use crate::handle::is_primary_handle;

/// A local employee that should exist on the loop, already reduced to what
/// the hub needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEmployee {
    /// Local `agents.id`.
    pub id: String,
    /// Display name as it should appear on the loop.
    pub name: String,
    /// Bot-scoped handle (`handle::secondary_handle`).
    pub slug: String,
    /// Description sent on register; empty means none.
    pub description: String,
}

/// The work a reconcile pass has to do.
#[derive(Debug, Default)]
pub struct ReconcilePlan {
    /// Local employees missing from the loop.
    pub register: Vec<LocalEmployee>,
    /// Local employees present on the loop under a stale name: (local, remote id).
    pub update: Vec<(LocalEmployee, String)>,
    /// Loop secondaries that no longer correspond to a local employee.
    pub deregister: Vec<AgentInfo>,
    /// Local employees already correct on the loop: (local, remote id).
    pub unchanged: Vec<(LocalEmployee, String)>,
    /// Local employees whose slug duplicates an earlier local employee's; not
    /// registered (the loop can hold one agent per slug).
    pub collisions: Vec<LocalEmployee>,
}

impl ReconcilePlan {
    /// True when the loop already matches the local roster.
    pub fn is_noop(&self) -> bool {
        self.register.is_empty() && self.update.is_empty() && self.deregister.is_empty()
    }
}

/// Diff the local roster against the loop's agents for this bot.
pub fn plan_reconcile(local: &[LocalEmployee], remote: &[AgentInfo]) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();

    let remote_by_slug: HashMap<&str, &AgentInfo> = remote
        .iter()
        .filter(|a| !is_primary_handle(&a.slug))
        .map(|a| (a.slug.as_str(), a))
        .collect();

    let mut seen_slugs: HashSet<&str> = HashSet::new();
    for emp in local {
        if !seen_slugs.insert(emp.slug.as_str()) {
            plan.collisions.push(emp.clone());
            continue;
        }
        match remote_by_slug.get(emp.slug.as_str()) {
            None => plan.register.push(emp.clone()),
            Some(r) if r.name != emp.name => plan.update.push((emp.clone(), r.id.clone())),
            Some(r) => plan.unchanged.push((emp.clone(), r.id.clone())),
        }
    }

    for r in remote {
        if is_primary_handle(&r.slug) {
            continue;
        }
        if !seen_slugs.contains(r.slug.as_str()) {
            plan.deregister.push(r.clone());
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: &str, name: &str, slug: &str) -> LocalEmployee {
        LocalEmployee {
            id: id.into(),
            name: name.into(),
            slug: slug.into(),
            description: String::new(),
        }
    }

    fn remote(id: &str, name: &str, slug: &str) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            bot_id: "bfa6275a-0000".into(),
            loop_id: "loop-1".into(),
            name: name.into(),
            slug: slug.into(),
            description: String::new(),
            status: "active".into(),
            bot_name: String::new(),
            bot_slug: String::new(),
        }
    }

    fn ids(v: &[LocalEmployee]) -> Vec<&str> {
        v.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn a_local_employee_missing_from_the_loop_is_registered() {
        let plan = plan_reconcile(&[local("a1", "Sales Advisor", "bot_bfa6275a_sales-advisor")], &[]);
        assert_eq!(ids(&plan.register), vec!["a1"]);
        assert!(plan.update.is_empty() && plan.deregister.is_empty() && plan.unchanged.is_empty());
    }

    #[test]
    fn a_loop_secondary_with_no_local_employee_is_deregistered() {
        let plan = plan_reconcile(&[], &[remote("r1", "Old Hire", "bot_bfa6275a_old-hire")]);
        assert_eq!(plan.deregister.len(), 1);
        assert_eq!(plan.deregister[0].id, "r1");
    }

    #[test]
    fn the_primary_is_never_touched() {
        // Not local (the primary is the local "assistant", excluded by the
        // executor) and present remotely: must NOT be deregistered.
        let plan = plan_reconcile(&[], &[remote("p", "Nebo", "bot_bfa6275a")]);
        assert!(plan.deregister.is_empty(), "primary must never be deregistered");
        assert!(plan.is_noop());
    }

    #[test]
    fn a_matching_employee_is_unchanged_and_carries_the_remote_id() {
        let plan = plan_reconcile(
            &[local("a1", "Sales Advisor", "bot_bfa6275a_sales-advisor")],
            &[remote("r1", "Sales Advisor", "bot_bfa6275a_sales-advisor")],
        );
        assert!(plan.is_noop());
        assert_eq!(plan.unchanged.len(), 1);
        assert_eq!(plan.unchanged[0].1, "r1");
    }

    #[test]
    fn a_stale_display_name_is_an_update_not_a_register() {
        let plan = plan_reconcile(
            &[local("a1", "Sales Advisor", "bot_bfa6275a_sales-advisor")],
            &[remote("r1", "sales-advisor", "bot_bfa6275a_sales-advisor")],
        );
        assert!(plan.register.is_empty());
        assert_eq!(plan.update.len(), 1);
        assert_eq!(plan.update[0].0.id, "a1");
        assert_eq!(plan.update[0].1, "r1");
    }

    #[test]
    fn a_rename_is_deregister_old_slug_plus_register_new_slug() {
        let plan = plan_reconcile(
            &[local("a1", "Front Desk", "bot_bfa6275a_front-desk")],
            &[remote("r1", "Receptionist", "bot_bfa6275a_receptionist")],
        );
        assert_eq!(ids(&plan.register), vec!["a1"]);
        assert_eq!(plan.deregister.len(), 1);
        assert_eq!(plan.deregister[0].slug, "bot_bfa6275a_receptionist");
    }

    #[test]
    fn duplicate_local_slugs_register_once_and_report_the_rest() {
        let plan = plan_reconcile(
            &[
                local("a1", "Receptionist", "bot_bfa6275a_receptionist"),
                local("a2", "Receptionist", "bot_bfa6275a_receptionist"),
            ],
            &[],
        );
        assert_eq!(ids(&plan.register), vec!["a1"]);
        assert_eq!(ids(&plan.collisions), vec!["a2"]);
    }

    #[test]
    fn a_full_pass_classifies_every_agent_exactly_once() {
        let local = [
            local("keep", "Keep", "bot_bfa6275a_keep"),
            local("fix", "Fix Name", "bot_bfa6275a_fix-name"),
            local("new", "New", "bot_bfa6275a_new"),
        ];
        let remote = [
            remote("p", "Nebo", "bot_bfa6275a"),
            remote("rk", "Keep", "bot_bfa6275a_keep"),
            remote("rf", "fix-name", "bot_bfa6275a_fix-name"),
            remote("rg", "Gone", "bot_bfa6275a_gone"),
        ];
        let plan = plan_reconcile(&local, &remote);
        assert_eq!(ids(&plan.register), vec!["new"]);
        assert_eq!(plan.update.len(), 1);
        assert_eq!(plan.update[0].0.id, "fix");
        assert_eq!(plan.unchanged.len(), 1);
        assert_eq!(plan.unchanged[0].0.id, "keep");
        assert_eq!(plan.deregister.len(), 1);
        assert_eq!(plan.deregister[0].id, "rg");
        assert!(plan.collisions.is_empty());
    }
}
