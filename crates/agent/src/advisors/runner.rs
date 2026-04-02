use std::sync::Arc;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tracing::{debug, warn};

use ai::{ChatRequest, Message, Provider, StreamEventType};

use super::advisor::{Advisor, Response};
use super::loader::Loader;

/// Maximum number of advisors to consult in parallel.
const MAX_ADVISORS: usize = 5;

/// Default per-advisor timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Runs advisor deliberation: consults enabled advisors in parallel via LLM.
pub struct Runner {
    loader: Arc<Loader>,
    providers: Arc<Vec<Arc<dyn Provider>>>,
}

impl Runner {
    pub fn new(loader: Arc<Loader>, providers: Arc<Vec<Arc<dyn Provider>>>) -> Self {
        Self { loader, providers }
    }

    /// Run deliberation: query up to MAX_ADVISORS enabled advisors in parallel,
    /// each with their own system prompt, and aggregate responses.
    pub async fn deliberate(&self, task: &str) -> Result<Vec<Response>, String> {
        let provider = self
            .providers
            .first()
            .ok_or_else(|| "no AI providers configured".to_string())?;

        let advisors = self.loader.list_enabled().await;
        if advisors.is_empty() {
            return Ok(Vec::new());
        }

        let capped: Vec<Advisor> = advisors.into_iter().take(MAX_ADVISORS).collect();
        debug!(count = capped.len(), task = %task, "starting advisor deliberation");

        let mut futures = FuturesUnordered::new();

        for advisor in &capped {
            let advisor = advisor.clone();
            let task = task.to_string();
            let timeout_secs = if advisor.timeout_seconds > 0 {
                advisor.timeout_seconds as u64
            } else {
                DEFAULT_TIMEOUT_SECS
            };

            let provider_ref: &dyn Provider = &**provider;

            futures.push(async move {
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    run_single_advisor(provider_ref, &advisor, &task),
                )
                .await;

                match result {
                    Ok(Ok(response)) => Some(response),
                    Ok(Err(e)) => {
                        warn!(advisor = %advisor.name, error = %e, "advisor deliberation failed");
                        None
                    }
                    Err(_) => {
                        warn!(advisor = %advisor.name, timeout_secs, "advisor timed out");
                        None
                    }
                }
            });
        }

        let mut responses = Vec::new();
        while let Some(result) = futures.next().await {
            if let Some(response) = result {
                responses.push(response);
            }
        }

        // Sort by confidence (highest first)
        responses.sort_by(|a, b| b.confidence.cmp(&a.confidence));

        debug!(
            count = responses.len(),
            "advisor deliberation complete"
        );

        Ok(responses)
    }

    /// Format advisor responses for injection into the main agent's context.
    pub fn format_for_injection(responses: &[Response]) -> String {
        if responses.is_empty() {
            return String::new();
        }

        let mut output = String::from(
            "---\n## Internal Deliberation (Advisor Perspectives)\n\n\
             Before responding, consider these internal perspectives:\n\n",
        );

        for response in responses {
            output.push_str(&format!("### {} ({})\n", response.advisor_name, response.role));
            output.push_str(&response.critique);
            if !response.risks.is_empty() {
                output.push_str(&format!("\n**Risks:** {}", response.risks));
            }
            if !response.suggestion.is_empty() {
                output.push_str(&format!("\n**Suggestion:** {}", response.suggestion));
            }
            output.push_str(&format!("\n*Confidence: {}/10*\n\n", response.confidence));
        }

        output.push_str("---\n\nSynthesize these perspectives but make your own decision. You are the authority.\n");

        output
    }
}

/// Run a single advisor's LLM call and parse the response.
async fn run_single_advisor(
    provider: &dyn Provider,
    advisor: &Advisor,
    task: &str,
) -> Result<Response, String> {
    let system_prompt = advisor.build_system_prompt(task);

    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: format!(
                "Analyze this task and provide your perspective:\n\n{}",
                task
            ),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 1024,
        temperature: 0.7,
        system: system_prompt,
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = provider
        .stream(&req)
        .await
        .map_err(|e| format!("provider error: {}", e))?;

    let mut full_text = String::new();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => {
                full_text.push_str(&event.text);
            }
            StreamEventType::Error => {
                return Err(event.error.unwrap_or_else(|| "unknown error".into()));
            }
            StreamEventType::Done => break,
            _ => {}
        }
    }

    if full_text.is_empty() {
        return Err("empty response from advisor".into());
    }

    let confidence = Response::extract_confidence(&full_text);
    let risks = Response::extract_section(&full_text, "Risks");
    let suggestion = Response::extract_section(&full_text, "Suggestion");
    let assessment = Response::extract_section(&full_text, "Assessment");

    let critique = if assessment.is_empty() {
        // If no structured assessment, use the full text
        full_text.clone()
    } else {
        assessment
    };

    Ok(Response {
        advisor_name: advisor.name.clone(),
        role: advisor.role.clone(),
        critique,
        confidence,
        risks,
        suggestion,
    })
}

/// Implement the AdvisorDeliberator trait from tools crate
/// so that AgentTool can call deliberate without circular dependencies.
impl tools::bot_tool::AdvisorDeliberator for Runner {
    fn deliberate<'a>(
        &'a self,
        task: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let responses = self.deliberate(task).await?;
            Ok(Self::format_for_injection(&responses))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_for_injection_empty() {
        let output = Runner::format_for_injection(&[]);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_for_injection() {
        let responses = vec![
            Response {
                advisor_name: "skeptic".to_string(),
                role: "critic".to_string(),
                critique: "This approach has significant risks.".to_string(),
                confidence: 8,
                risks: "Could fail under load.".to_string(),
                suggestion: "Add caching first.".to_string(),
            },
            Response {
                advisor_name: "pragmatist".to_string(),
                role: "builder".to_string(),
                critique: "The approach is practical and achievable.".to_string(),
                confidence: 7,
                risks: String::new(),
                suggestion: "Ship the MVP.".to_string(),
            },
        ];

        let output = Runner::format_for_injection(&responses);
        assert!(output.contains("### skeptic (critic)"));
        assert!(output.contains("### pragmatist (builder)"));
        assert!(output.contains("significant risks"));
        assert!(output.contains("Confidence: 8/10"));
        assert!(output.contains("Synthesize these perspectives"));
    }
}
