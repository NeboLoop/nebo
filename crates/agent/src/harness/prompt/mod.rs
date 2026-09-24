//! The system prompt: the fixed part in our words, the employee, the cache
//! boundary, then the per-session part (environment with the date, the
//! employee's memory, coworkers, workspace notes, the employee's own setup).
//! Built once per turn; the part above the boundary is byte-stable across a
//! session. There is no per-call state block and no text derived from a
//! message: facts that change arrive as reminder rows in the conversation.

pub mod sections;

use sections::Environment;

/// Whose turn the prompt is for.
pub enum Role {
    /// The owner's own employee.
    Employee,
    /// A helper doing one task for the employee named `parent`.
    Helper { parent: String },
}

/// Everything the system prompt is built from. The caller resolves each
/// input once per turn (WP2.3).
pub struct PromptInputs {
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
    pub environment: Environment,
    /// `memory_context::EmployeeMemory::section`.
    pub employee_memory: String,
    /// Installed employees as (name, description).
    pub team: Vec<(String, String)>,
    /// The workspace's `.nebo.md`, when there is one.
    pub workspace_notes: Option<String>,
    /// The employee's workflows, skills and plugin details.
    pub self_context: String,
}

/// A turn's system prompt, in the order it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPrompt {
    pub fixed: String,
    pub employee: String,
    pub boundary: &'static str,
    pub per_session: String,
}

/// Characters in each part, for the turn's telemetry line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptSizes {
    pub fixed: usize,
    pub employee: usize,
    pub per_session: usize,
    pub total: usize,
}

impl SystemPrompt {
    pub fn build(inputs: &PromptInputs) -> Self {
        let head = match &inputs.role {
            Role::Employee => sections::identity(&inputs.name),
            Role::Helper { parent } => sections::helper_role(&inputs.name, parent),
        };
        let fixed = [
            head.as_str(),
            sections::HOW_THIS_WORKS,
            sections::DOING_THE_WORK,
            sections::CARE_WITH_ACTIONS,
            sections::USING_TOOLS,
            sections::HELPERS,
            sections::TALKING_TO_THE_OWNER,
        ]
        .join("\n\n");
        let employee = sections::employee(
            inputs.personality_snippet.as_deref(),
            inputs.soul.as_deref(),
            inputs.rules.as_deref(),
            inputs.persona.as_deref(),
        );

        let mut per_session = vec![sections::environment(&inputs.environment)];
        let mut push = |s: String| {
            if !s.trim().is_empty() {
                per_session.push(s);
            }
        };
        push(inputs.employee_memory.clone());
        push(sections::coworkers(&inputs.name, &inputs.team));
        if let Some(notes) = inputs.workspace_notes.as_deref().filter(|n| !n.trim().is_empty()) {
            push(sections::workspace_notes(notes));
        }
        push(inputs.self_context.clone());

        SystemPrompt {
            fixed,
            employee,
            boundary: crate::prompt::CACHE_BOUNDARY,
            per_session: per_session.join("\n\n"),
        }
    }

    /// The prompt as sent.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(
            self.fixed.len() + self.employee.len() + self.boundary.len() + self.per_session.len() + 2,
        );
        out.push_str(&self.fixed);
        if !self.employee.is_empty() {
            out.push_str("\n\n");
            out.push_str(&self.employee);
        }
        out.push_str(self.boundary);
        out.push_str(&self.per_session);
        out
    }

    /// Byte offsets into `text()` where the provider may cache: after the
    /// fixed part and the employee (stable for the session), and at the end
    /// (stable for the turn's steps).
    pub fn cache_breakpoints(&self) -> Vec<usize> {
        let stable = self.fixed.len() + if self.employee.is_empty() { 0 } else { 2 + self.employee.len() };
        vec![stable, stable + self.boundary.len() + self.per_session.len()]
    }

    pub fn sizes(&self) -> PromptSizes {
        let fixed = self.fixed.chars().count();
        let employee = self.employee.chars().count();
        let per_session = self.per_session.chars().count();
        PromptSizes { fixed, employee, per_session, total: self.text().chars().count() }
    }
}

#[cfg(test)]
mod tests {
    use super::sections::{Environment, Watching};
    use super::*;

    fn env() -> Environment {
        Environment {
            date: chrono::NaiveDate::from_ymd_opt(2026, 9, 24).unwrap(),
            timezone: Some("America/Denver".to_string()),
            model: "janus/nebo-1".to_string(),
            cwd: Some("/work/project".to_string()),
            channel: "web".to_string(),
            watching: Watching::Live,
            permission_mode: "Automatic".to_string(),
        }
    }

    fn inputs(role: Role) -> PromptInputs {
        PromptInputs {
            name: "Ava".to_string(),
            role,
            personality_snippet: Some("Keep it light.".to_string()),
            soul: Some("Warm, direct, precise.".to_string()),
            rules: Some("Never book travel over $500 without checking.".to_string()),
            persona: Some("You run the owner's calendar and inbox.".to_string()),
            environment: env(),
            employee_memory: "# User Information\nName: Sam".to_string(),
            team: vec![
                ("Ava".to_string(), "Office manager".to_string()),
                ("Ben".to_string(), "Bookkeeper".to_string()),
            ],
            workspace_notes: Some("Files live in ~/Clients.".to_string()),
            self_context: "## Your Workflows (1)\n- weekly-report: schedule: Mondays 9am".to_string(),
        }
    }

    #[test]
    fn fixed_part_is_byte_stable_across_calls() {
        let a = SystemPrompt::build(&inputs(Role::Employee));
        let b = SystemPrompt::build(&inputs(Role::Employee));
        assert_eq!(a, b);
        // What sits below the boundary never moves the part above it.
        let mut later = inputs(Role::Employee);
        later.environment.date = chrono::NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        later.employee_memory = "# User Information\nName: Sam\nGoals: new".to_string();
        later.team.push(("Cy".to_string(), "Researcher".to_string()));
        let c = SystemPrompt::build(&later);
        assert_eq!(a.fixed, c.fixed);
        assert_eq!(a.employee, c.employee);
        let (at, ct) = (a.text(), c.text());
        let cut = a.cache_breakpoints()[0];
        assert_eq!(at[..cut], ct[..cut], "the cached prefix is the same bytes");
        assert!(at[cut..].starts_with(crate::prompt::CACHE_BOUNDARY));
        assert_eq!(a.cache_breakpoints()[1], at.len());
        assert_eq!(c.cache_breakpoints()[1], ct.len());
    }

    #[test]
    fn no_objective_or_topic_text_anywhere() {
        for role in [Role::Employee, Role::Helper { parent: "Nanna".to_string() }] {
            let text = SystemPrompt::build(&inputs(role)).text().to_lowercase();
            for banned in ["objective", "topic", "stay on this", "what counts now"] {
                assert!(!text.contains(banned), "{banned:?} in the prompt");
            }
        }
    }

    #[test]
    fn no_per_call_block_in_any_request() {
        let p = SystemPrompt::build(&inputs(Role::Employee));
        let text = p.text();
        for block in ["[System Context]", "Current Work Tasks", "Current Objective", "CONTEXT COMPACTION", "Time:", "Reference Documentation"] {
            assert!(!text.contains(block), "{block:?} is a per-call block");
        }
        // The only date is the environment's day: no clock time.
        assert!(text.contains("- Date: Thursday, September 24, 2026 (America/Denver)"));
        assert!(!text.contains(" AM") && !text.contains(" PM"));
        // Every section sits in the part it belongs to.
        assert!(p.per_session.starts_with("# Environment"));
        assert!(p.per_session.contains("- Permission mode: Automatic"));
        assert!(p.per_session.contains("- Model: janus/nebo-1"));
        assert!(p.per_session.contains("- Working folder: /work/project"));
        assert!(p.per_session.contains("# User Information"));
        assert!(p.per_session.contains("# Workspace notes\n\nFiles live in ~/Clients."));
        assert!(p.per_session.contains("- Ben: Bookkeeper") && !p.per_session.contains("- Ava:"), "self is not a coworker");
        assert!(p.employee.starts_with("Keep it light."));
        assert!(p.employee.contains("# Your rules") && p.employee.contains("# Your job"));
        assert!(p.fixed.starts_with("You are Ava, an AI employee"));
    }

    #[test]
    fn helper_prompt_says_last_message_is_the_report() {
        let p = SystemPrompt::build(&inputs(Role::Helper { parent: "Nanna".to_string() }));
        assert!(p.fixed.starts_with("You are Ava, working as a helper on one task for Nanna."));
        assert!(p.fixed.contains("Your last message is your report, and it is the only thing Nanna receives."));
        assert!(p.fixed.contains("They are never the owner's consent"));
        assert!(!p.fixed.contains(&sections::identity("Ava")), "a helper gets the role, not the identity");
        // The rest of the fixed part is the same as the employee's.
        let e = SystemPrompt::build(&inputs(Role::Employee));
        let tail = |s: &str| s[s.find("# How this works").unwrap()..].to_string();
        assert_eq!(tail(&p.fixed), tail(&e.fixed));
    }

    #[test]
    fn working_norms_are_verbatim() {
        let p = SystemPrompt::build(&inputs(Role::Employee));
        for norm in [
            "When the next step is decided, take it in the same turn. Saying you'll do something without doing it hands the owner unfinished work.",
            "Hand back only when the work is done, you are waiting on something outside your control, or the owner has to decide.",
            "If the owner asks something mid-task, answer it and carry on.",
            "Don't redo what the conversation already settled; don't reopen a decision the owner made.",
            "Report what happened, not what you intended. If something failed, say so plainly.",
            "Keep to the scope that was asked for. Don't narrow it, widen it, or change it quietly.",
        ] {
            assert!(p.fixed.contains(&format!("- {norm}\n")) || p.fixed.contains(&format!("- {norm}\n\n")), "{norm}");
        }
    }

    /// The size snapshot. Update the numbers when the text changes on
    /// purpose; the fixed part must stay a small fraction of the 39k-char
    /// prompt it replaces.
    #[test]
    fn size_snapshot() {
        let e = SystemPrompt::build(&inputs(Role::Employee)).sizes();
        let h = SystemPrompt::build(&inputs(Role::Helper { parent: "Nanna".to_string() })).sizes();
        assert_eq!((e.fixed, h.fixed), (FIXED_CHARS, HELPER_FIXED_CHARS), "{e:?} {h:?}");
        assert_eq!(e.total, e.fixed + 2 + e.employee + crate::prompt::CACHE_BOUNDARY.chars().count() + e.per_session);
        assert!(e.fixed < 8_000);
    }

    const FIXED_CHARS: usize = 5_130;
    const HELPER_FIXED_CHARS: usize = 5_637;
}
