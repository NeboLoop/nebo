//! `decide`: a typed decision through Jev (Janus `/v1/systemone`), for an
//! employee and for an app's page (`nebo.decide`, `POST
//! /apps/{id}/janus/decide`). Both doors take the same request and go
//! through [`decide`] to the one decision client the runner uses, billed to
//! the owner's NeboAI account like every other Janus call.
//!
//! A request is a state (text or JSON) and named questions in the Jev wire
//! shape: `choice` among named options, `score` on 2 to 10 ordered levels,
//! `noul` (a statement judged true or false). Every answer comes back with
//! its probabilities and a confidence in one round trip; nothing is
//! generated. Counting, dates and thresholds stay with the caller.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use ai::{DecideClient, Question, RequestTrace};
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const DECIDE: &str = "decide";

/// The decision client, installed once at server boot. Desktop control and
/// the `decide` tool both ask through it.
static DECIDER: OnceLock<Arc<DecideClient>> = OnceLock::new();

/// Installed once at server boot.
pub fn set_decider(client: Arc<DecideClient>) {
    let _ = DECIDER.set(client);
}

/// The decision client, when the server installed one.
pub fn decider() -> Option<&'static DecideClient> {
    DECIDER.get().map(|c| c.as_ref())
}

/// Why a decision could not be had.
#[derive(Debug, PartialEq)]
pub enum DecideError {
    /// The request is malformed; the text says which part.
    Invalid(String),
    /// No decision client, or no NeboAI sign-in to call it with.
    Unavailable,
    /// Nothing on the owner's account can pay for it (Janus's funding 429).
    Unfunded,
    /// Too many decisions at once (a throttling 429 that outlasted the retry).
    Busy,
    /// The decision service failed.
    Failed(String),
}

impl std::fmt::Display for DecideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => f.write_str(e),
            Self::Unavailable => f.write_str("Decisions need NeboAI connected. Sign in to NeboAI and try again."),
            Self::Unfunded => f.write_str(
                "You've used all the work included in your account. Choose a plan or add credits to continue.",
            ),
            Self::Busy => f.write_str("Too many decisions at once. Try again in a moment."),
            Self::Failed(e) => write!(f, "The decision could not be made: {e}"),
        }
    }
}

impl From<ai::ProviderError> for DecideError {
    /// What the decision service's answer means to the caller: Janus's 400
    /// (and 422) is the request's fault and keeps Janus's words, its funding
    /// 429 is the account's, no sign-in is NeboAI not connected, and only
    /// the rest is the service failing.
    fn from(e: ai::ProviderError) -> Self {
        match e {
            ai::ProviderError::Auth(_) => Self::Unavailable,
            ai::ProviderError::RateLimit { .. } => Self::Busy,
            ai::ProviderError::Api { code, .. } if code == ai::decide::USAGE_LIMIT_EXCEEDED => Self::Unfunded,
            ai::ProviderError::Api { code, message, .. } if code == "400" || code == "422" => Self::Invalid(message),
            other => Self::Failed(other.to_string()),
        }
    }
}

/// A request's state (clipped at [`ai::decide::STATE_CAP`] bytes, keeping
/// both ends) and its checked questions.
pub fn request(input: &Value) -> Result<(Value, BTreeMap<String, Question>), String> {
    let state = match input.get("state") {
        None | Some(Value::Null) => {
            return Err("`state` is required: the text or JSON the questions are about.".to_string());
        }
        Some(Value::String(s)) if s.trim().is_empty() => {
            return Err("`state` is empty: give the text or JSON the questions are about.".to_string());
        }
        Some(v) => v.clone(),
    };
    let text = match &state {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let state = if text.len() > ai::decide::STATE_CAP {
        Value::String(ai::decide::clip(&text, ai::decide::STATE_CAP).into_owned())
    } else {
        state
    };
    let raw = input
        .get("questions")
        .and_then(Value::as_object)
        .ok_or("`questions` must be an object of named questions.")?;
    Ok((state, ai::decide::questions(raw)?))
}

/// Ask `input`'s questions about its state: `{model, answers, usage}`, each
/// answer in the Jev shape (`choice`/`score`/`noul`, `confidence`,
/// `probabilities`).
pub async fn decide(client: Option<&DecideClient>, trace: &RequestTrace, input: &Value) -> Result<Value, DecideError> {
    let (state, questions) = request(input).map_err(DecideError::Invalid)?;
    let client = client.ok_or(DecideError::Unavailable)?;
    let asked: BTreeMap<&str, Question> = questions.iter().map(|(k, q)| (k.as_str(), q.clone())).collect();
    let d = client
        .decide(trace, &state, &asked)
        .await
        .map_err(DecideError::from)?;
    Ok(json!({
        "model": d.model,
        "answers": d.answers,
        "usage": {
            "input_tokens": d.usage.input_tokens,
            "output_tokens": d.usage.output_tokens,
            "cost_micro": d.usage.cost_micro,
        },
    }))
}

/// The employee's door.
pub struct DecideTool;

type Fut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

impl DynTool for DecideTool {
    fn name(&self) -> &str {
        DECIDE
    }

    fn description(&self) -> String {
        "Makes typed decisions about a state (text or JSON, e.g. records from app_data) in one fast call, \
         each answer with probabilities and a confidence. Name each question; its type is choice (pick one \
         of `criteria`: {option: description}, include an escape option like `other` when the set is not \
         exhaustive), score (a position on `criteria`: 2-10 ordered levels; the answer is fractional, 0.0 = \
         the first level) or noul (judge a statement; `noul` is the probability it holds). The whole \
         question lives in `instructions`; name the state's fields in backticks and keep the state to what \
         the questions need. Use it for judgments (triage, classify, rank, is this a duplicate); keep \
         counting, dates and thresholds in your own steps."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "state": {
                    "description": "What the questions are about: text, or any JSON (a record, a list of records)."
                },
                "questions": {
                    "type": "object",
                    "description": "Named questions; each answer comes back under its name.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["choice", "score", "noul"] },
                            "instructions": { "type": "string", "description": "The whole question." },
                            "criteria": {
                                "description": "choice: {option: description}, 2-255 options. score: [levels], 2-10, lowest first. noul: none."
                            }
                        },
                        "required": ["type", "instructions"]
                    }
                }
            },
            "required": ["state", "questions"]
        })
    }

    fn search_hint(&self) -> &str {
        "classify judge choose score yes no"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        request(input).map(|_| ())
    }

    fn activity(&self, _input: &Value) -> String {
        "deciding".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Decided".to_string()
    }

    fn execute_dyn<'a>(&'a self, ctx: &'a ToolContext, input: Value) -> Fut<'a> {
        Box::pin(async move { run(decider(), ctx, &input).await })
    }
}

/// One tool call against `client`.
async fn run(client: Option<&DecideClient>, ctx: &ToolContext, input: &Value) -> ToolResult {
    let trace = RequestTrace {
        agent_id: types::keyparser::extract_agent_id(&ctx.session_key),
        ..RequestTrace::new("employee_decide")
    };
    match decide(client, &trace, input).await {
        Ok(out) => ToolResult::ok(out.to_string()),
        Err(e) => ToolResult::error(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decision service on a local port: records each request body and
    /// answers with `answer`.
    async fn serving(answer: Value) -> (DecideClient, Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(at) = text.find("\r\n\r\n") {
                        let len = text[..at]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= at + 4 + len {
                            let body: Value = serde_json::from_slice(&buf[at + 4..at + 4 + len]).unwrap_or(Value::Null);
                            log.lock().unwrap().push(body);
                            break;
                        }
                    }
                }
                let body = answer.to_string();
                let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let client = DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer { token: "owner-token".into(), bot_id: Some("bot-1".into()) })
        });
        (client, seen)
    }

    fn ctx() -> ToolContext {
        ToolContext { session_key: "agent:crm-app:web".into(), ..Default::default() }
    }

    #[test]
    fn the_schema_is_one_flat_tool_over_the_three_question_types() {
        let schema = DecideTool.schema();
        assert_eq!(schema["required"], json!(["state", "questions"]));
        assert!(schema["properties"].get("action").is_none());
        assert_eq!(
            schema["properties"]["questions"]["additionalProperties"]["properties"]["type"]["enum"],
            json!(["choice", "score", "noul"])
        );
        assert!(DecideTool.should_defer(), "deferred: found by find_tools when a judgment is needed");
        assert!(DecideTool.read_only(&json!({})));
    }

    #[test]
    fn a_malformed_request_is_named_before_any_call() {
        let t = DecideTool;
        let q = json!({ "lead": { "type": "noul", "instructions": "Is `status` hot?" } });
        assert!(t.validate_input(&json!({ "state": "x", "questions": q })).is_ok());
        let missing = t.validate_input(&json!({ "questions": q })).unwrap_err();
        assert!(missing.contains("`state` is required"), "{missing}");
        let empty = t.validate_input(&json!({ "state": "x", "questions": {} })).unwrap_err();
        assert!(empty.contains("at least one"), "{empty}");
        let one = t
            .validate_input(&json!({ "state": "x", "questions": { "pick": { "type": "choice", "instructions": "Which?", "criteria": { "a": "A" } } } }))
            .unwrap_err();
        assert!(one.contains("choice question 'pick' needs 2 to 255 criteria, has 1"), "{one}");
        let kind = t
            .validate_input(&json!({ "state": "x", "questions": { "q": { "type": "maybe", "instructions": "?" } } }))
            .unwrap_err();
        assert!(kind.contains("expected choice, score or noul"), "{kind}");
    }

    #[tokio::test]
    async fn the_decision_goes_to_the_owners_janus_and_comes_back_typed() {
        let (client, seen) = serving(json!({
            "model": "jev-1.13.0",
            "answers": {
                "tier": { "type": "choice", "choice": "hot", "confidence": 0.91, "probabilities": { "hot": 0.91, "cold": 0.09 } },
                "fit": { "type": "score", "score": 1.6, "confidence": 0.8, "probabilities": { "low": 0.1, "mid": 0.2, "high": 0.7 } },
                "reply": { "type": "noul", "noul": 0.97 }
            },
            "usage": { "input_tokens": 120, "output_tokens": 3, "cost_micro": 12 }
        }))
        .await;
        let input = json!({
            "state": { "name": "Acme", "status": "asked for a quote today" },
            "questions": {
                "tier": { "type": "choice", "instructions": "How warm is this lead, by `status`?", "criteria": { "hot": "ready to buy", "cold": "not now" } },
                "fit": { "type": "score", "instructions": "How well does `name` fit?", "criteria": ["low", "mid", "high"] },
                "reply": { "type": "noul", "instructions": "`status` asks for a reply." }
            }
        });
        let out = run(Some(&client), &ctx(), &input).await;
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["model"], "jev-1.13.0");
        assert_eq!(v["answers"]["tier"]["choice"], "hot");
        assert_eq!(v["answers"]["fit"]["score"], 1.6);
        assert_eq!(v["answers"]["reply"]["noul"], 0.97);
        assert_eq!(v["usage"]["cost_micro"], 12);

        let sent = seen.lock().unwrap()[0].clone();
        assert_eq!(sent["model"], ai::JEV_MODEL);
        assert_eq!(sent["state"]["name"], "Acme");
        assert_eq!(sent["questions"]["fit"]["criteria"], json!(["low", "mid", "high"]));
    }

    #[tokio::test]
    async fn without_neboai_it_says_so_and_a_long_state_is_clipped() {
        let input = json!({ "state": "x", "questions": { "q": { "type": "noul", "instructions": "Is it?" } } });
        let out = run(None, &ctx(), &input).await;
        assert!(out.is_error && out.content.contains("NeboAI"), "{}", out.content);

        let (state, _) = request(&json!({ "state": "y".repeat(ai::decide::STATE_CAP * 2), "questions": input["questions"] })).unwrap();
        let text = state.as_str().unwrap();
        assert!(text.len() < ai::decide::STATE_CAP + 64 && text.contains("bytes left out"), "{}", text.len());
    }
}
