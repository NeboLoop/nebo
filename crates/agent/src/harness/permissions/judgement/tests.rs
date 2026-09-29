//! The judgement's path: Jev first, one batched call; the aux classifier
//! for exactly the questions Jev didn't settle; unjudged when both fail.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai::{ChatRequest, EventReceiver, Provider, ProviderError, StreamEvent};
use serde_json::json;
use tokio::sync::mpsc;
use types::permissions::AskCase;

use super::*;

fn question(tool: &str, untrusted: Option<&str>) -> Question {
    Question {
        tool: tool.into(),
        activity: format!("using {tool}"),
        input: json!({ "url": "https://example.com/form" }),
        untrusted: untrusted.map(str::to_string),
    }
}

/// How the fake Jev answers.
#[derive(Clone)]
enum Jev {
    /// Every decision comes back with these Nouls (`c0`, `c1`, …).
    Answers(Vec<f64>),
    /// Accepts and never answers.
    Hangs,
}

/// A Jev served by a local listener; counts the decisions it was asked for
/// and keeps each request's question keys.
async fn jev(behaviour: Jev) -> (DecideClient, Arc<AtomicUsize>, Arc<Mutex<Vec<Vec<String>>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (c, a) = (calls.clone(), asked.clone());
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (behaviour, calls, asked) = (behaviour.clone(), c.clone(), a.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let body_start = loop {
                    let Ok(n) = sock.read(&mut chunk).await else { return };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break end + 4;
                        }
                    }
                };
                calls.fetch_add(1, Ordering::SeqCst);
                let req: serde_json::Value = serde_json::from_slice(&buf[body_start..]).unwrap_or_default();
                let keys: Vec<String> =
                    req["questions"].as_object().map(|q| q.keys().cloned().collect()).unwrap_or_default();
                asked.lock().unwrap().push(keys);
                let Jev::Answers(nouls) = behaviour else {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    return;
                };
                let answers: serde_json::Map<String, serde_json::Value> = nouls
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (format!("c{i}"), json!({ "type": "noul", "noul": p })))
                    .collect();
                let body = json!({ "model": "jev-test", "answers": answers, "usage": {} }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    let client = DecideClient::new(&format!("http://{addr}"), || Some(ai::Bearer { token: "t".into(), bot_id: None }));
    (client, calls, asked)
}

/// A fake aux model: answers with `reply`, or fails when `None`; keeps the
/// requests it saw.
struct Aux {
    reply: Option<String>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Aux {
    fn new(reply: Option<&str>) -> Arc<Self> {
        Arc::new(Self { reply: reply.map(str::to_string), seen: Mutex::new(Vec::new()) })
    }
    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    /// The call ids the classifier was asked about.
    fn asked_ids(&self) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        let body: serde_json::Value = serde_json::from_str(&seen.last().unwrap().messages[0].content).unwrap();
        body["calls"].as_array().unwrap().iter().map(|c| c["tool"].as_str().unwrap().to_string()).collect()
    }
}

#[async_trait::async_trait]
impl Provider for Aux {
    fn id(&self) -> &str {
        "aux"
    }
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        let Some(text) = self.reply.clone() else {
            return Err(ProviderError::Request("down".into()));
        };
        let (tx, rx) = mpsc::channel(4);
        let _ = tx.send(StreamEvent::text(text)).await;
        let _ = tx.send(StreamEvent::done()).await;
        Ok(rx)
    }
}

fn trace(purpose: &'static str) -> RequestTrace {
    RequestTrace::new(purpose)
}

async fn judge(decide: Option<&DecideClient>, aux: &Arc<Aux>, questions: &[Question]) -> Vec<Verdict> {
    let providers: Vec<Arc<dyn Provider>> = vec![aux.clone()];
    let cx = JudgeCx { decide, providers: &providers, trace: &trace, objective: "file the report", last_message: "send it" };
    judge_round(&cx, questions).await
}

#[tokio::test]
async fn undecidable_goes_to_one_batched_judgement() {
    let (client, calls, asked) = jev(Jev::Answers(vec![0.02, 0.97, 0.05])).await;
    let aux = Aux::new(None);
    let qs = [question("browser", None), question("http_request", None), question("run_command", None)];
    let v = judge(Some(&client), &aux, &qs).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one decision for the whole round");
    assert_eq!(asked.lock().unwrap()[0], vec!["c0", "c1", "c2"]);
    assert!(matches!(v[0], Verdict::Allow { by: JudgedBy::Jev, .. }));
    assert!(matches!(&v[1], Verdict::Ask { case: AskCase::NewCounterparty { .. }, by: JudgedBy::Jev, .. }));
    assert!(matches!(v[2], Verdict::Allow { by: JudgedBy::Jev, .. }));
}

#[tokio::test]
async fn jev_answers_and_aux_is_not_called() {
    let (client, _, _) = jev(Jev::Answers(vec![0.9])).await;
    let aux = Aux::new(Some(r#"{"verdicts":[{"id":"c0","ask":false,"reason":"x"}]}"#));
    let v = judge(Some(&client), &aux, &[question("browser", Some("web"))]).await;
    assert_eq!(aux.calls(), 0);
    // A tainted run's "yes" is case 5.
    assert!(matches!(&v[0], Verdict::Ask { case: AskCase::UntrustedInput { source }, .. } if source == "web"));
}

#[tokio::test]
async fn jev_down_falls_back_to_aux_with_the_same_questions() {
    let aux = Aux::new(Some(
        r#"Here: {"verdicts":[{"id":"c0","ask":true,"reason":"submits a public form"},{"id":"c1","ask":false,"reason":"saves locally"}]}"#,
    ));
    let qs = [question("browser", None), question("http_request", None)];
    // No decide client at all (no NeboAI connection)...
    let v = judge(None, &aux, &qs).await;
    assert_eq!(aux.asked_ids(), vec!["browser", "http_request"]);
    assert_eq!(
        v,
        vec![
            Verdict::Ask {
                case: AskCase::NewCounterparty { who: crate::harness::permissions::cases::PUBLIC.into() },
                by: JudgedBy::AuxClassifier,
                reason: "submits a public form".into()
            },
            Verdict::Allow { by: JudgedBy::AuxClassifier, reason: "saves locally".into() },
        ]
    );
    // ...and a client whose Janus refuses (no bearer).
    let refusing = DecideClient::new("http://127.0.0.1:9", || None);
    let v2 = judge(Some(&refusing), &aux, &qs).await;
    assert_eq!(v2, v);
    assert_eq!(aux.calls(), 2);
}

#[tokio::test]
async fn jev_timeout_falls_back_to_aux() {
    let (client, _, _) = jev(Jev::Hangs).await;
    let aux = Aux::new(Some(r#"{"verdicts":[{"id":"c0","ask":false,"reason":"reads a page"}]}"#));
    let started = std::time::Instant::now();
    let v = judge(Some(&client), &aux, &[question("browser", None)]).await;
    assert!(started.elapsed() < Duration::from_secs(5), "the Jev deadline bounds the wait");
    assert_eq!(v, vec![Verdict::Allow { by: JudgedBy::AuxClassifier, reason: "reads a page".into() }]);
}

#[tokio::test]
async fn jev_low_confidence_sends_only_those_questions_to_aux() {
    // 0.55 and 0.3 are below the bar (confidence 0.1 and 0.4); 0.01 settles.
    let (client, _, _) = jev(Jev::Answers(vec![0.55, 0.01, 0.3])).await;
    let aux = Aux::new(Some(
        r#"{"verdicts":[{"id":"c0","ask":true,"reason":"posts a comment"},{"id":"c1","ask":false,"reason":"a search"}]}"#,
    ));
    let qs = [question("browser", None), question("fetch", None), question("http_request", None)];
    let v = judge(Some(&client), &aux, &qs).await;
    assert_eq!(aux.asked_ids(), vec!["browser", "http_request"], "only the unsettled questions");
    assert!(matches!(&v[0], Verdict::Ask { by: JudgedBy::AuxClassifier, reason, .. } if reason == "posts a comment"));
    assert!(matches!(v[1], Verdict::Allow { by: JudgedBy::Jev, .. }));
    assert!(matches!(&v[2], Verdict::Allow { by: JudgedBy::AuxClassifier, reason } if reason == "a search"));
}

#[tokio::test]
async fn aux_verdict_is_json_with_a_reason() {
    let ids = vec!["c0".to_string()];
    assert_eq!(
        parse_aux(r#"{"verdicts":[{"id":"c0","ask":true,"reason":" posts publicly "}]}"#, &ids),
        Some(vec![(true, "posts publicly".to_string())])
    );
    // No reason, a missing verdict, or prose: not an answer.
    assert_eq!(parse_aux(r#"{"verdicts":[{"id":"c0","ask":true}]}"#, &ids), None);
    assert_eq!(parse_aux(r#"{"verdicts":[]}"#, &ids), None);
    assert_eq!(parse_aux("it looks fine to me", &ids), None);
    // The classifier's request asks for exactly that shape and carries no tools.
    let aux = Aux::new(Some("not json"));
    let v = judge(None, &aux, &[question("browser", None)]).await;
    assert_eq!(v, vec![Verdict::Unjudged], "an unparseable answer is no answer");
    let req = aux.seen.lock().unwrap()[0].clone();
    assert!(req.tools.is_empty());
    assert!(req.system.contains(r#"{"verdicts": [{"id""#), "{}", req.system);
}

#[tokio::test]
async fn both_down_leaves_every_question_unjudged() {
    let aux = Aux::new(None);
    let v = judge(None, &aux, &[question("browser", None), question("http_request", None)]).await;
    assert_eq!(v, vec![Verdict::Unjudged, Verdict::Unjudged]);
    assert!(judge(None, &aux, &[]).await.is_empty(), "no questions, no calls");
    assert_eq!(aux.calls(), 1);
}

#[test]
fn secrets_are_redacted_by_key_name() {
    let input = json!({
        "headers": {"Authorization": "Bearer abc", "X-Trace": "t1"},
        "body": {"password": "hunter2", "items": [{"token": "t"}, {"name": "ok"}]},
    });
    let r = redact(&input).to_string();
    assert!(!r.contains("abc") && !r.contains("hunter2") && !r.contains(r#""token":"t""#));
    assert!(r.contains("t1") && r.contains("ok"));
}
