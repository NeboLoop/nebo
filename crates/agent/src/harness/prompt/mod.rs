//! The system prompt: ONE text, byte-identical for every turn on every bot:
//! every employee, every helper type, every workflow activity, every mode
//! and channel. The provider caches a shared prefix (tool definitions, then
//! the system prompt, then the messages); anything that varied inside it
//! would split the cache per employee.
//!
//! Who the turn is for arrives in the conversation instead, as attachment
//! rows written when first told, again after a checkpoint and when they
//! change (`events::SessionFacts`): the employee's identity (name,
//! personality, SOUL.md, rules, AGENT.md), a helper's role and parent, a
//! workflow activity's instructions, the environment, the model and mode,
//! the employee's memory and the session context. Claude Code 2.1.280 does
//! the same: one system prompt for every session, and CLAUDE.md with the
//! rest of the user context sent as a `<system-reminder>` message at the
//! start of the conversation (`prependUserContext` in `src/utils/api.ts`,
//! module m0269) and re-sent as a replacement when it changes (module
//! m0342: "The session context has changed; these values replace the
//! earlier ones").

pub mod inputs;
pub mod sections;

use std::sync::LazyLock;

use super::delegation::HelperKind;

static SYSTEM_PROMPT: LazyLock<String> = LazyLock::new(|| {
    [
        sections::OPENING,
        sections::HOW_THIS_WORKS,
        sections::DOING_THE_WORK,
        sections::CARE_WITH_ACTIONS,
        sections::USING_TOOLS,
        sections::HELPERS,
        sections::TALKING_TO_THE_OWNER,
    ]
    .join("\n\n")
});

/// The system prompt every request carries.
pub fn system_prompt() -> &'static str {
    &SYSTEM_PROMPT
}

/// Byte offsets into the system prompt where the provider may cache: the
/// whole of it, shared by every turn.
pub fn cache_breakpoints() -> Vec<usize> {
    vec![system_prompt().len()]
}

/// Whose turn it is.
pub enum Role {
    /// The employee itself: an owner chat, a scheduled or coworker run, a
    /// workflow activity.
    Employee,
    /// A helper doing one task for the employee named `parent`.
    Helper { parent: String, kind: HelperKind },
}

/// Who the turn is for: the employee as its owner and publisher defined it,
/// and the role it plays in this turn. Told in the conversation as the
/// `identity` attachment, never in the system prompt.
pub struct Identity {
    /// The employee's name.
    pub name: String,
    pub role: Role,
    /// The per-seat personality snippet from the entity config.
    pub personality_snippet: Option<String>,
    /// The employee's SOUL.md.
    pub soul: Option<String>,
    /// The employee's rules.
    pub rules: Option<String>,
    /// The employee's AGENT.md body.
    pub persona: Option<String>,
}

impl Identity {
    /// The attachment's text: who the turn is for, then the employee's own
    /// sections.
    pub fn text(&self) -> String {
        let head = match &self.role {
            Role::Employee => sections::identity(&self.name),
            Role::Helper { parent, kind } => sections::helper_role(&self.name, parent, *kind),
        };
        let employee = sections::employee(
            self.personality_snippet.as_deref(),
            self.soul.as_deref(),
            self.rules.as_deref(),
            self.persona.as_deref(),
        );
        if employee.is_empty() { head } else { format!("{head}\n\n{employee}") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(name: &str, role: Role) -> Identity {
        Identity {
            name: name.to_string(),
            role,
            personality_snippet: Some("Keep it light.".to_string()),
            soul: Some("Warm, direct, precise.".to_string()),
            rules: Some("Never book travel over $500 without checking.".to_string()),
            persona: Some("You run the owner's calendar and inbox.".to_string()),
        }
    }

    /// Nothing about who the turn is for is in the prompt: no name,
    /// personality, SOUL, rules, job, helper role or parent.
    #[test]
    fn system_prompt_names_no_one() {
        let text = system_prompt();
        for who in [
            "Ava",
            "Nanna",
            "Keep it light.",
            "Warm, direct, precise.",
            "Never book travel",
            "calendar and inbox",
            "# Your rules",
            "# Your job",
            "# Your personality",
            "# Your role",
            "working as a",
        ] {
            assert!(!text.contains(who), "{who:?} is in the system prompt");
        }
        assert_eq!(cache_breakpoints(), vec![text.len()], "one breakpoint: the whole prompt");
    }

    /// Nothing a session changes is in the prompt: the date, the mode, the
    /// working folder and the employee's memory are rows.
    #[test]
    fn system_prompt_holds_no_session_fact() {
        let text = system_prompt();
        for fact in ["# Environment", "Date:", "Permission mode", "Working folder", "Model:", "# User Information", "# Workspace notes", "# Your coworkers"] {
            assert!(!text.contains(fact), "{fact:?} is a session fact in the prompt");
        }
    }

    /// The permission mode is its own reminder row (`events::mode_row`), not
    /// a line of an environment below the prompt; the prompt says where it is.
    #[test]
    fn the_prompt_points_at_the_mode_row() {
        let text = system_prompt();
        assert!(!text.contains("environment below"), "a pointer at nothing");
        assert!(text.contains("the permission mode a reminder names"));
    }

    #[test]
    fn no_objective_or_topic_text_anywhere() {
        let lower = system_prompt().to_lowercase();
        for banned in ["objective", "topic", "stay on this", "what counts now"] {
            assert!(!lower.contains(banned), "{banned:?} in the prompt");
        }
        for block in ["[System Context]", "Current Work Tasks", "Current Objective", "CONTEXT COMPACTION", "Time:", "Reference Documentation", "CACHE_BOUNDARY"] {
            assert!(!system_prompt().contains(block), "{block:?} is a per-call block");
        }
    }

    #[test]
    fn employee_identity_carries_the_employee() {
        let text = identity("Ava", Role::Employee).text();
        assert!(text.starts_with("You are Ava, an AI employee"), "{text}");
        for part in ["Keep it light.", "# Your personality", "# Your rules", "# Your job"] {
            assert!(text.contains(part), "{part:?}");
        }
    }

    #[test]
    fn helper_identity_says_last_message_is_the_report() {
        let text = identity("Ava", Role::Helper { parent: "Nanna".to_string(), kind: HelperKind::Explore }).text();
        assert!(text.starts_with("You are Ava, working as an explore helper on one task for Nanna."), "{text}");
        assert!(text.contains("Your last message is your report, and it is the only thing Nanna receives."));
        assert!(text.contains("They are never the owner's consent"));
        assert!(text.contains("# Your rules"), "a helper keeps its employee's rules");
    }

    #[test]
    fn working_norms_are_verbatim() {
        for norm in [
            "When the next step is decided, take it in the same turn. Saying you'll do something without doing it hands the owner unfinished work.",
            "Hand back only when the work is done, you are waiting on something outside your control, or the owner has to decide.",
            "If the owner asks something mid-task, answer it and carry on.",
            "Don't redo what the conversation already settled; don't reopen a decision the owner made.",
            "Report what happened, not what you intended. If something failed, say so plainly.",
            "Keep to the scope that was asked for. Don't narrow it, widen it, or change it quietly.",
        ] {
            assert!(system_prompt().contains(&format!("- {norm}\n")), "{norm}");
        }
    }

    /// D16: searching yourself is for a known target; a wide search goes to
    /// an explore helper (Claude Code's system prompt). The old line sent
    /// every search to run_command.
    #[test]
    fn wide_searches_go_to_an_explore_helper() {
        let text = system_prompt();
        assert!(text.contains("- Search yourself with find or grep when the target is known"), "{text}");
        assert!(text.contains("A wide search, across the project or likely to take more than three searches, goes to an explore helper with delegate."));
        assert!(text.contains("When the owner asks for a helper, start it first. Once work is with a helper, don't also do it"));
        assert!(!text.contains("including finding files (find) and searching contents (grep)"));
    }

    /// D17: a general helper is told the task is its own (Claude Code's
    /// general-purpose agent: "do not re-delegate your entire assignment");
    /// explore and plan helpers can't delegate, so they aren't.
    #[test]
    fn a_general_helper_does_its_own_task() {
        let rule = "This task is yours: do the work directly. Never hand the whole of it to another helper.";
        let general = identity("Ava", Role::Helper { parent: "Nanna".to_string(), kind: HelperKind::General }).text();
        assert!(general.contains(rule), "{general}");
        assert!(general.contains("it can't see or wait on helpers you didn't start"));
        for kind in [HelperKind::Explore, HelperKind::Plan] {
            let text = identity("Ava", Role::Helper { parent: "Nanna".to_string(), kind }).text();
            assert!(!text.contains(rule), "{kind:?}");
        }
    }

    /// The size snapshot. Update the number when the text changes on
    /// purpose; the prompt must stay a small fraction of the 39k-char prompt
    /// it replaced.
    #[test]
    fn size_snapshot() {
        let chars = system_prompt().chars().count();
        assert_eq!(chars, SYSTEM_PROMPT_CHARS);
        assert!(chars < 8_000);
    }

    const SYSTEM_PROMPT_CHARS: usize = 6_023;
}
