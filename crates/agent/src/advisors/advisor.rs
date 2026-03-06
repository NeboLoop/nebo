use serde::{Deserialize, Serialize};

/// An advisor parsed from an ADVISOR.md file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advisor {
    pub name: String,
    pub role: String,
    pub description: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub memory_access: bool,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i32,
    /// The markdown body (persona instructions — not in YAML).
    #[serde(skip)]
    pub persona: String,
    /// Filesystem path this advisor was loaded from.
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> i32 {
    30
}

impl Advisor {
    /// Validate that required fields are present.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("advisor name is required".into());
        }
        if self.role.is_empty() {
            return Err("advisor role is required".into());
        }
        Ok(())
    }

    /// Build the system prompt for this advisor when deliberating on a task.
    pub fn build_system_prompt(&self, task: &str) -> String {
        let mut prompt = String::new();

        // Advisor's persona
        if !self.persona.is_empty() {
            prompt.push_str(&self.persona);
        } else {
            prompt.push_str(&format!(
                "You are {}. Your role is: {}.",
                self.name, self.role
            ));
        }

        prompt.push_str("\n\n---\n\n## Current Task\n\n");
        prompt.push_str(task);
        prompt.push_str(
            "\n\n---\n\n## Response Format\n\n\
             Provide your analysis in a concise, structured format:\n\n\
             1. **Assessment**: Your main critique or observation (2-3 sentences)\n\
             2. **Confidence**: How confident are you in this assessment? (1-10)\n\
             3. **Risks**: What could go wrong? (optional, 1-2 sentences)\n\
             4. **Suggestion**: What action do you recommend? (optional, 1 sentence)\n\n\
             Be direct. No fluff. Focus on what matters.",
        );

        prompt
    }
}

/// Response from a single advisor after deliberation.
#[derive(Debug, Clone)]
pub struct Response {
    pub advisor_name: String,
    pub role: String,
    pub critique: String,
    pub confidence: i32,
    pub risks: String,
    pub suggestion: String,
}

impl Response {
    /// Extract confidence score (1-10) from the response text.
    pub fn extract_confidence(text: &str) -> i32 {
        // Look for "Confidence: X" or "**Confidence**: X" or "Confidence: X/10"
        for line in text.lines() {
            let stripped = line
                .replace("**", "")
                .replace('*', "");
            if stripped.trim_start().starts_with("Confidence") {
                // Find the first digit
                if let Some(num) = stripped.chars().find(|c| c.is_ascii_digit()) {
                    let val = num.to_digit(10).unwrap_or(5) as i32;
                    // Check for two-digit (10)
                    let idx = stripped.find(num).unwrap();
                    if idx + 1 < stripped.len() {
                        let next = stripped.as_bytes().get(idx + 1);
                        if next == Some(&b'0') && val == 1 {
                            return 10;
                        }
                    }
                    return val.clamp(1, 10);
                }
            }
        }
        5 // default
    }

    /// Extract a named section from the response text.
    pub fn extract_section(text: &str, section_name: &str) -> String {
        let lower = text.to_lowercase();
        let section_lower = section_name.to_lowercase();

        // Find the section header
        let patterns = [
            format!("**{}**:", section_lower),
            format!("**{}**", section_lower),
            format!("{}:", section_lower),
        ];

        for pattern in &patterns {
            if let Some(start) = lower.find(pattern.as_str()) {
                let after = start + pattern.len();
                let rest = &text[after..];
                // Take until the next section header or end
                let end = rest
                    .find("\n**")
                    .or_else(|| rest.find("\n## "))
                    .unwrap_or(rest.len());
                let section = rest[..end].trim();
                if !section.is_empty() {
                    return section.to_string();
                }
            }
        }

        String::new()
    }
}

/// Parse an ADVISOR.md file into an Advisor struct.
pub fn parse_advisor_md(data: &[u8]) -> Result<Advisor, String> {
    let (frontmatter, body) = tools::skills::split_frontmatter(data)?;

    let mut advisor: Advisor =
        serde_yaml::from_slice(&frontmatter).map_err(|e| format!("YAML parse error: {}", e))?;

    advisor.persona = String::from_utf8_lossy(&body).to_string();
    advisor.validate()?;
    Ok(advisor)
}

/// Convert a database Advisor row to our Advisor struct.
pub fn from_db(db_advisor: &db::models::Advisor) -> Advisor {
    Advisor {
        name: db_advisor.name.clone(),
        role: db_advisor.role.clone(),
        description: db_advisor.description.clone(),
        priority: db_advisor.priority as i32,
        enabled: db_advisor.enabled != 0,
        memory_access: db_advisor.memory_access != 0,
        timeout_seconds: db_advisor.timeout_seconds as i32,
        persona: db_advisor.persona.clone(),
        source_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ADVISOR: &str = r#"---
name: skeptic
role: critic
description: Challenges assumptions and identifies weaknesses
priority: 10
enabled: true
memory_access: false
timeout_seconds: 30
---

You are the Skeptic. Your role is to challenge ideas and find flaws.
Always question assumptions and look for what could go wrong.
Be constructive but unflinching in your analysis.
"#;

    #[test]
    fn test_parse_advisor_md() {
        let advisor = parse_advisor_md(SAMPLE_ADVISOR.as_bytes()).unwrap();
        assert_eq!(advisor.name, "skeptic");
        assert_eq!(advisor.role, "critic");
        assert_eq!(advisor.priority, 10);
        assert!(advisor.enabled);
        assert!(!advisor.memory_access);
        assert_eq!(advisor.timeout_seconds, 30);
        assert!(advisor.persona.contains("Skeptic"));
    }

    #[test]
    fn test_build_system_prompt() {
        let advisor = parse_advisor_md(SAMPLE_ADVISOR.as_bytes()).unwrap();
        let prompt = advisor.build_system_prompt("Should we use microservices?");
        assert!(prompt.contains("Skeptic"));
        assert!(prompt.contains("Should we use microservices?"));
        assert!(prompt.contains("Confidence"));
    }

    #[test]
    fn test_extract_confidence() {
        assert_eq!(
            Response::extract_confidence("**Confidence**: 8/10"),
            8
        );
        assert_eq!(
            Response::extract_confidence("Confidence: 3"),
            3
        );
        assert_eq!(
            Response::extract_confidence("**Confidence**: 10/10"),
            10
        );
        assert_eq!(
            Response::extract_confidence("No confidence line here"),
            5
        );
    }

    #[test]
    fn test_extract_section() {
        let text = "**Assessment**: This is a good approach.\n\n**Risks**: It might be too slow.\n\n**Suggestion**: Start with a prototype.";
        assert_eq!(
            Response::extract_section(text, "Assessment"),
            "This is a good approach."
        );
        assert_eq!(
            Response::extract_section(text, "Risks"),
            "It might be too slow."
        );
        assert_eq!(
            Response::extract_section(text, "Suggestion"),
            "Start with a prototype."
        );
        assert_eq!(
            Response::extract_section(text, "Missing"),
            ""
        );
    }
}
