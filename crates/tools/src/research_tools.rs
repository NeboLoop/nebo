//! Research: `deep_research` runs the deterministic deep-research harness
//! (scope, search, fetch, adversarial verification, a cited report);
//! `quick_research` starts a research run the employee leads with helpers;
//! `submit_findings` is how those helpers hand their findings back.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::bot_tool::StructuredAgent;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// A deep research run legitimately takes many minutes; the runner's
/// default tool budget killed every standard and deep run mid-flight.
const DEEP_RESEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(60 * 60);

const DEPTHS: [&str; 3] = ["quick", "standard", "deep"];

pub struct Research {
    agent: Option<Arc<dyn StructuredAgent>>,
}

impl Research {
    pub fn new(agent: Option<Arc<dyn StructuredAgent>>) -> Self {
        Self { agent }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let research = Arc::new(self);
        [
            ResearchOp::Deep,
            ResearchOp::Quick,
            ResearchOp::SubmitFindings,
        ]
        .into_iter()
        .map(|op| {
            Box::new(ResearchTool {
                op,
                research: research.clone(),
            }) as Box<dyn DynTool>
        })
        .collect()
    }

    fn quick(&self, input: &Value) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let data_dir = match config::data_dir() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Cannot determine data dir: {e}")),
        };
        let run_id = format!("research-{}", uuid::Uuid::new_v4().as_simple());
        match crate::research::create_run_dir(&data_dir, &run_id, query) {
            Ok(run_dir) => ToolResult::ok(format!(
                "Research mode active. Run ID: {run_id}. Dir: {}.\n\n{}",
                run_dir.display(),
                crate::research::RESEARCH_LEAD_PROMPT,
            )),
            Err(e) => ToolResult::error(format!("Failed to create research dir: {e}")),
        }
    }

    fn submit_findings(&self, input: &Value) -> ToolResult {
        let subtask_id = input["subtask_id"].as_str().unwrap_or("");
        let findings: Vec<crate::research::Finding> = input["findings"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|f| crate::research::Finding {
                        claim: f["claim"].as_str().unwrap_or("").to_string(),
                        source_url: f["source_url"].as_str().unwrap_or("").to_string(),
                        source_ref: f["source_ref"].as_str().unwrap_or("").to_string(),
                        confidence: f["confidence"].as_f64().unwrap_or(0.5) as f32,
                        quote: f["quote"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let gaps: Vec<String> = input["gaps"]
            .as_array()
            .map(|g| {
                g.iter()
                    .filter_map(|g| g.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let worker_findings = crate::research::WorkerFindings {
            subtask_id: subtask_id.to_string(),
            findings,
            gaps,
        };

        // The run this worker belongs to: the research directory's run in
        // status "running".
        let data_dir = match config::data_dir() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Cannot determine data dir: {e}")),
        };
        let research_dir = data_dir.join("research");
        let Some(run_dir) = crate::research::find_active_run_dir(&research_dir) else {
            return ToolResult::error(format!(
                "No research run is currently in status 'running' under {}; the run this worker \
                 belongs to has ended or was not started. The lead starts one with \
                 quick_research(query: \"...\").",
                research_dir.display()
            ));
        };
        match crate::research::write_worker_findings(&run_dir, &worker_findings) {
            Ok(()) => ToolResult::ok(format!(
                "Findings submitted. {} claims, {} gaps.",
                worker_findings.findings.len(),
                worker_findings.gaps.len()
            )),
            Err(e) => ToolResult::error(format!("Failed to write findings: {e}")),
        }
    }

    async fn deep(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("").trim();
        let agent = match &self.agent {
            Some(a) => a.clone(),
            None => {
                return ToolResult::error(
                    "Deep research is unavailable: no structured-output-capable AI provider is configured. \
                     Configure a provider (Anthropic/OpenAI/Gemini/Janus) and retry.",
                );
            }
        };

        let depth = input["depth"].as_str().unwrap_or("standard");
        let cfg = crate::deep_research::Config::for_depth(depth);

        let data_dir = match config::data_dir() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Cannot determine data dir: {}", e)),
        };
        let run_id = format!("research-{}", uuid::Uuid::new_v4().as_simple());

        // ── Scope checkpoint: decompose the question, then confirm the plan with the user
        // before the (slow, credit-costly) fan-out. Only gate INTERACTIVE (User-origin) runs:
        // automations/cron/channels/sub-agents (and an explicit `confirm: false`) proceed
        // without prompting — an automation must never block on a UI button.
        let confirm_flag = input["confirm"].as_bool().unwrap_or(true);
        let confirm_gate = matches!(ctx.origin, crate::origin::Origin::User) && confirm_flag;
        let scope = crate::deep_research::scope(&agent, query, &cfg).await;
        let angle_labels: Vec<&str> = scope.angles.iter().map(|a| a.label.as_str()).collect();
        let mut plan = format!(
            "I'll research \"{}\" across {} angles at **{}** depth — a verified multi-source \
             search that takes a couple of minutes.\n\nAngles: {}.",
            query,
            scope.angles.len(),
            depth,
            angle_labels.join(", "),
        );
        if scope.clarifying_questions.is_empty() {
            plan.push_str("\n\nStart the research, or refine the plan first?");
        } else {
            plan.push_str("\n\nA few details would sharpen this:");
            for q in &scope.clarifying_questions {
                plan.push_str(&format!("\n• {q}"));
            }
            plan.push_str("\n\nStart as-is, refine the plan, or cancel?");
        }
        let mut gate_timed_out = false;
        if confirm_gate {
            let widgets = serde_json::json!([{ "type": "buttons", "options": ["Start research", "Refine the plan", "Cancel"] }]);
            // Bounded gate: an unanswered plan must NEVER become a tool-timeout
            // error (observed live: 300s timeout → the model 'tried different
            // approaches' in a retry spiral, burning balance). No answer in 90s
            // → start as planned; the run is cancelable and the plan is visible.
            let gate = match tokio::time::timeout(
                std::time::Duration::from_secs(90),
                ctx.ask_user(&plan, widgets),
            )
            .await
            {
                Ok(answer) => answer,
                Err(_) => {
                    // timeout → start as planned, and the final report says so
                    gate_timed_out = true;
                    None
                }
            };
            if let Some(resp) = gate {
                let r = resp.to_lowercase();
                if r.contains("cancel") {
                    return ToolResult::ok(format!(
                        "Research not started. I was going to cover: {}. Add any constraints \
                         (budget, region, use-case, time window) and ask again.",
                        angle_labels.join(", ")
                    ));
                }
                if r.contains("refine") {
                    // Hand control back to the conversation: the agent asks what to change,
                    // then re-invokes deep_research with the refinement folded into the query.
                    return ToolResult::ok(format!(
                        "The user wants to adjust this research plan before running — do NOT start \
                         the research yet. Ask them what to change: narrow or broaden the topic, \
                         add or drop an angle, or change depth (quick/standard/deep). Then call \
                         deep_research again with their changes folded into the query. The current \
                         plan was {} angles ({}) at {} depth.",
                        scope.angles.len(),
                        angle_labels.join(", "),
                        depth
                    ));
                }
            }
        }

        // Pre-compute the run's report path + a unique, readable Work-panel filename
        // (the harness writes a generic report.md per run-dir, which would collide in files/).
        let report_src = data_dir.join("research").join(&run_id).join("report.md");
        let files_dir = data_dir.join("files");
        let short = run_id.rsplit('-').next().unwrap_or(&run_id);
        let work_name = format!(
            "{}-{}.md",
            research_slug(query),
            &short[..short.len().min(8)]
        );

        let research_started = std::time::Instant::now();
        match crate::deep_research::run(
            agent,
            data_dir,
            run_id,
            query.to_string(),
            cfg,
            ctx.cancel_token.clone(),
            ctx.stream_tx.clone(),
            Some(scope),
        )
        .await
        {
            Ok(report) => {
                let mut text = crate::deep_research::format_report(&report);
                if gate_timed_out {
                    text = format!(
                        "(No answer to the plan prompt within 90s; research started as planned.)\n\n{}",
                        text
                    );
                }
                let mut result = ToolResult::ok(text)
                    // Final card state for the live stream AND reloaded history.
                    .with_payload(serde_json::json!({
                        "kind": "research_summary",
                        "question": report.question,
                        "sources_read": report.stats.sources_fetched,
                        "results_found": report.stats.claims_extracted,
                        "claims_verified": report.stats.confirmed,
                        "elapsed_ms": research_started.elapsed().as_millis() as u64,
                    }));
                // Surface the report in the Work panel under its unique name.
                if report_src.exists() {
                    let _ = std::fs::create_dir_all(&files_dir);
                    let dest = files_dir.join(&work_name);
                    if std::fs::copy(&report_src, &dest).is_ok() {
                        result = result.with_image_url(dest.to_string_lossy().to_string());
                    }
                }
                result
            }
            Err(e) => ToolResult::error(format!("Deep research failed: {}", e)),
        }
    }
}

/// Slugify a research question into a readable filename stem (lowercase, alnum + single
/// dashes, capped). Falls back to "research" if the question has no usable characters.
fn research_slug(question: &str) -> String {
    let mut out = String::new();
    for c in question.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() {
        "research".to_string()
    } else {
        slug
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResearchOp {
    Deep,
    Quick,
    SubmitFindings,
}

struct ResearchTool {
    op: ResearchOp,
    research: Arc<Research>,
}

impl DynTool for ResearchTool {
    fn name(&self) -> &str {
        match self.op {
            ResearchOp::Deep => "deep_research",
            ResearchOp::Quick => "quick_research",
            ResearchOp::SubmitFindings => "submit_findings",
        }
    }

    fn description(&self) -> String {
        match self.op {
            ResearchOp::Deep => "Researches a question in depth and returns a cited report: it searches from several angles, reads the sources, fact-checks each claim against the others, then writes the report.\n\
                 - For a fact-checked, multi-source answer. For one quick lookup, search the web directly.\n\
                 - `depth`: quick, standard (default) or deep. It takes minutes.\n\
                 - In a chat with the owner it shows the plan first; `confirm: false` skips that (unattended work)."
                .to_string(),
            ResearchOp::Quick => "Starts a research run you lead: you split the question into subtasks, start a helper for each, and write the report from their findings. Returns the run's folder and the method to follow.".to_string(),
            ResearchOp::SubmitFindings => "Hands a research helper's findings back to the research run it works for: each finding is a claim with its source, and gaps are questions it couldn't answer.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            ResearchOp::Deep => json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The question, with any constraints (region, time window, use case)." },
                    "depth": { "type": "string", "enum": DEPTHS, "description": "How far to go (default standard)." },
                    "confirm": { "type": "boolean", "description": "Show the owner the plan first (default true in a chat)." }
                },
                "required": ["query"]
            }),
            ResearchOp::Quick => json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "The question to research." } },
                "required": ["query"]
            }),
            ResearchOp::SubmitFindings => json!({
                "type": "object",
                "properties": {
                    "subtask_id": { "type": "string", "description": "The subtask id from your task." },
                    "findings": {
                        "type": "array",
                        "description": "What you found, one claim each.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "claim": { "type": "string" },
                                "source_url": { "type": "string" },
                                "source_ref": { "type": "string" },
                                "confidence": { "type": "number" },
                                "quote": { "type": "string" }
                            },
                            "required": ["claim"]
                        }
                    },
                    "gaps": { "type": "array", "items": { "type": "string" }, "description": "Questions you couldn't answer." }
                },
                "required": ["subtask_id", "findings"]
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            ResearchOp::Deep => "cited fact-checked research report",
            ResearchOp::Quick => "lead a research run with helpers",
            ResearchOp::SubmitFindings => "hand research findings back",
        }
    }

    /// Research is the employee's own work.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        types::permissions::CallEffects::none()
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let key = if self.op == ResearchOp::SubmitFindings {
            "subtask_id"
        } else {
            "query"
        };
        if input[key].as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err(format!("{key} can't be empty."));
        }
        if let Some(i) = input["findings"].as_array().and_then(|f| {
            f.iter()
                .position(|f| f["claim"].as_str().is_none_or(|c| c.trim().is_empty()))
        }) {
            return Err(format!("findings[{i}].claim is empty."));
        }
        Ok(())
    }

    /// Models routinely send `confirm` as the string "false" (observed
    /// live): its string forms mean the boolean.
    fn normalize_input(&self, mut input: Value) -> Value {
        if self.op == ResearchOp::Deep
            && let Some(s) = input["confirm"].as_str()
        {
            input["confirm"] = Value::Bool(!s.eq_ignore_ascii_case("false"));
        }
        input
    }

    fn execution_timeout(&self, _input: &Value) -> Option<std::time::Duration> {
        (self.op == ResearchOp::Deep).then_some(DEEP_RESEARCH_BUDGET)
    }

    fn activity(&self, input: &Value) -> String {
        match self.op {
            ResearchOp::Deep | ResearchOp::Quick => {
                format!("researching {}", input["query"].as_str().unwrap_or(""))
            }
            ResearchOp::SubmitFindings => "handing back findings".to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        match self.op {
            ResearchOp::Deep | ResearchOp::Quick => {
                format!("Researched {}", input["query"].as_str().unwrap_or(""))
            }
            ResearchOp::SubmitFindings => "Handed back findings".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                ResearchOp::Deep => self.research.deep(&input, ctx).await,
                ResearchOp::Quick => self.research.quick(&input),
                ResearchOp::SubmitFindings => self.research.submit_findings(&input),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deep_research_without_a_provider_says_why_and_has_its_budget() {
        let tools = Research::new(None).tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["deep_research", "quick_research", "submit_findings"]
        );
        assert_eq!(
            tools[0].execution_timeout(&json!({})),
            Some(DEEP_RESEARCH_BUDGET)
        );
        assert_eq!(tools[1].execution_timeout(&json!({})), None);
        let r = tools[0]
            .execute_dyn(&ToolContext::default(), json!({"query": "q"}))
            .await;
        assert!(
            r.is_error && r.content.contains("no structured-output-capable"),
            "{}",
            r.content
        );
        assert!(
            tools[2]
                .validate_input(&json!({"subtask_id": "s", "findings": [{"claim": ""}]}))
                .is_err()
        );
    }

    #[test]
    fn a_question_becomes_a_readable_file_stem() {
        assert_eq!(
            research_slug("How effective is X, for Y?"),
            "how-effective-is-x-for-y"
        );
        assert_eq!(research_slug("???"), "research");
    }
}
