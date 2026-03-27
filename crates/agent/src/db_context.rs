use std::collections::HashSet;

use db::models::{AgentProfile, UserPreference, UserProfile};
use db::Store;
use tracing::debug;

use crate::memory::{self, ScoredMemory};

/// Rich context loaded from the database for prompt assembly.
pub struct DBContext {
    pub agent: Option<AgentProfile>,
    pub user: Option<UserProfile>,
    pub preferences: Option<UserPreference>,
    pub personality_directive: Option<String>,
    pub tacit_memories: Vec<ScoredMemory>,
}

/// Load all database context needed for prompt assembly.
pub fn load_db_context(store: &Store, user_id: &str) -> DBContext {
    let agent = store.get_agent_profile().ok().flatten();
    let user = store.get_user_profile().ok().flatten();
    let preferences = store.get_user_preferences().ok().flatten();

    // Load personality directive from tacit/personality/directive
    let personality_directive = store
        .get_memory_by_key_and_user("tacit/personality", "directive", user_id)
        .ok()
        .flatten()
        .map(|m| m.value);

    // Load scored tacit memories
    let tacit_memories = memory::load_scored_memories(store, user_id, 40);

    DBContext {
        agent,
        user,
        preferences,
        personality_directive,
        tacit_memories,
    }
}

/// Format the DB context into a rich system prompt section.
/// Produces 9 sections joined with separators, matching Go's FormatForSystemPrompt.
pub fn format_for_system_prompt(ctx: &DBContext, agent_name: &str) -> String {
    let mut sections: Vec<String> = Vec::new();

    // 1. Agent identity
    if let Some(ref agent) = ctx.agent {
        let personality = agent
            .custom_personality
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| personality_preset_prompt(agent.personality_preset.as_deref()))
            .unwrap_or("You are a helpful personal AI companion.");
        sections.push(format!("# Identity\n{}", personality));
    }

    // 2. Character (creature, role, vibe, emoji)
    if let Some(ref agent) = ctx.agent {
        let mut parts = Vec::new();
        if let Some(ref creature) = agent.creature {
            if !creature.is_empty() {
                parts.push(format!("Creature: {}", creature));
            }
        }
        if let Some(ref role) = agent.role {
            if !role.is_empty() {
                parts.push(format!("Role: {}", role));
            }
        }
        if let Some(ref vibe) = agent.vibe {
            if !vibe.is_empty() {
                parts.push(format!("Vibe: {}", vibe));
            }
        }
        if let Some(ref emoji) = agent.emoji {
            if !emoji.is_empty() {
                parts.push(format!("Emoji: {}", emoji));
            }
        }
        if !parts.is_empty() {
            sections.push(format!("# Character\n{}", parts.join("\n")));
        }
    }

    // 3. Personality directive (learned from style observations)
    if let Some(ref directive) = ctx.personality_directive {
        if !directive.is_empty() {
            sections.push(format!(
                "# Personality (Learned)\n{}",
                directive
            ));
        }
    }

    // 4. Communication style
    {
        let mut parts = Vec::new();

        // Language preference from user preferences
        if let Some(ref prefs) = ctx.preferences {
            if !prefs.language.is_empty() && prefs.language != "en" {
                parts.push(format!(
                    "Language: The user's preferred language is {}. Always respond in this language unless the user explicitly writes in a different language.",
                    language_display_name(&prefs.language)
                ));
            }
        }

        if let Some(ref agent) = ctx.agent {
            if let Some(ref voice) = agent.voice_style {
                if !voice.is_empty() {
                    parts.push(format!("Voice: {}", voice));
                }
            }
            if let Some(ref formality) = agent.formality {
                if !formality.is_empty() {
                    parts.push(format!("Formality: {}", formality));
                }
            }
            if let Some(ref length) = agent.response_length {
                if !length.is_empty() {
                    parts.push(format!("Response length: {}", length));
                }
            }
            if let Some(ref emoji_usage) = agent.emoji_usage {
                if !emoji_usage.is_empty() {
                    parts.push(format!("Emoji usage: {}", emoji_usage));
                }
            }
        }

        if !parts.is_empty() {
            sections.push(format!("# Communication Style\n{}", parts.join("\n")));
        }
    }

    // 5. User information
    if let Some(ref user) = ctx.user {
        let mut parts = Vec::new();
        if let Some(ref name) = user.display_name {
            if !name.is_empty() {
                parts.push(format!("Name: {}", name));
            }
        }
        if let Some(ref location) = user.location {
            if !location.is_empty() {
                parts.push(format!("Location: {}", location));
            }
        }
        if let Some(ref tz) = user.timezone {
            if !tz.is_empty() {
                parts.push(format!("Timezone: {}", tz));
            }
        }
        if let Some(ref occ) = user.occupation {
            if !occ.is_empty() {
                parts.push(format!("Occupation: {}", occ));
            }
        }
        if let Some(ref interests) = user.interests {
            if !interests.is_empty() {
                parts.push(format!("Interests: {}", interests));
            }
        }
        if let Some(ref goals) = user.goals {
            if !goals.is_empty() {
                parts.push(format!("Goals: {}", goals));
            }
        }
        if let Some(ref context) = user.context {
            if !context.is_empty() {
                parts.push(format!("Context: {}", context));
            }
        }
        if !parts.is_empty() {
            sections.push(format!("# User Information\n{}", parts.join("\n")));
        }
    }

    // 6. Agent rules
    if let Some(ref agent) = ctx.agent {
        if let Some(ref rules) = agent.agent_rules {
            if !rules.is_empty() {
                let formatted = format_structured_or_raw(rules, "Rules");
                sections.push(format!("# Agent Rules\n{}", formatted));
            }
        }
    }

    // 7. Tool notes
    if let Some(ref agent) = ctx.agent {
        if let Some(ref notes) = agent.tool_notes {
            if !notes.is_empty() {
                let formatted = format_structured_or_raw(notes, "Tool Notes");
                sections.push(format!("# Tool Notes\n{}", formatted));
            }
        }
    }

    // 8. What You Know (scored tacit memories)
    if !ctx.tacit_memories.is_empty() {
        let mut lines = Vec::new();
        for sm in &ctx.tacit_memories {
            lines.push(format!("- {}: {}", sm.memory.key, sm.memory.value));
        }
        sections.push(format!("# What You Know\n{}", lines.join("\n")));
    }

    // 9. Memory tool instructions
    sections.push(
        "# Memory Instructions\n\
         You have persistent memory. Facts are automatically extracted from conversations.\n\
         Use agent(resource: memory, action: search) to find memories.\n\
         Use agent(resource: memory, action: recall, key: \"...\") for specific facts.\n\
         Only use explicit store when the user says \"remember this\"."
            .to_string(),
    );

    let mut result = sections.join("\n\n---\n\n");
    result = result.replace("{agent_name}", agent_name);
    result
}

/// Search for memories relevant to the user's current prompt using FTS.
/// Returns a formatted string to append to the system prompt, excluding
/// memories already present in the tacit_memories list.
pub fn load_prompt_relevant_memories(
    store: &Store,
    user_id: &str,
    prompt: &str,
    existing_memory_ids: &HashSet<i64>,
) -> String {
    if prompt.is_empty() {
        return String::new();
    }

    // FTS search against memories table
    let fts_results = match store.search_memories_fts(prompt, user_id, 10) {
        Ok(results) => results,
        Err(_) => return String::new(),
    };

    // Fetch full memories, filtering out duplicates
    let mut lines = Vec::new();
    for (mem_id, _rank) in fts_results {
        if existing_memory_ids.contains(&mem_id) {
            continue;
        }
        if let Ok(Some(mem)) = store.get_memory(mem_id) {
            lines.push(format!("- {}: {}", mem.key, mem.value));
            if lines.len() >= 5 {
                break;
            }
        }
    }

    if lines.is_empty() {
        return String::new();
    }

    debug!(count = lines.len(), "injected prompt-relevant memories");
    format!("\n\n## Relevant to This Conversation\n{}", lines.join("\n"))
}

/// Try to parse as JSON array of strings and format as markdown list,
/// otherwise return the raw text.
fn format_structured_or_raw(text: &str, _label: &str) -> String {
    // Try JSON array of strings
    if let Ok(items) = serde_json::from_str::<Vec<String>>(text) {
        return items
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Try JSON array of objects with "text" or "rule" field
    if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
        let lines: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .or_else(|| item.get("rule"))
                    .or_else(|| item.get("note"))
                    .and_then(|v| v.as_str())
                    .map(|s| format!("- {}", s))
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }

    // Raw text fallback
    text.to_string()
}

/// Map language code to display name for the system prompt.
fn language_display_name(code: &str) -> &'static str {
    match code {
        "de" => "German (Deutsch)",
        "es" => "Spanish (Español)",
        "fr" => "French (Français)",
        "it" => "Italian (Italiano)",
        "pt-BR" => "Brazilian Portuguese (Português do Brasil)",
        "nl" => "Dutch (Nederlands)",
        "pl" => "Polish (Polski)",
        "tr" => "Turkish (Türkçe)",
        "uk" => "Ukrainian (Українська)",
        "vi" => "Vietnamese (Tiếng Việt)",
        "ar" => "Arabic (العربية)",
        "hi" => "Hindi (हिन्दी)",
        "ja" => "Japanese (日本語)",
        "ko" => "Korean (한국어)",
        "zh-CN" => "Simplified Chinese (简体中文)",
        "zh-TW" => "Traditional Chinese (繁體中文)",
        _ => "English",
    }
}

/// Map personality preset names to prompt text.
fn personality_preset_prompt(preset: Option<&str>) -> Option<&'static str> {
    match preset? {
        "professional" => Some("You are professional, precise, and efficient. You focus on accuracy and clear communication."),
        "friendly" => Some("You are warm, friendly, and approachable. You make people feel comfortable and supported."),
        "casual" => Some("You are laid-back and casual. You keep things light and conversational."),
        "creative" => Some("You are creative, imaginative, and expressive. You bring fresh perspectives and ideas."),
        "analytical" => Some("You are methodical, detail-oriented, and data-driven. You think critically and provide thorough analysis."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_context() {
        let ctx = DBContext {
            agent: None,
            user: None,
            preferences: None,
            personality_directive: None,
            tacit_memories: vec![],
        };
        let result = format_for_system_prompt(&ctx, "Nebo");
        assert!(result.contains("Memory Instructions"));
    }

    #[test]
    fn test_format_with_agent_profile() {
        let agent = AgentProfile {
            id: 1,
            name: "TestBot".to_string(),
            personality_preset: Some("friendly".to_string()),
            custom_personality: None,
            voice_style: Some("warm".to_string()),
            response_length: Some("medium".to_string()),
            emoji_usage: Some("moderate".to_string()),
            formality: Some("casual".to_string()),
            proactivity: None,
            created_at: 0,
            updated_at: 0,
            emoji: Some("🤖".to_string()),
            creature: Some("robot".to_string()),
            vibe: Some("chill".to_string()),
            avatar: None,
            agent_rules: None,
            tool_notes: None,
            role: Some("assistant".to_string()),
            quiet_hours_start: "22:00".to_string(),
            quiet_hours_end: "08:00".to_string(),
        };

        let ctx = DBContext {
            agent: Some(agent),
            user: None,
            preferences: None,
            personality_directive: None,
            tacit_memories: vec![],
        };

        let result = format_for_system_prompt(&ctx, "TestBot");
        assert!(result.contains("Identity"));
        assert!(result.contains("friendly"));
        assert!(result.contains("Character"));
        assert!(result.contains("robot"));
        assert!(result.contains("Communication Style"));
        assert!(result.contains("warm"));
    }

    #[test]
    fn test_format_with_user_profile() {
        let user = UserProfile {
            user_id: "u1".to_string(),
            display_name: Some("Alice".to_string()),
            bio: None,
            location: Some("NYC".to_string()),
            timezone: Some("America/New_York".to_string()),
            occupation: Some("Engineer".to_string()),
            interests: Some("coding, hiking".to_string()),
            communication_style: None,
            goals: Some("Build cool stuff".to_string()),
            context: None,
            onboarding_completed: None,
            onboarding_step: None,
            created_at: 0,
            updated_at: 0,
            tool_permissions: None,
            terms_accepted_at: None,
        };

        let ctx = DBContext {
            agent: None,
            user: Some(user),
            preferences: None,
            personality_directive: None,
            tacit_memories: vec![],
        };

        let result = format_for_system_prompt(&ctx, "Nebo");
        assert!(result.contains("User Information"));
        assert!(result.contains("Alice"));
        assert!(result.contains("NYC"));
        assert!(result.contains("Engineer"));
    }

    #[test]
    fn test_format_with_personality_directive() {
        let ctx = DBContext {
            agent: None,
            user: None,
            preferences: None,
            personality_directive: Some("Be concise and direct.".to_string()),
            tacit_memories: vec![],
        };

        let result = format_for_system_prompt(&ctx, "Nebo");
        assert!(result.contains("Personality (Learned)"));
        assert!(result.contains("Be concise and direct."));
    }

    #[test]
    fn test_format_with_memories() {
        let mem = db::models::Memory {
            id: 1,
            namespace: "tacit/preferences".to_string(),
            key: "favorite-color".to_string(),
            value: "blue".to_string(),
            tags: None,
            metadata: None,
            created_at: None,
            updated_at: None,
            accessed_at: None,
            access_count: Some(1),
            user_id: "u1".to_string(),
        };

        let ctx = DBContext {
            agent: None,
            user: None,
            preferences: None,
            personality_directive: None,
            tacit_memories: vec![ScoredMemory {
                memory: mem,
                score: 1.0,
            }],
        };

        let result = format_for_system_prompt(&ctx, "Nebo");
        assert!(result.contains("What You Know"));
        assert!(result.contains("favorite-color: blue"));
    }

    #[test]
    fn test_format_structured_json_array() {
        let json = r#"["Rule one", "Rule two", "Rule three"]"#;
        let result = format_structured_or_raw(json, "Rules");
        assert!(result.contains("- Rule one"));
        assert!(result.contains("- Rule two"));
    }

    #[test]
    fn test_format_structured_json_objects() {
        let json = r#"[{"text": "Do this"}, {"text": "Do that"}]"#;
        let result = format_structured_or_raw(json, "Rules");
        assert!(result.contains("- Do this"));
        assert!(result.contains("- Do that"));
    }

    #[test]
    fn test_format_raw_fallback() {
        let text = "Just plain text rules";
        let result = format_structured_or_raw(text, "Rules");
        assert_eq!(result, "Just plain text rules");
    }

    #[test]
    fn test_agent_name_replacement() {
        let agent = AgentProfile {
            id: 1,
            name: "Nebo".to_string(),
            personality_preset: None,
            custom_personality: Some("You are {agent_name}, a helpful bot.".to_string()),
            voice_style: None,
            response_length: None,
            emoji_usage: None,
            formality: None,
            proactivity: None,
            created_at: 0,
            updated_at: 0,
            emoji: None,
            creature: None,
            vibe: None,
            avatar: None,
            agent_rules: None,
            tool_notes: None,
            role: None,
            quiet_hours_start: "22:00".to_string(),
            quiet_hours_end: "08:00".to_string(),
        };

        let ctx = DBContext {
            agent: Some(agent),
            user: None,
            preferences: None,
            personality_directive: None,
            tacit_memories: vec![],
        };

        let result = format_for_system_prompt(&ctx, "Nebo");
        assert!(result.contains("You are Nebo, a helpful bot."));
        assert!(!result.contains("{agent_name}"));
    }
}
