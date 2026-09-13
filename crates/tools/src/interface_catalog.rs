//! Interface operation catalog — the ONE source the runtime's gate reads for
//! which typed interface operations are **gated** (money movement, outbound
//! contact, or an irreversible write) and which are **critical** (money
//! movement, contract formation, the company's own file, or standing authority:
//! an employee-wide full-autonomy default must never silently loosen them).
//!
//! The catalogue ships inside the binary: `interfaces/_catalog.yaml` beside this
//! module is embedded with `include_str!` and parsed once, at first use. Every
//! operation carries `gated: true/false`; the critical ones also carry
//! `critical: true`. The departments repository keeps a copy of the same file at
//! `interfaces/_catalog.yaml` for package authors and its validator — that copy
//! is documentation and must be byte-identical to the bundled one; the
//! `departments_copy_matches_bundled` test fails when it drifts.
//!
//! A catalogue that does not parse is a build defect, not a runtime condition:
//! the `bundled_catalog_parses` test fails on every build, and at runtime the
//! first use panics naming the file. There is no fallback to an empty table,
//! because an empty table gates nothing.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::plugin_tool::port_suffix;

/// Where the bundled catalogue lives in the tree, for the failure message.
const CATALOG_PATH: &str = "crates/tools/src/interfaces/_catalog.yaml";
const CATALOG_YAML: &str = include_str!("interfaces/_catalog.yaml");

/// The per-operation declaration as written in the YAML. Unknown keys are
/// refused so a misspelt `gated` fails the build instead of ungating silently.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpSpec {
    gated: bool,
    #[serde(default)]
    critical: bool,
}

struct Entry {
    gated: bool,
    critical: bool,
}

/// The parsed catalogue. Operations are keyed by their suffix as written,
/// `<capability>.<resource>.<action>`. A seat's port carries a
/// `<department>.<role>.` provenance prefix; lookups normalize to the suffix, so
/// `accounting.ap-specialist.ledger.bill.create` matches `ledger.bill.create`.
struct Catalog {
    entries: HashMap<&'static str, Entry>,
    /// Gated operation suffixes, in catalogue order.
    gated: &'static [&'static str],
}

fn parse(yaml: &str) -> Result<Catalog, String> {
    let capabilities: serde_yaml::Mapping = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
    let mut entries = HashMap::new();
    let mut gated = Vec::new();
    for (capability, operations) in capabilities {
        let capability = capability
            .as_str()
            .ok_or_else(|| format!("capability key {capability:?} is not a string"))?;
        let operations: serde_yaml::Mapping =
            serde_yaml::from_value(operations).map_err(|e| format!("{capability}: {e}"))?;
        for (operation, spec) in operations {
            let operation = operation
                .as_str()
                .ok_or_else(|| format!("{capability}: operation key {operation:?} is not a string"))?;
            if !operation.starts_with(&format!("{capability}.")) {
                return Err(format!("{operation} is listed under {capability} but does not begin with it"));
            }
            let spec: OpSpec = serde_yaml::from_value(spec).map_err(|e| format!("{operation}: {e}"))?;
            if spec.critical && !spec.gated {
                return Err(format!("{operation} is critical but not gated"));
            }
            // The table lives for the whole process; leaking each key once is
            // what makes `gated_operations()` a `&'static` slice.
            let operation: &'static str = Box::leak(operation.to_owned().into_boxed_str());
            if entries
                .insert(operation, Entry { gated: spec.gated, critical: spec.critical })
                .is_some()
            {
                return Err(format!("{operation} is listed twice"));
            }
            if spec.gated {
                gated.push(operation);
            }
        }
    }
    if gated.is_empty() {
        return Err("no operation is gated".to_string());
    }
    Ok(Catalog { entries, gated: Box::leak(gated.into_boxed_slice()) })
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        parse(CATALOG_YAML).unwrap_or_else(|e| panic!("{CATALOG_PATH} does not parse: {e}"))
    })
}

fn entry(operation: &str) -> Option<&'static Entry> {
    catalog().entries.get(port_suffix(operation).as_str())
}

/// Whether the operation (bare op or fully-qualified port) is gated.
pub fn is_gated(operation: &str) -> bool {
    entry(operation).is_some_and(|e| e.gated)
}

/// Whether the operation is critical (protected from global auto-loosening).
pub fn is_critical(operation: &str) -> bool {
    entry(operation).is_some_and(|e| e.critical)
}

/// All gated operation suffixes (for building the per-employee policy UI list).
pub fn gated_operations() -> &'static [&'static str] {
    catalog().gated
}

/// Capabilities the runtime performs itself: no plugin binding, no seat
/// interface in `agent.json`. Every seat can reach them, so the per-employee
/// Approvals list shows them for every employee instead of filtering on the
/// interfaces the seat binds.
const BUILTIN: &[&str] = &["layers"];

/// Whether this capability is performed by the runtime rather than by a bound
/// plugin interface.
pub fn is_builtin_capability(capability: &str) -> bool {
    BUILTIN.contains(&capability)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gated list as it was compiled into the binary before the catalogue
    /// shipped. Nothing that was protected may become unprotected.
    const PREVIOUSLY_COMPILED_GATED: &[&str] = &[
        "crm.contact.upsert",
        "crm.opportunity.create",
        "crm.opportunity.update",
        "crm.opportunity.status",
        "crm.message.send",
        "mail.message.send",
        "sms.message.send",
        "esign.document.send",
        "helpdesk.ticket.update",
        "helpdesk.ticket.reply",
        "kb.article.create",
        "kb.article.update",
        "store.order.update",
        "store.inventory.update",
        "store.product.update",
        "store.fulfillment.create",
        "store.return.create",
        "store.po.create",
        "ats.interview.schedule",
        "cms.post.create",
        "cms.post.update",
        "social.post.schedule",
        "social.post.publish",
        "email-marketing.campaign.send",
        "reviews.review.respond",
        "ads.campaign.update",
        "tickets.issue.create",
        "tickets.issue.update",
        "ledger.bill.create",
        "ledger.billpayment.create",
        "ledger.invoice.create",
        "ledger.invoice.send",
        "ledger.payment.apply",
        "ledger.creditmemo.create",
        "ledger.journalentry.create",
        "ledger.deposit.record",
        "ledger.expense.record",
        "ledger.document.attach",
        "authority.grant.grant",
        "authority.grant.widen",
        "layers.industry.write",
        "layers.industry.remove",
        "layers.franchise.write",
        "layers.franchise.remove",
        "layers.company.write",
        "layers.company.remove",
    ];

    /// The critical list as it was compiled before the catalogue shipped: the
    /// money-moving, contract-forming, company-file and authority-granting ones.
    const PREVIOUSLY_COMPILED_CRITICAL: &[&str] = &[
        "ledger.billpayment.create",
        "ledger.payment.apply",
        "ledger.creditmemo.create",
        "ledger.invoice.send",
        "esign.document.send",
        "store.po.create",
        "authority.grant.grant",
        "authority.grant.widen",
        "layers.company.write",
        "layers.company.remove",
    ];

    /// Operations the catalogue names with four segments. The runtime addresses
    /// an operation by its last three segments (`port_suffix`) and routes on the
    /// first of those as the capability, so these cannot be reached through a
    /// typed port as written — neither gated nor performed. The catalogue must
    /// rename them to `<capability>.<resource>.<action>`; this list may shrink
    /// as that happens and must never grow.
    const NOT_ADDRESSABLE_BY_PORT: &[&str] = &[
        "ledger.card.limit.set",
        "ledger.card.statement.get",
        "ledger.customer.credit_limit.set",
        "ledger.customer.hold.set",
        "store.inventory.count.create",
        "store.inventory.count.get",
        "store.order.payment.get",
        "hris.benefits.enrollment.list",
        "hris.payroll.calendar.get",
        "hris.payroll.register.get",
        "ats.candidate.stage.set",
        "testing.visual.baseline.set",
        "directory.group.member.add",
        "directory.group.member.remove",
        "network.circuit.contract.get",
        "insurance.certificate.holder.list",
        "property.owner.statement.get",
        "membership.directory.entry.get",
        "membership.renewal.notice.send",
        "sis.application.checklist.get",
        "sis.directory_info.policy.get",
        "sis.record.amendment.request",
        "matters.conflict.index.upsert",
        "ediscovery.hold.acknowledgment.list",
    ];

    /// Runs on every build: a catalogue that does not parse never ships.
    #[test]
    fn bundled_catalog_parses() {
        let parsed = parse(CATALOG_YAML).expect("bundled catalogue parses");
        let capabilities: std::collections::BTreeSet<&str> =
            parsed.entries.keys().map(|op| op.split('.').next().unwrap()).collect();
        assert_eq!(capabilities.len(), 131, "capabilities");
        assert_eq!(parsed.entries.len(), 1403, "operations");
        assert_eq!(parsed.gated.len(), 510, "gated operations");
        assert_eq!(parsed.entries.values().filter(|e| e.critical).count(), 10, "critical operations");
        assert_eq!(gated_operations().len(), 510);
    }

    #[test]
    fn nothing_previously_compiled_is_unprotected() {
        for op in PREVIOUSLY_COMPILED_GATED {
            assert!(is_gated(op), "{op} was gated in the compiled list and must still be");
            assert!(gated_operations().contains(op), "{op} must be listed for the Approvals view");
        }
        for op in PREVIOUSLY_COMPILED_CRITICAL {
            assert!(is_critical(op), "{op} was critical in the compiled list and must still be");
        }
        // Exactly the ten, no more: critical is opted in by name in the YAML.
        let critical: Vec<&str> = gated_operations().iter().copied().filter(|op| is_critical(op)).collect();
        assert_eq!(critical.len(), PREVIOUSLY_COMPILED_CRITICAL.len(), "critical set: {critical:?}");
    }

    /// Gated on paper only until the catalogue shipped: three operations from
    /// three capabilities that the compiled list never named — a bank transfer,
    /// a payroll run, and a regulator filing.
    #[test]
    fn paper_only_gates_now_hold() {
        for op in ["ledger.transfer.create", "hris.payroll.run", "foodsafety.report.file"] {
            assert!(!PREVIOUSLY_COMPILED_GATED.contains(&op), "{op} is meant to be a new gate");
            assert!(is_gated(op), "{op} is gated in the catalogue and must gate at runtime");
            assert!(is_gated(&format!("operations.line-cook.{op}")), "{op} gates through a full port");
            assert!(!is_critical(op));
        }
    }

    /// A capability the catalogue does not know stays "not gated" here; the
    /// policy still gates it once the employee has been told about it (a rule,
    /// a ceiling, the company's reservation) — see `OperationPolicy::decide`.
    #[test]
    fn unknown_operation_is_not_gated() {
        assert!(!is_gated("stranger.thing.do"));
        assert!(!is_critical("stranger.thing.do"));
        assert!(!is_gated("ops.someone.stranger.thing.do"));
        assert!(!gated_operations().contains(&"stranger.thing.do"));
    }

    #[test]
    fn gated_lookup_normalizes_full_port() {
        assert!(is_gated("accounting.ap-specialist.ledger.billpayment.create"));
        assert!(is_gated("ledger.billpayment.create"));
        assert!(!is_gated("ledger.vendor.find"));
        assert!(!is_gated("accounting.ap-specialist.ledger.vendor.find"));
    }

    #[test]
    fn critical_is_subset_of_gated() {
        let parsed = parse(CATALOG_YAML).unwrap();
        for (op, e) in &parsed.entries {
            if e.critical {
                assert!(e.gated, "critical op {op} must also be gated");
                assert!(is_critical(op));
            }
        }
        // A gated-but-not-critical op reads correctly.
        assert!(is_gated("mail.message.send"));
        assert!(!is_critical("mail.message.send"));
    }

    /// The knowledge base plugin's read/write split rides this catalog: its
    /// manifest binds `kb.article.search` (read) and `kb.article.create` /
    /// `kb.article.update` (write), and a seat calls them as `ballast.kb.article.*`.
    /// If a future edit drops the `kb` entries or changes suffix normalization, KB
    /// writes silently stop asking for approval — so pin the exact strings.
    #[test]
    fn kb_read_write_split_is_gated_as_the_plugin_expects() {
        assert!(!is_gated("ballast.kb.article.search"), "search must stay ungated");
        assert!(is_gated("ballast.kb.article.create"), "ingest must be gated");
        assert!(is_gated("ballast.kb.article.update"), "forget must be gated");

        // A KB write is not money movement, so it must not be `critical` — the
        // owner can grant it standing approval; a payment op never can.
        assert!(!is_critical("ballast.kb.article.create"));
    }

    /// The company's own files are a capability like any other, with one
    /// difference that the whole setting rests on: writing or removing the
    /// COMPANY file is critical, so no employee-wide default can hand it to a
    /// seat — the owner grants it per operation. The industry and franchise
    /// files are gated but not critical: a general default may cover them.
    #[test]
    fn the_company_file_is_critical_and_the_trade_files_are_only_gated() {
        for op in ["layers.company.write", "layers.company.remove"] {
            assert!(is_gated(op), "{op} must be gated");
            assert!(is_critical(op), "{op} must be critical");
        }
        for op in [
            "layers.industry.write",
            "layers.industry.remove",
            "layers.franchise.write",
            "layers.franchise.remove",
        ] {
            assert!(is_gated(op), "{op} must be gated");
            assert!(!is_critical(op), "{op} is not money or company law");
        }
        // Reading a layer is never gated.
        assert!(!is_gated("layers.company.read"));
        // The capability is the runtime's own, so the Approvals list shows it
        // for every employee rather than only for seats binding an interface.
        assert!(is_builtin_capability("layers"));
        assert!(!is_builtin_capability("ledger"));
    }

    /// Granting standing authority is the operation that makes every other
    /// unattended operation possible, so it is gated and critical. Taking
    /// authority away is free: an employee may always be made less powerful.
    #[test]
    fn granting_authority_is_critical_and_taking_it_away_is_free() {
        for op in ["authority.grant.grant", "authority.grant.widen"] {
            assert!(is_gated(op), "{op} must be gated");
            assert!(is_critical(op), "{op} must be critical");
        }
        for op in ["authority.grant.narrow", "authority.grant.suspend", "authority.grant.list"] {
            assert!(!is_gated(op), "{op} takes power away or reads: never gated");
        }
    }

    /// Every entry the port scheme can address is a 3-segment suffix equal to
    /// itself; the ones it cannot are exactly the known list, which may only
    /// shrink. None of them is critical.
    #[test]
    fn catalog_entries_are_valid_suffixes() {
        let parsed = parse(CATALOG_YAML).unwrap();
        for (op, e) in &parsed.entries {
            if op.split('.').count() == 3 {
                assert_eq!(&port_suffix(op), op, "{op} must equal its own suffix");
            } else {
                assert!(
                    NOT_ADDRESSABLE_BY_PORT.contains(op),
                    "{op} cannot be addressed by a typed port and is not in the known list"
                );
                assert!(!e.critical, "{op} is critical but unreachable through a port");
            }
        }
    }

    /// The departments repository's copy is documentation of this file and must
    /// not drift. Skipped, with a note, when the sibling checkout is absent.
    #[test]
    fn departments_copy_matches_bundled() {
        let path = "/Users/almatuck/workspaces/nebo/repos/departments/interfaces/_catalog.yaml";
        let Ok(theirs) = std::fs::read_to_string(path) else {
            eprintln!("skipped: {path} is not present on this machine");
            return;
        };
        assert!(
            theirs == CATALOG_YAML,
            "{path} differs from {CATALOG_PATH}; the departments copy is documentation of the bundled one and must be byte-identical"
        );
    }

    /// The failure path names the file and never yields an empty table.
    #[test]
    fn a_broken_catalogue_fails_loudly() {
        assert!(parse("ledger: [not, a, mapping]").is_err());
        assert!(parse("ledger:\n  ledger.bill.create: { gatd: true }").is_err(), "unknown key refused");
        assert!(parse("ledger:\n  ledger.bill.create: { gated: false, critical: true }").is_err());
        assert!(parse("ledger:\n  other.bill.create: { gated: true }").is_err(), "wrong capability refused");
        assert!(parse("ledger:\n  ledger.bill.find: { gated: false }").is_err(), "nothing gated refused");
    }
}
