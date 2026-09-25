//! Research: `deep_research` runs the deterministic deep-research harness
//! (scope, search, fetch, adversarial verification, a cited report);
//! `quick_research` starts a research run the employee leads with helpers;
//! `submit_findings` is how those helpers hand their findings back.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::bot_tool::StructuredAgent;
use crate::orchestrator::{OrchestratorHandle, SpawnRequest};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

const DEPTHS: [&str; 3] = ["quick", "standard", "deep"];

pub struct Research {
    agent: Option<Arc<dyn StructuredAgent>>,
    /// The helper registry's door: a deep research run is one of the
    /// caller's background helpers.
    helpers: OrchestratorHandle,
}

impl Research {
    pub fn new(agent: Option<Arc<dyn StructuredAgent>>, helpers: OrchestratorHandle) -> Self {
        Self { agent, helpers }
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

    /// Start a deep research run in the background, as one of the caller's
    /// helpers, and return its receipt at once. The run takes minutes; the
    /// cited report comes back as the helper's notification and lands in the
    /// Work panel. Like any long work, it never holds the conversation
    /// (Claude Code runs long work as a background agent that notifies,
    /// `AgentTool.tsx` / `LocalAgentTask.tsx`), and the owner's Stop reaches it.
    async fn deep(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("").trim().to_string();
        let Some(agent) = self.agent.clone() else {
            return ToolResult::error(
                "Deep research is unavailable: no structured-output-capable AI provider is configured. \
                 Configure a provider (Anthropic/OpenAI/Gemini/Janus) and retry.",
            );
        };
        let Some(helpers) = self.helpers.get() else {
            return ToolResult::error(
                "Research can't start yet: the server is still starting. Try again in a moment.",
            );
        };
        let depth = input["depth"].as_str().unwrap_or("standard");
        let cfg = crate::deep_research::Config::for_depth(depth);
        let data_dir = match config::data_dir() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Cannot determine data dir: {}", e)),
        };
        let run_id = format!("research-{}", uuid::Uuid::new_v4().as_simple());
        let short = run_id.rsplit('-').next().unwrap_or(&run_id);
        let work_name = format!("{}-{}.md", research_slug(&query), &short[..short.len().min(8)]);
        let question = query.clone();
        let work: crate::orchestrator::Work = Box::new(move |cancel, progress, session_key| {
            Box::pin(async move {
                let report_src = data_dir.join("research").join(&run_id).join("report.md");
                let files_dir = data_dir.join("files");
                let started = std::time::Instant::now();
                let report = crate::deep_research::run(agent, data_dir, run_id, session_key, question, cfg, cancel, Some(progress.clone())).await?;
                // The research card's final state.
                let _ = progress
                    .send(ai::StreamEvent {
                        payload: Some(json!({
                            "kind": "research_progress",
                            "phase": "complete",
                            "complete": true,
                            "question": report.question,
                            "sources_read": report.stats.sources_fetched,
                            "results_found": report.stats.claims_extracted,
                            "claims_verified": report.stats.confirmed,
                            "elapsed_ms": started.elapsed().as_millis() as u64,
                        })),
                        ..ai::StreamEvent::text("")
                    })
                    .await;
                let mut text = crate::deep_research::format_report(&report);
                // The report in the Work panel, under a readable name (every
                // run writes a generic report.md in its own folder).
                if report_src.exists() {
                    let _ = std::fs::create_dir_all(&files_dir);
                    let dest = files_dir.join(&work_name);
                    if std::fs::copy(&report_src, &dest).is_ok() {
                        text.push_str(&format!("\nThe report is saved at {}.", dest.display()));
                    }
                }
                Ok(text)
            })
        });
        let req = SpawnRequest {
            description: format!("research: {}", short_question(&query)),
            prompt: query,
            wait: false,
            ..SpawnRequest::child_of(ctx)
        };
        match helpers.start_work(req, work).await {
            // The research card follows the run by its id.
            Ok(r) => ToolResult::ok(r.output).with_payload(json!({ "kind": "research_run", "task_id": r.task_id })),
            Err(e) => ToolResult::error(format!("Couldn't start the research: {e}")),
        }
    }
}

/// A question cut for the owner's activity line.
fn short_question(q: &str) -> String {
    if q.chars().count() > 60 {
        format!("{}…", q.chars().take(60).collect::<String>())
    } else {
        q.to_string()
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
            ResearchOp::Deep => "Researches a question in depth in the background and reports a cited report: it searches from several angles, reads the sources, fact-checks each claim against the others, then writes the report.\n\
                 - For a fact-checked, multi-source answer. For one quick lookup, search the web directly.\n\
                 - `depth`: quick, standard (default) or deep. It takes minutes; you're notified with the report when it ends."
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
                    "depth": { "type": "string", "enum": DEPTHS, "description": "How far to go (default standard)." }
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

    /// A structured agent the tool never reaches: the research runs in the
    /// background, not in the call.
    struct Unused;
    impl StructuredAgent for Unused {
        fn run<'a>(&'a self, _: crate::bot_tool::StructuredTask, _: Option<Arc<std::sync::atomic::AtomicU64>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
            panic!("the pipeline ran inside the call")
        }
        fn execute_tool<'a>(&'a self, _: String, _: String, _: Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
            panic!("the pipeline ran inside the call")
        }
        fn close_tab<'a>(&'a self, _: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    /// Records the background work it was handed.
    #[derive(Default)]
    struct Door(std::sync::Mutex<Vec<SpawnRequest>>);
    type Fut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
    impl crate::orchestrator::SubAgentOrchestrator for Arc<Door> {
        fn spawn(&self, _: SpawnRequest) -> Fut<'_, Result<crate::orchestrator::SpawnResult, String>> {
            panic!("research is background work, not a model helper")
        }
        fn start_work(&self, req: SpawnRequest, _: crate::orchestrator::Work) -> Fut<'_, Result<crate::orchestrator::SpawnResult, String>> {
            self.0.lock().unwrap().push(req);
            Box::pin(async {
                Ok(crate::orchestrator::SpawnResult {
                    task_id: "h-1".into(),
                    success: true,
                    output: "Helper h-1 is working in the background.".into(),
                    error: None,
                    taint: Vec::new(),
                })
            })
        }
        fn cancel(&self, _: &str, _: &str) -> Fut<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self, _: &str, _: &str) -> Fut<'_, Result<String, String>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn send(&self, _: &str, _: &str, _: SpawnRequest) -> Fut<'_, Result<crate::orchestrator::FollowUp, String>> {
            Box::pin(async { Err("no".into()) })
        }
        fn list_active(&self, _: &str) -> Fut<'_, Vec<(String, String, String)>> {
            Box::pin(async { Vec::new() })
        }
        fn recover(&self) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    /// deep_research hands the run to the helper registry and returns its
    /// receipt: no plan card to wait on, no hour-long call, no budget of its
    /// own. Before: it ran the pipeline inside the call.
    #[tokio::test]
    async fn deep_research_starts_a_background_helper_and_returns_its_receipt() {
        let door = Arc::new(Door::default());
        let handle = crate::orchestrator::new_handle();
        let _ = handle.set(Box::new(door.clone()));
        let tools = Research::new(Some(Arc::new(Unused)), handle).tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["deep_research", "quick_research", "submit_findings"]);
        assert_eq!(tools[0].execution_timeout(&json!({})), None, "the call returns at once");
        let ctx = ToolContext { session_key: "agent:analyst:web".into(), ..ToolContext::new(crate::origin::Origin::User) };
        let r = tools[0].execute_dyn(&ctx, json!({"query": "Phoenix rents 2026", "depth": "quick"})).await;
        assert_eq!(r.content, "Helper h-1 is working in the background.", "the harness's receipt");
        let started = door.0.lock().unwrap();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].parent_session_key, "agent:analyst:web", "one of the caller's helpers");
        assert_eq!(started[0].description, "research: Phoenix rents 2026");
        assert!(!started[0].wait);
    }

    #[tokio::test]
    async fn deep_research_without_a_provider_says_why() {
        let tools = Research::new(None, crate::orchestrator::new_handle()).tools();
        let r = tools[0].execute_dyn(&ToolContext::default(), json!({"query": "q"})).await;
        assert!(r.is_error && r.content.contains("no structured-output-capable"), "{}", r.content);
        assert!(tools[2].validate_input(&json!({"subtask_id": "s", "findings": [{"claim": ""}]})).is_err());
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
