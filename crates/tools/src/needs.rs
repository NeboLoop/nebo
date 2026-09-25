//! Working out an employee's needs, and the one plain line that asks the
//! owner for them (Turn-Controller-Technical-Design §2.12.6).
//!
//! One step, [`work_out_needs`], serves every door a job is made or changed
//! through: hiring a package, the builder, creating one in chat, and editing
//! a job. A package's needs come straight off its manifest (`requires.
//! interfaces`, its plugins, its watches) with no model call. An owner-made
//! employee's come from its description (one aux call that maps free text
//! onto the capability vocabulary and nothing else) plus what the skills,
//! plugins and workflows it is given declare. Either way the result is named
//! in the same vocabulary: the built-in capabilities and the interfaces
//! catalog's terms. [`consent_line`] renders it as one sentence.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::origin::ToolContext;

/// The capabilities the runtime performs itself (the tools' own
/// `capability()` answers), in plain words.
const BUILT_IN: &[(&str, &str)] = &[
    ("file", "read and change files on this computer"),
    ("shell", "run commands on this computer"),
    ("web", "look things up on the web"),
    ("browser", "use a web browser"),
    ("desktop", "control the mouse, keyboard and windows"),
    ("media", "use the camera, microphone and screen"),
    ("system", "read and change this computer's settings"),
    ("contacts", "read your contacts"),
];

/// Every interfaces-catalog capability, in plain words. The catalogue names
/// the terms; this table only says them the way an owner would. A test
/// holds it to exactly the catalogue's terms.
const CATALOG_WORDS: &[(&str, &str)] = &[
    ("ledger", "read and update your books"),
    ("payments", "handle payments, refunds and disputes"),
    ("billing", "manage billing, invoices and subscriptions"),
    ("expense", "review expense reports"),
    ("commission", "work out sales commissions"),
    ("projectbilling", "bill projects and change orders"),
    ("timebilling", "manage billable time"),
    ("legalbilling", "prepare and send invoices for billed time"),
    ("trust", "manage money held in trust"),
    ("crm", "read and update your customer records"),
    ("pricebook", "read and update your price book"),
    ("esign", "send documents for signature"),
    ("enrichment", "look up details about companies and people"),
    ("partner", "manage partner registrations"),
    ("deals", "run deals and their due diligence"),
    ("mail", "read and send email"),
    ("sms", "send text messages"),
    ("telephony", "answer your calls"),
    ("email-marketing", "run email campaigns"),
    ("maillist", "manage mailing lists"),
    ("postal", "send and track postal mail"),
    ("print", "order print jobs"),
    ("media", "buy and track advertising media"),
    ("social", "post and reply on social media"),
    ("cms", "update your website"),
    ("seo", "check how your website shows up in search"),
    ("listings", "manage your business listings"),
    ("reviews", "read and respond to reviews"),
    ("ads", "run ad campaigns"),
    ("brand", "manage your brand assets"),
    ("creators", "work with creators and their content"),
    ("affiliate", "manage affiliates and their payouts"),
    ("promotions", "run promotions"),
    ("merchandising", "manage merchandising and displays"),
    ("events", "run events and their registrations"),
    ("survey", "send surveys and read the answers"),
    ("community", "moderate and reply in your community"),
    ("automation", "manage automated flows"),
    ("experiments", "run experiments"),
    ("helpdesk", "answer support tickets"),
    ("kb", "write and update help articles"),
    ("calendar", "manage your calendar"),
    ("meetings", "record meetings and read their transcripts"),
    ("research", "run research studies with participants"),
    ("design", "work with design files"),
    ("store", "manage orders and inventory"),
    ("shipping", "ship packages and track them"),
    ("customs", "handle customs entries"),
    ("fieldservice", "schedule and dispatch field jobs"),
    ("facilities", "handle facility requests"),
    ("production", "manage production orders"),
    ("bom", "manage bills of materials"),
    ("quality", "record quality inspections"),
    ("maintenance", "schedule equipment maintenance"),
    ("warehouse", "run your data pipelines"),
    ("projects", "manage projects and deliverables"),
    ("channel", "manage marketplace listings and orders"),
    ("travel", "book and change travel"),
    ("drive", "read and organize your shared files"),
    ("records", "manage records and how long they are kept"),
    ("hris", "manage employee records"),
    ("ats", "manage hiring and candidates"),
    ("benefits", "manage benefits enrollment"),
    ("lms", "assign and track training courses"),
    ("learning", "enroll people in courses"),
    ("workforce", "draft and publish work schedules"),
    ("safety", "log safety incidents and hazards"),
    ("labor", "handle labor agreements and grievances"),
    ("ehs", "record environment, health and safety cases"),
    ("repo", "work in your code repositories"),
    ("ci", "run builds and deployments"),
    ("deploy", "roll out releases"),
    ("testing", "run tests"),
    ("appstore", "manage app store releases"),
    ("monitoring", "watch your systems and alerts"),
    ("observability", "read logs, errors and traces"),
    ("incidents", "manage incidents"),
    ("statuspage", "post status updates"),
    ("infrastructure", "change your cloud infrastructure"),
    ("database", "manage your databases"),
    ("security", "handle security alerts and findings"),
    ("migration", "run data migrations"),
    ("models", "train and deploy models"),
    ("analytics", "read and report on your data"),
    ("tickets", "manage issues and tickets"),
    ("directory", "manage user accounts and access"),
    ("devices", "manage company devices"),
    ("network", "manage your network"),
    ("saas", "manage software subscriptions and seats"),
    ("backups", "check your backups"),
    ("integrations", "keep your connected systems in sync"),
    ("fleet", "manage vehicles and drivers"),
    ("layers", "read and update your company's description"),
    ("authority", "manage standing authority for employees"),
    ("contracts", "manage contracts and obligations"),
    ("licensing", "manage licenses and filings"),
    ("insurance", "manage insurance certificates and claims"),
    ("governance", "prepare board meetings and filings"),
    ("grc", "manage risks and controls"),
    ("privacy", "handle privacy requests"),
    ("ip", "manage trademarks"),
    ("policy", "track policy acknowledgements"),
    ("continuity", "keep business continuity plans"),
    ("esg", "report sustainability figures"),
    ("architecture", "keep architecture standards and reviews"),
    ("ehr", "read and update patient records"),
    ("payer", "check coverage and submit claims to payers"),
    ("credentialing", "manage provider credentials"),
    ("property", "manage leases, tenants and rent"),
    ("screening", "run background screenings"),
    ("association", "manage an association's members and rules"),
    ("transaction", "manage transaction documents and deadlines"),
    ("estimating", "build and submit bids"),
    ("permits", "apply for permits and inspections"),
    ("liens", "prepare and file liens and waivers"),
    ("warranty", "file warranty claims"),
    ("pos", "read point-of-sale records"),
    ("recipes", "cost and publish recipes"),
    ("foodsafety", "log food safety checks"),
    ("grants", "apply for and report on grants"),
    ("donors", "manage donors and gifts"),
    ("volunteers", "manage volunteers and their shifts"),
    ("membership", "manage memberships and renewals"),
    ("sis", "manage student records"),
    ("finaid", "manage financial aid"),
    ("loans", "process loan applications"),
    ("claims", "handle claims"),
    ("financialcrime", "review financial crime alerts"),
    ("matters", "open and manage matters"),
    ("docket", "track court deadlines"),
    ("ediscovery", "run legal holds and discovery"),
];

/// Workflow activity types that are themselves one capability.
const ACTIVITY_CAPABILITY: &[(&str, &str)] =
    &[("email", "mail"), ("http", "web"), ("research", "web"), ("command", "shell")];

/// One term of the capability vocabulary and how the owner hears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityTerm {
    pub key: String,
    pub words: String,
}

/// The vocabulary a job's needs are named in: the built-in capabilities,
/// then the catalogue's terms. A term both name (`media`) keeps the
/// built-in words: the runtime's own tools decide calls on that key.
pub fn vocabulary() -> Vec<CapabilityTerm> {
    let mut seen = BTreeSet::new();
    BUILT_IN
        .iter()
        .copied()
        .chain(crate::interface_catalog::capabilities().iter().map(|k| (*k, catalog_words(k))))
        .filter(|(k, _)| seen.insert(*k))
        .map(|(k, w)| CapabilityTerm { key: k.to_string(), words: w.to_string() })
        .collect()
}

fn catalog_words(key: &str) -> &'static str {
    CATALOG_WORDS.iter().find(|(k, _)| *k == key).map(|(_, w)| *w).unwrap_or("use its connected systems")
}

/// How the owner hears one capability, or `None` for a word outside the
/// vocabulary.
pub fn words_of(key: &str) -> Option<&'static str> {
    BUILT_IN
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, w)| *w)
        .or_else(|| crate::interface_catalog::capabilities().contains(&key).then(|| catalog_words(key)))
}

fn is_built_in(key: &str) -> bool {
    BUILT_IN.iter().any(|(k, _)| *k == key) || crate::interface_catalog::is_builtin_capability(key)
}

/// What a package declares it needs, read off its manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredNeeds {
    /// `requires.interfaces`.
    pub interfaces: Vec<String>,
    /// `requires.plugins`.
    pub plugins: Vec<String>,
    /// Each workflow's watch trigger: a capability term or a plugin.
    pub watches: Vec<String>,
}

impl DeclaredNeeds {
    /// The declared needs of a package's `agent.json`.
    pub fn of(config: &napp::agent::AgentConfig) -> Self {
        let mut watches: Vec<String> = config
            .workflows
            .values()
            .filter_map(|w| match &w.trigger {
                napp::agent::AgentTrigger::Watch { plugin, .. } => Some(plugin.clone()),
                _ => None,
            })
            .collect();
        watches.sort();
        watches.dedup();
        Self { interfaces: config.requires.interfaces.clone(), plugins: config.requires.plugins.clone(), watches }
    }
}

/// Everything a job's needs are worked out from.
pub struct JobSource<'a> {
    pub name: &'a str,
    pub description: &'a str,
    /// What the skills it is given declare they use: capability terms or
    /// plugin names.
    pub skills: &'a [String],
    /// The plugins it is given.
    pub plugins: &'a [String],
    /// The workflows it is created with.
    pub workflows: &'a [napp::agent::WorkflowBinding],
    /// A package's manifest. When present the description is not read: a
    /// package's needs are what it declares.
    pub declared: Option<&'a DeclaredNeeds>,
    /// The installed plugins and the interfaces each binds, to name a
    /// plugin's needs and to tell which capabilities have no account yet.
    pub installed: &'a [(String, Vec<String>)],
}

/// A capability that runs through a connected system with no plugin bound
/// for it yet. The owner hears it through the Inbox needs flow, never as a
/// permission ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountNeed {
    pub capability: String,
}

/// A job's needs: the capabilities it may use, and the accounts those need.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub accounts: Vec<AccountNeed>,
}

impl Needs {
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// These needs less the capabilities the owner removed before creating.
    pub fn without(&self, removed: &[String]) -> Needs {
        let capabilities: BTreeSet<String> =
            self.capabilities.iter().filter(|c| !removed.contains(c)).cloned().collect();
        let accounts = self.accounts.iter().filter(|a| capabilities.contains(&a.capability)).cloned().collect();
        Needs { capabilities, accounts }
    }

    /// Each capability with its words, for a page that lists them.
    pub fn items(&self) -> Vec<CapabilityTerm> {
        self.capabilities
            .iter()
            .map(|k| CapabilityTerm { key: k.clone(), words: words_of(k).unwrap_or("use its connected systems").to_string() })
            .collect()
    }
}

/// Reads a job description onto the vocabulary: one aux call that returns
/// only terms of `vocabulary`.
#[async_trait::async_trait]
pub trait DescriptionReader: Send + Sync {
    async fn capabilities_in(&self, description: &str, vocabulary: &[CapabilityTerm]) -> Vec<String>;
}

/// The one step that works out a job's needs, for hire, the builder, chat
/// creation and job edits.
pub async fn work_out_needs(src: &JobSource<'_>, reader: &dyn DescriptionReader) -> Needs {
    let vocabulary = vocabulary();
    let known = |k: &str| vocabulary.iter().any(|t| t.key == k);
    let mut capabilities = BTreeSet::new();
    // A capability term, or a plugin named by the interfaces it binds.
    let add_term_or_plugin = |entry: &str, capabilities: &mut BTreeSet<String>| {
        let name = plugin_name(entry);
        if known(name) {
            capabilities.insert(name.to_string());
            return;
        }
        if let Some((_, interfaces)) = src.installed.iter().find(|(slug, _)| slug == name) {
            capabilities.extend(interfaces.iter().filter(|i| known(i)).cloned());
        }
    };
    match src.declared {
        Some(declared) => {
            for entry in declared.interfaces.iter().chain(&declared.plugins).chain(&declared.watches) {
                add_term_or_plugin(entry, &mut capabilities);
            }
        }
        None => {
            if !src.description.trim().is_empty() {
                let read = reader.capabilities_in(src.description, &vocabulary).await;
                capabilities.extend(read.into_iter().filter(|k| known(k)));
            }
        }
    }
    for entry in src.skills.iter().chain(src.plugins) {
        add_term_or_plugin(entry, &mut capabilities);
    }
    for workflow in src.workflows {
        if let napp::agent::AgentTrigger::Watch { plugin, .. } = &workflow.trigger {
            add_term_or_plugin(plugin, &mut capabilities);
        }
        for activity in &workflow.activities {
            if let Some((_, cap)) = ACTIVITY_CAPABILITY.iter().find(|(t, _)| *t == activity.activity_type) {
                capabilities.insert(cap.to_string());
            }
        }
    }
    let accounts = capabilities
        .iter()
        .filter(|c| !is_built_in(c))
        .filter(|c| !src.installed.iter().any(|(_, interfaces)| interfaces.contains(c)))
        .map(|c| AccountNeed { capability: c.clone() })
        .collect();
    Needs { capabilities, accounts }
}

/// The plain name of a plugin reference: `@org/plugins/name@^1` → `name`.
fn plugin_name(entry: &str) -> &str {
    let bare = entry.strip_prefix('@').map(|s| s.split('@').next().unwrap_or(s)).unwrap_or(entry);
    bare.rsplit('/').next().unwrap_or(bare)
}

/// The one plain sentence that asks the owner for `needs`: "Receptionist
/// will answer your calls, read and send email, and manage your calendar."
pub fn consent_line(employee: &str, needs: &Needs) -> String {
    let employee = employee.trim();
    let phrases: Vec<String> = needs.items().into_iter().map(|t| t.words).collect();
    match phrases.as_slice() {
        [] => format!("{employee} will only keep its own notes and tasks."),
        [one] => format!("{employee} will {one}."),
        [a, b] => format!("{employee} will {a} and {b}."),
        [rest @ .., last] => format!("{employee} will {}, and {last}.", rest.join(", ")),
    }
}

/// What `after` needs that `before` doesn't: an edited job asks only for
/// these.
pub fn added(before: &Needs, after: &Needs) -> Needs {
    let capabilities: BTreeSet<String> = after.capabilities.difference(&before.capabilities).cloned().collect();
    let accounts = after.accounts.iter().filter(|a| capabilities.contains(&a.capability)).cloned().collect();
    Needs { capabilities, accounts }
}

/// What granting a new employee's job came to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Granted {
    /// The needs now standing allow rules.
    pub granted: Needs,
    /// The needs beyond the creator's grant, waiting on the owner's card.
    pub extras: Needs,
    /// The card's ask id, when there are extras.
    pub card: Option<String>,
}

/// One job to grant: a new employee's, or the needs an edit adds.
pub struct JobGrant<'a> {
    pub agent_id: &'a str,
    pub name: &'a str,
    pub needs: &'a Needs,
    pub draft_id: &'a str,
    /// The owner said yes to this draft's line.
    pub consented: bool,
    /// A new employee (held under its creator until the owner answers);
    /// otherwise an edit to an existing one, which only the owner widens.
    pub created: bool,
}

/// The shared cell the registry fills late with the permission system's
/// [`JobConsent`] (registration runs before the server's state exists).
pub type JobConsentCell = std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<dyn JobConsent>>>>;

/// What the create tool asks of the permission system. Implemented by
/// `agent::harness::permissions::consent::Consent`: reading a description,
/// whether the owner said yes, and writing the job.
#[async_trait::async_trait]
pub trait JobConsent: DescriptionReader {
    /// Whether an owner message arrived in the draft's chat after its line
    /// was shown: the owner's "yes, create it".
    fn owner_consented(&self, draft_id: &str) -> bool;
    /// Grant a job. With the owner's consent every need becomes a standing
    /// allow; without it the employee whose run this is hands over no more
    /// than it holds, and the rest goes to the owner as one card.
    fn grant(&self, ctx: &ToolContext, job: &JobGrant<'_>) -> Result<Granted, String>;
    /// The job an employee holds now, as needs.
    fn job_of(&self, agent_id: &str) -> Needs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A reader that counts its calls and answers with fixed words, some
    /// outside the vocabulary.
    struct Reader {
        calls: AtomicUsize,
        answer: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl DescriptionReader for Reader {
        async fn capabilities_in(&self, _d: &str, _v: &[CapabilityTerm]) -> Vec<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.iter().map(|s| s.to_string()).collect()
        }
    }

    fn reader(answer: Vec<&'static str>) -> Reader {
        Reader { calls: AtomicUsize::new(0), answer }
    }

    fn source<'a>(description: &'a str, declared: Option<&'a DeclaredNeeds>) -> JobSource<'a> {
        JobSource {
            name: "Receptionist",
            description,
            skills: &[],
            plugins: &[],
            workflows: &[],
            declared,
            installed: &[],
        }
    }

    #[test]
    fn every_catalog_term_has_words_and_no_words_lack_a_term() {
        let terms = crate::interface_catalog::capabilities();
        for t in terms {
            assert!(CATALOG_WORDS.iter().any(|(k, _)| k == t), "{t} has no words");
        }
        for (k, w) in CATALOG_WORDS {
            assert!(terms.contains(k), "{k} is not a catalogue term");
            assert!(!w.is_empty() && !w.contains('.') && w.chars().next().unwrap().is_lowercase(), "{k}: {w}");
        }
        let v = vocabulary();
        let keys: BTreeSet<&str> = v.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys.len(), v.len(), "each term once");
    }

    #[tokio::test]
    async fn package_needs_are_read_without_a_model_call() {
        let declared = DeclaredNeeds {
            interfaces: vec!["telephony".into(), "mail".into()],
            plugins: vec![],
            watches: vec!["calendar".into()],
        };
        let r = reader(vec!["shell"]);
        let needs = work_out_needs(&source("Answers calls and runs scripts", Some(&declared)), &r).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 0, "a package's needs never ask a model");
        assert_eq!(needs.capabilities, ["calendar", "mail", "telephony"].map(String::from).into());
    }

    #[tokio::test]
    async fn description_maps_only_onto_the_vocabulary() {
        let r = reader(vec!["mail", "calendar", "teleport", "invoices", "web"]);
        let needs = work_out_needs(&source("Chases unpaid invoices by email", None), &r).await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 1, "one aux call");
        assert_eq!(needs.capabilities, ["calendar", "mail", "web"].map(String::from).into());
        // Nothing to read, nothing asked.
        let r = reader(vec!["mail"]);
        assert!(work_out_needs(&source("  ", None), &r).await.is_empty());
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn skills_plugins_and_workflows_add_their_declared_needs() {
        let installed = vec![("acme-books".to_string(), vec!["ledger".to_string(), "not-a-term".to_string()])];
        let workflow: napp::agent::WorkflowBinding = serde_json::from_value(serde_json::json!({
            "trigger": {"type": "watch", "plugin": "mail"},
            "activities": [{"id": "a", "type": "http"}]
        }))
        .unwrap();
        let skills = vec!["calendar".to_string(), "python".to_string()];
        let plugins = vec!["@acme/plugins/acme-books@^1".to_string()];
        let src = JobSource {
            name: "Clerk",
            description: "",
            skills: &skills,
            plugins: &plugins,
            workflows: std::slice::from_ref(&workflow),
            declared: None,
            installed: &installed,
        };
        let needs = work_out_needs(&src, &reader(vec![])).await;
        assert_eq!(needs.capabilities, ["calendar", "ledger", "mail", "web"].map(String::from).into());
        // Calendar and mail run through a connected system no installed
        // plugin binds: accounts the owner connects, not permissions.
        let accounts: Vec<&str> = needs.accounts.iter().map(|a| a.capability.as_str()).collect();
        assert_eq!(accounts, vec!["calendar", "mail"]);
    }

    #[test]
    fn consent_line_is_one_plain_sentence() {
        let needs = |c: &[&str]| Needs { capabilities: c.iter().map(|s| s.to_string()).collect(), accounts: vec![] };
        assert_eq!(
            consent_line("Receptionist", &needs(&["telephony", "mail", "calendar"])),
            "Receptionist will manage your calendar, read and send email, and answer your calls."
        );
        assert_eq!(consent_line("Clerk", &needs(&["mail", "calendar"])), "Clerk will manage your calendar and read and send email.");
        assert_eq!(consent_line("Clerk", &needs(&["web"])), "Clerk will look things up on the web.");
        assert_eq!(consent_line("Clerk", &needs(&[])), "Clerk will only keep its own notes and tasks.");
        for line in [
            consent_line("Receptionist", &needs(&["telephony", "mail", "calendar", "shell", "ledger"])),
            consent_line("Clerk", &needs(&["web"])),
        ] {
            assert_eq!(line.matches('.').count(), 1, "one sentence: {line}");
            assert!(line.ends_with('.'));
            for rule_string in ["mail.message", "telephony", "ledger", "capability", "_"] {
                assert!(!line.contains(rule_string), "no rule strings in the line: {line}");
            }
        }
    }

    #[test]
    fn added_is_only_what_the_edit_brings() {
        let before = Needs { capabilities: ["mail".to_string()].into(), accounts: vec![] };
        let after = Needs {
            capabilities: ["calendar".to_string(), "mail".to_string()].into(),
            accounts: vec![AccountNeed { capability: "calendar".into() }, AccountNeed { capability: "mail".into() }],
        };
        let new = added(&before, &after);
        assert_eq!(new.capabilities, ["calendar".to_string()].into());
        assert_eq!(new.accounts, vec![AccountNeed { capability: "calendar".into() }]);
        assert!(added(&after, &after).is_empty());
    }
}
