//! The comm outbox against a fake hub: a message that cannot go out now
//! waits in the outbox and goes out once, in order, on the next connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use nebo_comm as comm;
use comm::frame::{self, Header};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

struct NoOffsets;

impl comm::StreamOffsets for NoOffsets {
    fn acked(&self, _bot_id: &str, _stream: &str) -> u64 {
        0
    }
    fn record(&self, _bot_id: &str, _stream: &str, _seq: u64) {}
}

/// The outbox in memory. `lose_removes` drops every delete, as a process
/// that died between a send and its delete would.
#[derive(Default)]
struct MemOutbox {
    msgs: Mutex<Vec<(comm::CommMessage, Option<i64>)>>,
    lose_removes: AtomicBool,
}

impl MemOutbox {
    fn ids(&self) -> Vec<String> {
        self.msgs.lock().unwrap().iter().map(|(m, _)| m.id.clone()).collect()
    }
}

impl comm::Outbox for MemOutbox {
    fn put(&self, msg: &comm::CommMessage) -> bool {
        let mut msgs = self.msgs.lock().unwrap();
        if !msgs.iter().any(|(m, _)| m.id == msg.id) {
            msgs.push((msg.clone(), None));
        }
        true
    }
    fn pending(&self) -> Vec<(comm::CommMessage, Option<i64>)> {
        self.msgs.lock().unwrap().clone()
    }
    fn handed_off(&self, id: &str, at: Option<i64>) {
        for (m, handed) in self.msgs.lock().unwrap().iter_mut() {
            if m.id == id {
                *handed = at;
            }
        }
    }
    fn remove(&self, id: &str) {
        if !self.lose_removes.load(Ordering::SeqCst) {
            self.msgs.lock().unwrap().retain(|(m, _)| m.id != id);
        }
    }
}

fn message(id: &str, conversation_id: &str) -> comm::CommMessage {
    serde_json::from_value(serde_json::json!({
        "id": id, "from": "", "to": "", "topic": "dm",
        "conversation_id": conversation_id, "type": "message", "content": format!("reply {id}"),
    }))
    .unwrap()
}

/// One durable SEND as the hub saw it.
struct Sent {
    conn: usize,
    msg_id: String,
    payload: serde_json::Value,
}

impl Sent {
    fn client_id(&self) -> &str {
        self.payload["content"]["clientId"].as_str().unwrap_or_default()
    }
}

/// A fake hub. Every connection is answered AUTH_OK, naming `features`;
/// with `drop_first` the first is closed right after. Each durable SEND it
/// receives comes out of the returned channel.
async fn fake_hub(drop_first: bool, features: &[&str]) -> (String, mpsc::UnboundedReceiver<Sent>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let auth_ok = serde_json::to_vec(&serde_json::json!({"ok": true, "features": features})).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        for conn in 0.. {
            let (tcp, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            let auth_ok = auth_ok.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let _connect = ws.next().await;
                let ok = frame::encode(
                    Header { frame_type: frame::TYPE_AUTH_OK, ..Default::default() },
                    &auth_ok,
                )
                .unwrap();
                ws.send(WsMessage::Binary(ok.into())).await.unwrap();
                if drop_first && conn == 0 {
                    let _ = ws.close(None).await;
                    return;
                }
                while let Some(Ok(WsMessage::Binary(data))) = ws.next().await {
                    let (h, payload) = frame::decode(&data).unwrap();
                    if h.frame_type == frame::TYPE_SEND_MESSAGE && !h.is_ephemeral() {
                        let _ = tx.send(Sent {
                            conn,
                            msg_id: uuid::Uuid::from_bytes(h.msg_id).to_string(),
                            payload: serde_json::from_slice(payload).unwrap(),
                        });
                    }
                }
            });
        }
    });
    (url, rx)
}

async fn manager_with(outbox: Arc<MemOutbox>) -> comm::PluginManager {
    let plugin = comm::NeboAIPlugin::new(Arc::new(NoOffsets)).with_outbox(outbox);
    let manager = comm::PluginManager::new();
    manager.register(Arc::new(plugin)).await;
    manager.set_active("neboai").await.unwrap();
    manager
}

fn config(url: &str) -> HashMap<String, String> {
    HashMap::from([
        ("gateway".to_string(), url.to_string()),
        ("bot_id".to_string(), "bot-under-test".to_string()),
        ("token".to_string(), "test-token".to_string()),
        ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
    ])
}

async fn next_send(rx: &mut mpsc::UnboundedReceiver<Sent>) -> Sent {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the hub received a send")
        .unwrap()
}

async fn wait_for(cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition never held");
}

async fn wait_disconnected(manager: &comm::PluginManager) {
    for _ in 0..200 {
        if !manager.is_connected().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("still connected");
}

/// The E2E failure: the hub drops the connection, a reply is produced in the
/// gap ("not connected"), and the bot redials. The reply reaches the hub
/// exactly once, on the new connection, and the outbox is empty after.
#[tokio::test]
async fn a_reply_made_while_reconnecting_is_delivered_once() {
    let (url, mut sends) = fake_hub(true, &[]).await;
    let outbox = Arc::new(MemOutbox::default());
    let manager = manager_with(outbox.clone()).await;

    manager.connect_active(config(&url)).await.unwrap();
    wait_disconnected(&manager).await;

    manager.send(message("r1", "conv-a")).await.expect("kept for the next connection");
    let ids = outbox.ids();
    assert_eq!(ids.len(), 1);

    manager.connect_active(config(&url)).await.unwrap();
    let sent = next_send(&mut sends).await;
    assert_eq!(sent.conn, 1, "sent on the new connection");
    assert_eq!(sent.payload["conversationId"], "conv-a");
    assert_eq!(sent.payload["content"]["text"], "reply r1");
    assert_eq!(sent.client_id(), ids[0]);
    assert_eq!(sent.msg_id, ids[0], "the wire id is the message's own");

    wait_for(|| outbox.ids().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sends.try_recv().is_err(), "delivered exactly once");
}

/// A hub without msg-id dedupe would store a second copy, so a message that
/// may have reached it (handed to the connection, never confirmed) is
/// dropped from the outbox, not sent again.
#[tokio::test]
async fn without_hub_dedupe_an_ambiguous_send_is_not_resent() {
    let (url, mut sends) = fake_hub(false, &[]).await;
    let outbox = Arc::new(MemOutbox::default());
    // The process dies between handing the message off and deleting it.
    outbox.lose_removes.store(true, Ordering::SeqCst);
    let manager = manager_with(outbox.clone()).await;

    manager.connect_active(config(&url)).await.unwrap();
    manager.send(message("r1", "conv-a")).await.unwrap();
    assert_eq!(next_send(&mut sends).await.conn, 0);
    assert_eq!(outbox.ids().len(), 1, "the delete was lost");

    outbox.lose_removes.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(5)).await;
    manager.connect_active(config(&url)).await.unwrap();
    wait_for(|| outbox.ids().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sends.try_recv().is_err(), "not sent again");
}

/// A hub that dedupes by msg id gets every unconfirmed send again, with the
/// same id, after the connection it went out on dropped. Once out on the
/// live connection it is not sent again there.
#[tokio::test]
async fn with_hub_dedupe_an_ambiguous_send_goes_again_with_the_same_id() {
    let (url, mut sends) = fake_hub(false, &["msg_dedup"]).await;
    let outbox = Arc::new(MemOutbox::default());
    let manager = manager_with(outbox.clone()).await;

    manager.connect_active(config(&url)).await.unwrap();
    manager.send(message("r1", "conv-a")).await.unwrap();
    let first = next_send(&mut sends).await;
    assert_eq!(first.conn, 0);
    assert_eq!(outbox.ids().len(), 1, "kept until confirmed");

    tokio::time::sleep(Duration::from_millis(5)).await;
    manager.connect_active(config(&url)).await.unwrap();
    let second = next_send(&mut sends).await;
    assert_eq!(second.conn, 1);
    assert_eq!(second.msg_id, first.msg_id);
    assert_eq!(second.client_id(), first.client_id());

    // A later send on the same connection carries only itself.
    manager.send(message("r2", "conv-a")).await.unwrap();
    let third = next_send(&mut sends).await;
    assert_eq!(third.payload["content"]["text"], "reply r2");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sends.try_recv().is_err(), "nothing repeated on a live connection");
}

/// What a previous process left in the outbox (never handed off) goes out
/// on the first connection, oldest first.
#[tokio::test]
async fn what_waits_goes_out_in_order_after_a_restart() {
    let (url, mut sends) = fake_hub(false, &[]).await;
    let outbox = Arc::new(MemOutbox::default());
    for (id, conv) in [("a", "conv-1"), ("b", "conv-1"), ("c", "conv-2"), ("d", "conv-1")] {
        comm::Outbox::put(outbox.as_ref(), &message(id, conv));
    }
    let manager = manager_with(outbox.clone()).await;

    manager.connect_active(config(&url)).await.unwrap();
    let mut got = Vec::new();
    for _ in 0..4 {
        got.push(next_send(&mut sends).await.client_id().to_string());
    }
    assert_eq!(got, ["a", "b", "c", "d"]);
    wait_for(|| outbox.ids().is_empty()).await;
}

/// A message the bot can never send is dropped, not retried, and does not
/// hold up the messages behind it.
#[tokio::test]
async fn a_permanent_refusal_is_dropped_and_not_retried() {
    let (url, mut sends) = fake_hub(false, &[]).await;
    let outbox = Arc::new(MemOutbox::default());
    let manager = manager_with(outbox.clone()).await;

    // Queued while there is no connection: one with nowhere to go, one fine.
    manager.send(message("nowhere", "")).await.unwrap();
    manager.send(message("fine", "conv-a")).await.unwrap();
    assert_eq!(outbox.ids().len(), 2);

    manager.connect_active(config(&url)).await.unwrap();
    assert_eq!(next_send(&mut sends).await.payload["content"]["text"], "reply fine");
    wait_for(|| outbox.ids().is_empty()).await;

    // Refused live: the caller hears it and nothing is kept.
    let err = manager.send(message("nowhere-2", "")).await.unwrap_err();
    assert!(matches!(err, comm::CommError::Other(_)), "{err:?}");
    assert!(outbox.ids().is_empty());

    // A later connection sends nothing more.
    manager.connect_active(config(&url)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sends.try_recv().is_err(), "nothing retried");
}
