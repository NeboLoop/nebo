//! `rules` — the ONE way a seat writes the package part of its Rules.
//!
//! A seat meets its industry, franchise, and company packages by reading
//! them once per change in an update run and writing what matters to its
//! job into its own Rules — the same editable Rules the owner sees and
//! edits in the employee's settings. The seat's part sits between two
//! markers; everything outside them is the owner's and is kept as written.
//! Rules are resident in the seat's static prompt; nothing about the
//! packages is fetched at work time. This tool is the write half. The read
//! half is the prompt.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent_tool::AgentRegistry;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const PACKAGES_START: &str = "<!-- from the industry and company packages: written by this employee, edit freely; the next package review rewrites this part -->";
pub const PACKAGES_END: &str = "<!-- end of the package part -->";

/// The seat's package part of its Rules, if it has written one.
pub fn package_block(rules: &str) -> Option<&str> {
    let start = rules.find(PACKAGES_START)? + PACKAGES_START.len();
    let end = start + rules[start..].find(PACKAGES_END)?;
    Some(rules[start..end].trim())
}

/// Rules with the package part replaced by `section`, everything the owner
/// wrote outside the markers kept where it was. A first write appends.
pub fn merge_package_block(rules: &str, section: &str) -> String {
    let block = format!("{PACKAGES_START}\n{}\n{PACKAGES_END}", section.trim());
    match (rules.find(PACKAGES_START), rules.find(PACKAGES_END)) {
        (Some(s), Some(e)) if e > s => {
            let before = rules[..s].trim_end();
            let after = rules[e + PACKAGES_END.len()..].trim_start();
            let mut out = String::new();
            if !before.is_empty() {
                out.push_str(before);
                out.push_str("\n\n");
            }
            out.push_str(&block);
            if !after.is_empty() {
                out.push_str("\n\n");
                out.push_str(after);
            }
            out
        }
        _ => {
            let own = rules.trim();
            if own.is_empty() { block } else { format!("{own}\n\n{block}") }
        }
    }
}

pub struct RulesTool {
    store: Arc<db::Store>,
    /// The live registry the runner reads; updated in place so the next turn
    /// carries the new section without a restart.
    agent_registry: Option<AgentRegistry>,
}

impl RulesTool {
    pub fn new(store: Arc<db::Store>, agent_registry: Option<AgentRegistry>) -> Self {
        Self { store, agent_registry }
    }
}

impl DynTool for RulesTool {
    fn name(&self) -> &str {
        "rules"
    }

    fn description(&self) -> String {
        "The package part of your Rules: what you know about this company and its trade, written by you from the industry, franchise, and company packages. \
         `write` replaces that part of your Rules and keeps everything the owner wrote outside it (call it at the end of an update run, once, with the whole part). `show` returns the current part and what it was written against."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["write", "show"] },
                "section": {
                    "type": "string",
                    "description": "The whole package part, in your own words: the facts and rules you will work by. Markdown. Only for `write`."
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        if agent_id.is_empty() {
            return ToolResult::error("rules: this session is not an employee's");
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("show");
        match action {
            "show" => match self.store.get_agent(&agent_id) {
                Ok(Some(a)) => ToolResult::ok(
                    json!({
                        "section": a.rules.as_deref().and_then(package_block).unwrap_or_default(),
                        "stamp": a.context_stamp.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
                    })
                    .to_string(),
                ),
                Ok(None) => ToolResult::error("rules: employee not found"),
                Err(e) => ToolResult::error(format!("rules: {e}")),
            },
            "write" => {
                let section = input.get("section").and_then(|v| v.as_str()).unwrap_or("").trim();
                if section.is_empty() {
                    return ToolResult::error("rules write: `section` is required and must not be empty");
                }
                let (rules, stamp) = match self.store.get_agent(&agent_id) {
                    Ok(Some(a)) => (
                        a.rules.unwrap_or_default(),
                        a.context_stamp
                            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                            .unwrap_or_else(|| json!({})),
                    ),
                    Ok(None) => return ToolResult::error("rules: employee not found"),
                    Err(e) => return ToolResult::error(format!("rules: {e}")),
                };
                let merged = merge_package_block(&rules, section);
                let mut stamp = stamp;
                stamp["status"] = json!("written");
                stamp["written_at"] = json!(chrono::Utc::now().timestamp());
                stamp["written_in"] = json!(ctx.session_key);
                if let Err(e) = self.store.set_agent_rules_from_packages(&agent_id, &merged, &stamp.to_string()) {
                    return ToolResult::error(format!("rules write: {e}"));
                }
                if let Some(reg) = &self.agent_registry {
                    if let Some(entry) = reg.write().await.get_mut(&agent_id) {
                        entry.rules = Some(merged.clone());
                    }
                }
                ToolResult::ok(format!(
                    "The package part of your Rules is written ({} chars). It is in your instructions from your next turn.",
                    section.chars().count()
                ))
            }
            other => ToolResult::error(format!("rules: unknown action `{other}`")),
        }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_write_appends_below_the_owners_rules_and_a_rewrite_replaces_only_the_block() {
        let own = "# Rules\n- Never send without approval";
        let first = merge_package_block(own, "- Carrier scope before supplement");
        assert!(first.starts_with(own), "the owner's lines stay first: {first}");
        assert_eq!(package_block(&first), Some("- Carrier scope before supplement"));

        let edited = format!("{first}\n\n- Owner added this after");
        let second = merge_package_block(&edited, "- New: mortgagee endorsement");
        assert!(second.starts_with(own));
        assert!(second.ends_with("- Owner added this after"), "text after the block is kept: {second}");
        assert_eq!(package_block(&second), Some("- New: mortgagee endorsement"));
        assert!(!second.contains("Carrier scope"), "the old package part is gone");
        assert_eq!(second.matches(PACKAGES_START).count(), 1);
    }

    #[test]
    fn no_rules_yet_means_the_block_alone_and_no_block_means_none() {
        let only = merge_package_block("", "- a");
        assert!(only.starts_with(PACKAGES_START));
        assert_eq!(package_block(&only), Some("- a"));
        assert_eq!(package_block("# Rules\n- mine"), None);
    }
}
