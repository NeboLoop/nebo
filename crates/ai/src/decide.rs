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
//! See docs/prd/2026-09-21-jev-typed-decisions-fit.md.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::types::ProviderError;

/// Model alias sent on every request. Pin a versioned id (`jev-1.13.0`) once
/// thresholds are tuned against it; the alias moves when TypeSafe ships.
pub const JEV_MODEL: &str = "jev-latest";

/// One typed question. The key it is sent under names the answer; the model
/// never sees the key, so the whole question lives in `instructions`.
#[derive(Debug, Clone, Serialize)]
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

/// The answer to one question.
#[derive(Debug, Clone, Deserialize)]
pub struct Answer {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub noul: Option<f64>,
    /// Confidence of a Choice or Score, 0 to 1. Absent on a Noul.
    #[serde(default)]
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

/// Client for Janus `/v1/systemone`. Built beside the Janus chat provider
/// with the same gateway root, bearer and bot id.
pub struct DecideClient {
    url: String,
    api_key: String,
    bot_id: Option<String>,
    http: reqwest::Client,
}

impl DecideClient {
    /// `janus_url` is the gateway root (no `/v1`).
    pub fn new(janus_url: &str, api_key: impl Into<String>, bot_id: Option<String>) -> Self {
        Self {
            url: format!("{}/v1/systemone", janus_url.trim_end_matches('/')),
            api_key: api_key.into(),
            bot_id,
            http: crate::http::request_client(),
        }
    }

    /// Ask every question in `questions` about `state` in one round trip.
    pub async fn decide(
        &self,
        state: &serde_json::Value,
        questions: &BTreeMap<&str, Question>,
    ) -> Result<Decision, ProviderError> {
        let body = serde_json::json!({
            "model": JEV_MODEL,
            "state": state,
            "questions": questions,
        });
        let mut req = self
            .http
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body);
        if let Some(bot_id) = &self.bot_id {
            req = req.header("X-Bot-ID", bot_id);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Request(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Auth(text),
                429 => ProviderError::RateLimit,
                code => ProviderError::Api {
                    code: code.to_string(),
                    message: text,
                    retryable: code >= 500,
                },
            });
        }
        resp.json::<Decision>()
            .await
            .map_err(|e| ProviderError::Request(format!("systemone decode: {e}")))
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
    fn client_posts_to_the_systemone_path() {
        let c = DecideClient::new("https://janus.example.com/", "k", None);
        assert_eq!(c.url, "https://janus.example.com/v1/systemone");
    }
}
