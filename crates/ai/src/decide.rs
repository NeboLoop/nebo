//! Typed decisions through Janus `/v1/systemone` (TypeSafe System One, the
//! Jev model).
//!
//! A decision is not a chat turn. The caller hands over a state (text or
//! JSON) and a map of typed questions; every question is answered in one
//! round trip with a distribution and a confidence, in milliseconds, and
//! nothing is generated. This is the door for the judgments the runner used
//! to ask the chat model in prose (did the turn finish, is this a new task,
//! does this fact duplicate that one). Counting, dates, matching and the
//! thresholds stay in code.
//!
//! Jev reads the state literally and does not treat it as hostile: trim the
//! state to the fields a question needs, name them in the instructions with
//! backticks, and keep untrusted text out of the instructions themselves.
//! See the design doc in the neboloop repo, `docs/prd/decide.md`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::types::{ProviderError, RequestTrace};

/// Model sent on every request: the alias, on purpose (owner, 2026-09-22).
/// TypeSafe ships improvements to the alias and we want every one of them
/// without a code change. The trade is that thresholds in the call sites
/// were written against one version's answers and may drift when the alias
/// moves; every decision therefore logs the versioned `model` that actually
/// answered (`Decision::model`, `site=` lines), so a shift shows up in the
/// logs and the thresholds get re-checked, rather than being frozen behind
/// a pin nobody bumps.
pub const JEV_MODEL: &str = "jev-latest";

/// Ceiling on one decision round trip. A decision answers in about 200 ms;
/// this only bounds an upstream that hangs instead of erroring, so a stalled
/// TypeSafe never costs a workflow step more than this. Every caller
/// inherits it.
const DECIDE_TIMEOUT: Duration = Duration::from_secs(5);
/// One retry after a rate limit or an overload, the way the TypeSafe SDK
/// does; anything else fails to the caller's safe default at once.
const RETRY_AFTER: Duration = Duration::from_millis(300);

/// One typed question. The key it is sent under names the answer; the model
/// never sees the key, so the whole question lives in `instructions`. The
/// wire shape is also the authoring shape: a workflow `decide` activity
/// deserializes its `params.questions` straight into this type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// One option from a named set. The answer carries `choice`,
    /// `probabilities` over the options and a `confidence`. Include an
    /// escape option (`other`, `unclear`) whenever the set is not exhaustive.
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    /// A position on 2 to 10 ordered levels. `score` is fractional: 0.0 is
    /// the first level, and a value between two levels means between them.
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
    /// A statement to judge. `noul` is the probability it holds; there is no
    /// separate confidence because the value already is the certainty.
    Noul { instructions: String },
}

impl Question {
    pub fn choice(instructions: &str, criteria: &[(&str, &str)]) -> Self {
        Self::Choice {
            instructions: instructions.to_string(),
            criteria: criteria
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    pub fn score(instructions: &str, levels: &[&str]) -> Self {
        Self::Score {
            instructions: instructions.to_string(),
            criteria: levels.iter().map(|l| l.to_string()).collect(),
        }
    }

    pub fn noul(instructions: &str) -> Self {
        Self::Noul {
            instructions: instructions.to_string(),
        }
    }
}

/// The answer to one question. Serializes back to the wire shape (absent
/// fields omitted) so a workflow node can record it as its output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noul: Option<f64>,
    /// Confidence of a Choice or Score, 0 to 1. Absent on a Noul.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
}

impl Answer {
    /// Probability that a Noul statement holds; 0.0 for any other kind.
    pub fn yes(&self) -> f64 {
        self.noul.unwrap_or(0.0)
    }

    /// The option a Choice picked; empty for any other kind.
    pub fn picked(&self) -> &str {
        self.choice.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Retail cost Janus billed for this call, microdollars.
    #[serde(default)]
    pub cost_micro: i64,
}

/// One answered request.
#[derive(Debug, Clone, Deserialize)]
pub struct Decision {
    /// Versioned model id that answered (e.g. `jev-1.13.0`).
    pub model: String,
    pub answers: HashMap<String, Answer>,
    #[serde(default)]
    pub usage: Usage,
}

impl Decision {
    pub fn answer(&self, name: &str) -> Option<&Answer> {
        self.answers.get(name)
    }
}

/// What a call authenticates with: the NeboAI token as bearer and the bot
/// id for per-bot billing. Resolved on every call so a rotated token is
/// picked up without a restart, the same way the chat path does it.
pub struct Bearer {
    pub token: String,
    pub bot_id: Option<String>,
}

/// Cap `text` at `max_bytes` for a decision state, keeping both ends: the
/// opening says what a message is about, the close says what it asks for or
/// promises next ("next I will…"). A cut keeps the first and last halves of
/// the budget on char boundaries, with a marker naming how much was left out,
/// so Jev reads a gap and not a sentence. Text within the cap is unchanged.
pub fn clip(text: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if text.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut head_end = max_bytes / 2;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - (max_bytes - max_bytes / 2);
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let omitted = tail_start - head_end;
    std::borrow::Cow::Owned(format!(
        "{}\n[... {omitted} bytes left out ...]\n{}",
        &text[..head_end],
        &text[tail_start..]
    ))
}

/// Client for Janus `/v1/systemone`.
pub struct DecideClient {
    url: String,
    auth: Box<dyn Fn() -> Option<Bearer> + Send + Sync>,
    http: reqwest::Client,
}

impl DecideClient {
    /// `janus_url` is the gateway root (no `/v1`). `auth` resolves the current
    /// bearer; `None` means no NeboAI token yet and the call fails as Auth.
    pub fn new(janus_url: &str, auth: impl Fn() -> Option<Bearer> + Send + Sync + 'static) -> Self {
        Self {
            url: format!("{}/v1/systemone", janus_url.trim_end_matches('/')),
            auth: Box::new(auth),
            http: crate::http::request_client(),
        }
    }

    /// Ask every question in `questions` about `state` in one round trip.
    /// `trace` names what the decision is for, sent as `X-Purpose` with the
    /// ids in scope, the same headers the chat path sends.
    pub async fn decide(
        &self,
        trace: &RequestTrace,
        state: &serde_json::Value,
        questions: &BTreeMap<&str, Question>,
    ) -> Result<Decision, ProviderError> {
        let body = serde_json::json!({
            "model": JEV_MODEL,
            "state": state,
            "questions": questions,
        });
        let Some(bearer) = (self.auth)() else {
            return Err(ProviderError::Auth("no NeboAI token".into()));
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut req = self
                .http
                .post(&self.url)
                .timeout(DECIDE_TIMEOUT)
                .bearer_auth(&bearer.token)
                .headers(trace.headers())
                .json(&body);
            if let Some(bot_id) = &bearer.bot_id {
                req = req.header("X-Bot-ID", bot_id);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| ProviderError::Request(e.to_string()))?;
            let status = resp.status();
            if status.is_success() {
                return resp
                    .json::<Decision>()
                    .await
                    .map_err(|e| ProviderError::Request(format!("systemone decode: {e}")));
            }
            let err = classify(status.as_u16(), resp.text().await.unwrap_or_default());
            if attempt == 1 && err.is_retryable() {
                tokio::time::sleep(RETRY_AFTER).await;
                continue;
            }
            return Err(err);
        }
    }
}

/// Map a non-success status to the provider error the caller falls back on.
/// 429 and 5xx (Janus relays an upstream 529 as 502) are the retryable ones.
fn classify(status: u16, text: String) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Auth(text),
        429 => ProviderError::RateLimit { retry_after_secs: None },
        code => ProviderError::Api {
            code: code.to_string(),
            message: text,
            retryable: code >= 500,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_serialize_to_the_wire_shape() {
        let mut q = BTreeMap::new();
        q.insert(
            "dept",
            Question::choice(
                "Which team handles this",
                &[("billing", "money"), ("other", "anything else")],
            ),
        );
        q.insert(
            "mood",
            Question::score("How upset", &["calm", "annoyed", "furious"]),
        );
        q.insert("urgent", Question::noul("It needs attention now"));
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["dept"]["type"], "choice");
        assert_eq!(v["dept"]["criteria"]["billing"], "money");
        assert_eq!(v["mood"]["type"], "score");
        assert_eq!(v["mood"]["criteria"][2], "furious");
        assert_eq!(v["urgent"]["type"], "noul");
        assert_eq!(v["urgent"]["instructions"], "It needs attention now");
        assert!(v["urgent"].get("criteria").is_none());
    }

    #[test]
    fn a_live_answer_decodes() {
        // Verbatim shape of a real jev-1.13.0 answer through Janus.
        let raw = r#"{"model":"jev-1.13.0","answers":{
            "unfinished_commitment":{"type":"noul","noul":0.88},
            "action":{"type":"choice","choice":"update","confidence":1.0,"probabilities":{"update":1.0,"clear":0.0,"set":0.0,"keep":0.0}},
            "urgency":{"type":"score","score":0.02,"confidence":0.97,"legend":{"0":"Routine","1":"Soon","2":"Now"},"probabilities":{"0":0.98,"1":0.02,"2":0.0}}},
            "usage":{"input_tokens":488,"output_tokens":79,"cost_micro":488}}"#;
        let d: Decision = serde_json::from_str(raw).unwrap();
        assert_eq!(d.model, "jev-1.13.0");
        assert_eq!(d.answer("unfinished_commitment").unwrap().yes(), 0.88);
        assert_eq!(d.answer("action").unwrap().picked(), "update");
        assert_eq!(d.answer("action").unwrap().confidence, Some(1.0));
        assert_eq!(d.answer("urgency").unwrap().score, Some(0.02));
        assert_eq!(d.usage.input_tokens, 488);
        assert_eq!(d.usage.cost_micro, 488);
        assert_eq!(d.answer("action").unwrap().yes(), 0.0);
        assert_eq!(d.answer("urgency").unwrap().picked(), "");
    }

    #[test]
    fn only_rate_limits_and_overloads_are_retried() {
        assert!(classify(429, String::new()).is_retryable());
        assert!(classify(502, "upstream 529".into()).is_retryable());
        assert!(!classify(422, "bad question".into()).is_retryable());
        assert!(!classify(401, String::new()).is_retryable());
        assert!(matches!(classify(401, String::new()), ProviderError::Auth(_)));
        assert!(matches!(classify(429, String::new()), ProviderError::RateLimit { .. }));
    }

    #[test]
    fn the_model_is_the_alias_and_the_answer_names_the_version() {
        // Owner 2026-09-22: follow TypeSafe's alias, never pin. The versioned
        // id lives on the answer (`Decision::model`) and is logged per site.
        assert_eq!(JEV_MODEL, "jev-latest");
    }

    #[test]
    fn clip_keeps_both_ends() {
        assert_eq!(clip("short", 10), "short");
        let long = format!("OPENING {} next I will send it", "x".repeat(10_000));
        let clipped = clip(&long, 100);
        assert!(clipped.starts_with("OPENING "), "{clipped}");
        assert!(clipped.ends_with("next I will send it"), "{clipped}");
        assert!(clipped.contains("bytes left out"));
        // Budget plus the marker, never the whole text.
        assert!(clipped.len() < 100 + 40, "{}", clipped.len());
        // Multi-byte text never splits a character.
        let wide = "é".repeat(1_000);
        let clipped = clip(&wide, 101);
        assert!(clipped.starts_with('é') && clipped.ends_with('é'));
    }

    #[test]
    fn client_posts_to_the_systemone_path() {
        let c = DecideClient::new("https://janus.example.com/", || None);
        assert_eq!(c.url, "https://janus.example.com/v1/systemone");
        assert!((c.auth)().is_none());
    }
}
