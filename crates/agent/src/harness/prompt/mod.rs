//! The system prompt: the fixed part in our words, the cache boundary, then
//! the employee. Byte-stable across a session: it changes only when the
//! owner edits the employee. There is no per-call state block and no session
//! fact in it; the environment, the model and mode, the employee's memory
//! and the session context arrive as attachment rows written when they are
//! first told and when they change (`events::SessionFacts`).

pub mod inputs;
pub mod sections;

/// Whose turn the prompt is for.
pub enum Role {
    /// The owner's own employee.
    Employee,
    /// A helper doing one task for the employee named `parent`.
    Helper { parent: String },
}

/// Everything the system prompt is built from.
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
}

/// A turn's system prompt, in the order it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPrompt {
    pub fixed: String,
    pub boundary: &'static str,
    pub employee: String,
}

/// Characters in each part, for the turn's telemetry line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptSizes {
    pub fixed: usize,
    pub employee: usize,
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
        SystemPrompt {
            fixed,
            boundary: crate::prompt::CACHE_BOUNDARY,
            employee,
        }
    }

    /// The prompt as sent.
    pub fn text(&self) -> String {
        format!("{}{}{}", self.fixed, self.boundary, self.employee)
    }

    /// Byte offsets into `text()` where the provider may cache: after the
    /// fixed part (shared by every employee) and at the end.
    pub fn cache_breakpoints(&self) -> Vec<usize> {
        vec![self.fixed.len(), self.fixed.len() + self.boundary.len() + self.employee.len()]
    }

    pub fn sizes(&self) -> PromptSizes {
        let fixed = self.fixed.chars().count();
        let employee = self.employee.chars().count();
        PromptSizes { fixed, employee, total: self.text().chars().count() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(role: Role) -> PromptInputs {
        PromptInputs {
            name: "Ava".to_string(),
            role,
            personality_snippet: Some("Keep it light.".to_string()),
            soul: Some("Warm, direct, precise.".to_string()),
            rules: Some("Never book travel over $500 without checking.".to_string()),
            persona: Some("You run the owner's calendar and inbox.".to_string()),
        }
    }

    #[test]
    fn fixed_part_is_byte_stable_across_calls() {
        let a = SystemPrompt::build(&inputs(Role::Employee));
        assert_eq!(a, SystemPrompt::build(&inputs(Role::Employee)));
        let text = a.text();
        let [shared, end] = a.cache_breakpoints()[..] else { panic!("two breakpoints") };
        assert!(text[shared..].starts_with(crate::prompt::CACHE_BOUNDARY), "the shared part ends at the boundary");
        assert_eq!(end, text.len());
        // Another employee shares the part above the boundary only when its
        // name is the same; its own section sits below.
        let mut other = inputs(Role::Employee);
        other.persona = Some("You keep the books.".to_string());
        let b = SystemPrompt::build(&other);
        assert_eq!(a.fixed, b.fixed);
        assert_ne!(a.employee, b.employee);
    }

    /// Nothing a session changes is in the prompt: the date, the mode, the
    /// working folder and the employee's memory are rows, so none of them
    /// is an input here and the prompt holds none of their text.
    #[test]
    fn system_prompt_unchanged_by_date_mode_folder_or_memory_changes() {
        let text = SystemPrompt::build(&inputs(Role::Employee)).text();
        for fact in ["# Environment", "Date:", "Permission mode", "Working folder", "Model:", "# User Information", "# Workspace notes", "# Your coworkers"] {
            assert!(!text.contains(fact), "{fact:?} is a session fact in the prompt");
        }
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
        assert_eq!(e.total, e.fixed + crate::prompt::CACHE_BOUNDARY.chars().count() + e.employee);
        assert!(e.fixed < 8_000);
    }

    const FIXED_CHARS: usize = 5_130;
    const HELPER_FIXED_CHARS: usize = 5_637;
}
