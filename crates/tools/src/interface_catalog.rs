//! Interface operation catalog — the runtime's source of truth for which typed
//! interface operations are **gated** (money movement, outbound contact, or an
//! irreversible write) and which are **critical** (money movement / contract
//! formation that a global full-autonomy setting must not silently loosen).
//!
//! The authoritative declaration is the per-operation `gated: true/false` flag in
//! `departments/interfaces/_catalog.yaml` + `ledger.yaml` — but that YAML is never
//! shipped into the runtime, so nothing could read it (the three-way disconnect:
//! the interface flag, the plugin `approval` field, and the runtime gate were
//! never wired together). This module mirrors the catalog so the per-operation
//! approval policy can decide against a real signal. **Keep in sync with those
//! YAML files** — the drift test below asserts every entry is a valid suffix.

use crate::plugin_tool::port_suffix;

/// Gated operations, in operation-suffix form `<capability>.<resource>.<action>`.
/// A seat's port carries a `<department>.<role>.` provenance prefix; lookups
/// normalize to the suffix, so `accounting.ap-specialist.ledger.bill.create`
/// matches `ledger.bill.create`.
const GATED: &[&str] = &[
    // crm
    "crm.contact.upsert",
    "crm.opportunity.create",
    "crm.opportunity.update",
    "crm.opportunity.status",
    "crm.message.send",
    // mail / sms / esign
    "mail.message.send",
    "sms.message.send",
    "esign.document.send",
    // helpdesk / kb
    "helpdesk.ticket.update",
    "helpdesk.ticket.reply",
    "kb.article.create",
    "kb.article.update",
    // store (e-commerce)
    "store.order.update",
    "store.inventory.update",
    "store.product.update",
    "store.fulfillment.create",
    "store.return.create",
    "store.po.create",
    // ats / cms / social
    "ats.interview.schedule",
    "cms.post.create",
    "cms.post.update",
    "social.post.schedule",
    "social.post.publish",
    // email-marketing / reviews / ads / tickets
    "email-marketing.campaign.send",
    "reviews.review.respond",
    "ads.campaign.update",
    "tickets.issue.create",
    "tickets.issue.update",
    // ledger
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
];

/// Money-movement / contract-formation operations. A global full-autonomy
/// baseline must NOT auto-loosen these below `Approval` — the customer has to
/// explicitly opt each one in per employee (see `OperationPolicy::decide`).
const CRITICAL: &[&str] = &[
    "ledger.billpayment.create",
    "ledger.payment.apply",
    "ledger.creditmemo.create",
    "ledger.invoice.send",
    "esign.document.send",
    "store.po.create",
];

/// Whether the operation (bare op or fully-qualified port) is gated.
pub fn is_gated(operation: &str) -> bool {
    GATED.contains(&port_suffix(operation).as_str())
}

/// Whether the operation is critical (protected from global auto-loosening).
pub fn is_critical(operation: &str) -> bool {
    CRITICAL.contains(&port_suffix(operation).as_str())
}

/// All gated operation suffixes (for building the per-employee policy UI list).
pub fn gated_operations() -> &'static [&'static str] {
    GATED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gated_lookup_normalizes_full_port() {
        assert!(is_gated("accounting.ap-specialist.ledger.billpayment.create"));
        assert!(is_gated("ledger.billpayment.create"));
        assert!(!is_gated("ledger.vendor.find"));
        assert!(!is_gated("accounting.ap-specialist.ledger.vendor.find"));
    }

    #[test]
    fn critical_is_subset_of_gated() {
        for op in CRITICAL {
            assert!(GATED.contains(op), "critical op {op} must also be gated");
            assert!(is_critical(op));
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

    #[test]
    fn catalog_entries_are_valid_suffixes() {
        // Every entry must be a 3-segment operation suffix (capability.resource.action),
        // so port normalization matches. Guards against a malformed edit.
        for op in GATED {
            assert_eq!(op.split('.').count(), 3, "{op} is not a 3-part operation");
            assert_eq!(&port_suffix(op), op, "{op} must equal its own suffix");
        }
    }
}
