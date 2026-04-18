//! Research mode: types, filesystem operations, and prompts.
//!
//! Everything lives here so bot_tool (same crate) can reference prompts directly.
//! The agent crate re-exports via `tools::research`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Prompts ────────────────────────────────────────────────────────

/// System prompt appended to the lead agent when research mode is active.
pub const RESEARCH_LEAD_PROMPT: &str = r#"
## Research Mode Active

You are now the lead researcher. Follow this methodology exactly:

### 1. Decompose
Break the user's query into 3-5 independent, focused subtasks. Each subtask should:
- Answer ONE specific question
- Be searchable via web search
- Not depend on other subtasks' results

Write your decomposition as a JSON array to `plan_initial.json` in the research directory using system(action: "write"). Format:
```json
[{"id": "subtask-slug", "question": "specific question to answer"}]
```

### 2. Spawn Workers
Call `bot(action: "spawn_parallel")` with one task per subtask. Each task prompt must include:
- The specific question to research
- The research directory path for saving sources
- Instructions to call `bot(action: "submit_findings")` when done

Set `max_iterations: 10` for each worker. Workers use web(action: "search") and web(action: "fetch") only — never navigate.

### 3. Evaluate Results
After workers complete, read their output files (`worker_<slug>.json`) from the research directory.
- If critical gaps remain and this is the first wave, spawn 1-3 targeted follow-up workers
- If gaps are minor or this is already a follow-up wave, proceed to synthesis
- Maximum 2 waves total

### 4. Synthesize
Write a comprehensive report combining all worker findings. For each factual claim, include an inline citation marker: `[REF:sources/src_<hash>.txt]` matching the worker's source_ref.

Present the report to the user. Then write the report to `report.md` in the research directory.

### Rules
- Stay in research mode until the report is delivered
- If all workers return empty, tell the user honestly what was tried and what failed
- Do not call bot(action: "research") from within research — you are already researching
"#;

/// Prompt prepended to each research worker's task.
pub const RESEARCH_WORKER_PROMPT: &str = r#"You are a research worker with a single focused task. Follow these rules:

1. Use ONLY `web(action: "search", query: "...")` to find sources and `web(action: "fetch", url: "...")` to read them. Never use navigate, desktop, system(shell), or any other tools.

2. For each useful source, save the raw content:
   - Compute a short hash from the URL (first 8 chars of hex-encoded hash)
   - Write to `{research_dir}/sources/src_<hash>.txt` with format: "URL: <url>\n\n<content>"

3. When you have enough information OR you've used 8+ iterations, stop and submit:
   Call `bot(action: "submit_findings", subtask_id: "{subtask_id}", findings: [...], gaps: [...])`

   Each finding: {"claim": "single sentence", "source_url": "https://...", "source_ref": "sources/src_<hash>.txt", "confidence": 0.0-1.0, "quote": "<=25 words verbatim"}

4. If a URL fails (403, 404, timeout), try a different source. Never retry the same URL.

5. If you can't find the information after a reasonable search, that's fine. Report what you found and list gaps honestly. Partial findings are better than no findings.

6. Stay focused on your assigned question. Do not research tangential topics.
"#;

/// Prompt for the citation pass sub-agent.
pub const CITATION_PROMPT: &str = "\
You are a citation formatter. You receive a research report draft and a findings index.\n\
\n\
Your job:\n\
1. Find every [REF:sources/src_<hash>.txt] marker in the draft\n\
2. Replace each with a numbered citation: [1], [2], etc.\n\
3. Append a Sources section at the end with numbered entries:\n\
   [1] <source_url>\n\
   [2] <source_url>\n\
\n\
Rules:\n\
- Preserve the report text exactly — only replace [REF:...] markers\n\
- If a [REF:...] doesn't match any source in the index, drop it silently\n\
- Number citations in order of first appearance\n\
- Output ONLY the final formatted report with sources section";

// ── Types ──────────────────────────────────────────────────────────

/// Status of a research run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Planning,
    Running,
    Synthesizing,
    Completed,
    Cancelled,
    Failed,
}

/// Metadata written to `meta.json` at run creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub query: String,
    pub status: RunStatus,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

/// A single finding from a research worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub claim: String,
    pub source_url: String,
    /// Relative path to source file: `sources/src_<hash>.txt`
    pub source_ref: String,
    pub confidence: f32,
    /// <=25 words verbatim from source
    pub quote: String,
}

/// Structured output from a research worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerFindings {
    pub subtask_id: String,
    pub findings: Vec<Finding>,
    pub gaps: Vec<String>,
}

impl WorkerFindings {
    /// Validate findings. Returns error message if invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.subtask_id.is_empty() {
            return Err("subtask_id is required".into());
        }
        for (i, f) in self.findings.iter().enumerate() {
            if f.claim.is_empty() {
                return Err(format!("finding[{}].claim is empty", i));
            }
            if f.source_url.is_empty() {
                return Err(format!("finding[{}].source_url is empty", i));
            }
        }
        Ok(())
    }
}

/// Create the research run directory and write meta.json.
/// Returns the run directory path.
pub fn create_run_dir(data_dir: &Path, run_id: &str, query: &str) -> Result<PathBuf, String> {
    let run_dir = data_dir.join("research").join(run_id);
    std::fs::create_dir_all(run_dir.join("sources"))
        .map_err(|e| format!("Failed to create research dir: {}", e))?;

    let meta = RunMeta {
        run_id: run_id.to_string(),
        query: query.to_string(),
        status: RunStatus::Planning,
        created_at: chrono::Utc::now().timestamp(),
        completed_at: None,
    };

    let meta_json = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("Failed to serialize meta: {}", e))?;
    std::fs::write(run_dir.join("meta.json"), meta_json)
        .map_err(|e| format!("Failed to write meta.json: {}", e))?;

    Ok(run_dir)
}

/// Update the status in meta.json.
pub fn update_run_status(run_dir: &Path, status: RunStatus) -> Result<(), String> {
    let meta_path = run_dir.join("meta.json");
    let content = std::fs::read_to_string(&meta_path)
        .map_err(|e| format!("Failed to read meta.json: {}", e))?;
    let mut meta: RunMeta = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse meta.json: {}", e))?;

    meta.status = status.clone();
    if matches!(status, RunStatus::Completed | RunStatus::Cancelled | RunStatus::Failed) {
        meta.completed_at = Some(chrono::Utc::now().timestamp());
    }

    let updated = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("Failed to serialize meta: {}", e))?;
    std::fs::write(&meta_path, updated)
        .map_err(|e| format!("Failed to write meta.json: {}", e))?;

    Ok(())
}

/// Write validated worker findings to disk.
pub fn write_worker_findings(run_dir: &Path, findings: &WorkerFindings) -> Result<(), String> {
    findings.validate()?;
    let filename = format!("worker_{}.json", findings.subtask_id);
    let json = serde_json::to_string_pretty(findings)
        .map_err(|e| format!("Failed to serialize findings: {}", e))?;
    std::fs::write(run_dir.join(&filename), json)
        .map_err(|e| format!("Failed to write {}: {}", filename, e))?;
    Ok(())
}

/// Find the most recently created research run directory with status "planning" or "running".
pub fn find_active_run_dir(research_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(research_dir).ok()?;
    let mut best: Option<(PathBuf, i64)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("meta.json");
        if let Ok(content) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<RunMeta>(&content) {
                if matches!(meta.status, RunStatus::Planning | RunStatus::Running) {
                    if best.as_ref().map_or(true, |(_, ts)| meta.created_at > *ts) {
                        best = Some((path, meta.created_at));
                    }
                }
            }
        }
    }

    best.map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_findings_valid() {
        let f = WorkerFindings {
            subtask_id: "cruise-pricing".into(),
            findings: vec![Finding {
                claim: "Repositioning cruises cost $50-100/day".into(),
                source_url: "https://example.com".into(),
                source_ref: "sources/src_abc12345.txt".into(),
                confidence: 0.85,
                quote: "prices range from fifty to one hundred dollars per day".into(),
            }],
            gaps: vec!["Could not find 2025 pricing".into()],
        };
        assert!(f.validate().is_ok());
    }

    #[test]
    fn test_worker_findings_empty_subtask_id() {
        let f = WorkerFindings {
            subtask_id: "".into(),
            findings: vec![],
            gaps: vec![],
        };
        assert!(f.validate().is_err());
        assert!(f.validate().unwrap_err().contains("subtask_id"));
    }

    #[test]
    fn test_worker_findings_empty_claim() {
        let f = WorkerFindings {
            subtask_id: "test".into(),
            findings: vec![Finding {
                claim: "".into(),
                source_url: "https://example.com".into(),
                source_ref: "sources/src_abc.txt".into(),
                confidence: 0.5,
                quote: "some quote".into(),
            }],
            gaps: vec![],
        };
        assert!(f.validate().is_err());
        assert!(f.validate().unwrap_err().contains("claim"));
    }

    #[test]
    fn test_worker_findings_empty_source_url() {
        let f = WorkerFindings {
            subtask_id: "test".into(),
            findings: vec![Finding {
                claim: "some claim".into(),
                source_url: "".into(),
                source_ref: "sources/src_abc.txt".into(),
                confidence: 0.5,
                quote: "some quote".into(),
            }],
            gaps: vec![],
        };
        assert!(f.validate().is_err());
        assert!(f.validate().unwrap_err().contains("source_url"));
    }

    #[test]
    fn test_run_status_serialization() {
        let status = RunStatus::Planning;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"planning\"");

        let deserialized: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, RunStatus::Planning);
    }

    #[test]
    fn test_create_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_run_dir(tmp.path(), "test-run-1", "best cruises").unwrap();

        assert!(run_dir.join("meta.json").exists());
        assert!(run_dir.join("sources").is_dir());

        let meta: RunMeta =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta.run_id, "test-run-1");
        assert_eq!(meta.query, "best cruises");
        assert_eq!(meta.status, RunStatus::Planning);
        assert!(meta.completed_at.is_none());
    }

    #[test]
    fn test_update_run_status() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_run_dir(tmp.path(), "test-run-2", "query").unwrap();

        update_run_status(&run_dir, RunStatus::Running).unwrap();
        let meta: RunMeta =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta.status, RunStatus::Running);
        assert!(meta.completed_at.is_none());

        update_run_status(&run_dir, RunStatus::Completed).unwrap();
        let meta: RunMeta =
            serde_json::from_str(&std::fs::read_to_string(run_dir.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta.status, RunStatus::Completed);
        assert!(meta.completed_at.is_some());
    }

    #[test]
    fn test_write_worker_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_run_dir(tmp.path(), "test-run-3", "query").unwrap();

        let findings = WorkerFindings {
            subtask_id: "cruise-pricing".into(),
            findings: vec![Finding {
                claim: "Cruises cost $50/day".into(),
                source_url: "https://example.com".into(),
                source_ref: "sources/src_abc12345.txt".into(),
                confidence: 0.9,
                quote: "fifty dollars per day".into(),
            }],
            gaps: vec![],
        };

        write_worker_findings(&run_dir, &findings).unwrap();

        let path = run_dir.join("worker_cruise-pricing.json");
        assert!(path.exists());

        let loaded: WorkerFindings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.subtask_id, "cruise-pricing");
        assert_eq!(loaded.findings.len(), 1);
        assert_eq!(loaded.findings[0].claim, "Cruises cost $50/day");
    }

    #[test]
    fn test_write_worker_findings_rejects_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_run_dir(tmp.path(), "test-run-4", "query").unwrap();

        let findings = WorkerFindings {
            subtask_id: "".into(),
            findings: vec![],
            gaps: vec![],
        };

        assert!(write_worker_findings(&run_dir, &findings).is_err());
    }
}
