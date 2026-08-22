use serde::{Deserialize, Serialize};

/// Provenance classes — the closed, coarse taxonomy of untrusted content
/// sources a run can touch (coworker trust boundaries design, 2026-08-22).
/// Classes gate policy (memory write bars, untrusted-content treatment,
/// coworker reply labeling); they are not an audit log — the run transcript
/// and the coworker thread are the audit. The engine stamps them
/// deterministically from a static tool→class table; no model can set,
/// forge, or omit one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceClass {
    /// Fetched/browsed/searched web content.
    Web,
    /// Mail read from a connected mailbox.
    ExternalEmail,
    /// Inbound remote messages: Slack/Discord, loop traffic from another bot,
    /// SDK embed visitors, agent-authored loop posts.
    Channel,
    /// Agent-fetched files (downloads, ingested attachments). Owner-uploaded
    /// files are owner-authored and carry no taint (decision 2026-08-22).
    Document,
    /// Telephony caller content.
    Phone,
    /// Content relayed by another agent — every coworker reply carries at
    /// least this; a colleague relaying a webpage is still a webpage, so the
    /// source classes ride along with it.
    Coworker,
}

impl ProvenanceClass {
    /// Human-readable label for provenance headers and refusal messages.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::ExternalEmail => "external email",
            Self::Channel => "channel messages",
            Self::Document => "external documents",
            Self::Phone => "phone calls",
            Self::Coworker => "a coworker",
        }
    }
}

/// Render a class set as a short human-readable list ("web, external email").
pub fn label_classes(classes: &[ProvenanceClass]) -> String {
    classes
        .iter()
        .map(|c| c.label())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_kebab_round_trip() {
        let v = vec![ProvenanceClass::ExternalEmail, ProvenanceClass::Web];
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#"["external-email","web"]"#);
        let back: Vec<ProvenanceClass> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn labels_join() {
        assert_eq!(
            label_classes(&[ProvenanceClass::Web, ProvenanceClass::Phone]),
            "web, phone calls"
        );
    }
}
