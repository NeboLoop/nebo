//! The RESIDENCY frame on the wire, against a fake hub: a process nothing
//! subscribed for declines with a `busy` frame; one that parks answers
//! nothing and from then on neither handles nor acks a delivery, so it waits
//! in the hub mailbox for the next process.
//!
//! Its own test binary: the subscriber and the held deliveries are
//! process-wide.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use nebo_comm as comm;
use nebo_comm::CommPlugin;
use nebo_comm::frame::{self, Header};
use tokio_tungstenite::tungstenite::Message as WsMessage;

struct NoOffsets;

impl comm::StreamOffsets for NoOffsets {
    fn acked(&self, _bot_id: &str, _stream: &str) -> u64 {
        0
    }
    fn record(&self, _bot_id: &str, _stream: &str, _seq: u64) {}
}

fn server_frame(frame_type: u8, seq: u64, payload: serde_json::Value) -> WsMessage {
    let bytes = frame::encode(
        Header {
            frame_type,
            seq,
            msg_id: [seq as u8; 16],
            conversation_id: [0x22; 16],
            ..Default::default()
        },
        &serde_json::to_vec(&payload).unwrap(),
    )
    .unwrap();
    WsMessage::Binary(bytes.into())
}

/// The next frame of `frame_type` the bot sends within `wait`, skipping JOINs.
async fn next_of<S>(ws: &mut S, frame_type: u8, wait: Duration) -> Option<serde_json::Value>
where
    S: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let msg = match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(m))) => m,
            _ => return None,
        };
        let WsMessage::Binary(data) = msg else {
            continue;
        };
        let (h, payload) = frame::decode(&data).unwrap();
        if h.frame_type == frame_type {
            return Some(serde_json::from_slice(payload).unwrap());
        }
    }
}

#[tokio::test]
async fn a_parked_bot_declines_or_holds_its_deliveries() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handled = Arc::new(AtomicUsize::new(0));

    let plugin = comm::NeboAIPlugin::new(Arc::new(NoOffsets));
    let seen = handled.clone();
    plugin.set_message_handler(Arc::new(move |_msg| {
        seen.fetch_add(1, Ordering::SeqCst);
    }));
    let config = HashMap::from([
        ("gateway".to_string(), format!("ws://{addr}/ws")),
        ("bot_id".to_string(), "bot-under-test".to_string()),
        ("token".to_string(), "test-token".to_string()),
        ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
    ]);

    let hub = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.next().await.unwrap().unwrap(); // CONNECT
        ws.send(server_frame(
            frame::TYPE_AUTH_OK,
            0,
            serde_json::json!({"ok": true}),
        ))
        .await
        .unwrap();

        // Nothing here parks: declined, with the reason.
        let ask =
            serde_json::json!({"type": "passivate", "idleForSecs": 1800, "minNextWakeSecs": 600});
        ws.send(server_frame(frame::TYPE_RESIDENCY, 0, ask.clone()))
            .await
            .unwrap();
        let busy = next_of(&mut ws, frame::TYPE_RESIDENCY, Duration::from_secs(5))
            .await
            .expect("a busy answer");
        assert_eq!(busy["type"], "busy");
        assert_eq!(busy["reason"], "this bot does not park");

        // A delivery before parking is handled and acked.
        ws.send(server_frame(frame::TYPE_MESSAGE_DELIVERY, 7, serde_json::json!({"senderId": "owner", "stream": "chat", "content": {"text": "before"}})))
            .await
            .unwrap();
        let ack = next_of(&mut ws, frame::TYPE_ACK, Duration::from_secs(5))
            .await
            .expect("the delivery is acked");
        assert_eq!(ack["ackedSeq"], 7);

        // Now it parks (the test subscribes and decides Parking): nothing
        // goes back, and a delivery after that is neither handled nor acked.
        let mut requests = comm::residency::subscribe();
        ws.send(server_frame(frame::TYPE_RESIDENCY, 0, ask))
            .await
            .unwrap();
        let req = requests.recv().await.unwrap();
        assert_eq!(req.passivate.idle_for, Duration::from_secs(1800));
        comm::residency::hold_deliveries();
        req.answer.send(comm::residency::Decision::Parking).unwrap();
        ws.send(server_frame(frame::TYPE_MESSAGE_DELIVERY, 8, serde_json::json!({"senderId": "owner", "stream": "chat", "content": {"text": "after"}})))
            .await
            .unwrap();
        assert!(
            next_of(&mut ws, frame::TYPE_ACK, Duration::from_secs(2))
                .await
                .is_none(),
            "a held delivery must not be acked"
        );
        // And parking answers nothing.
    });

    plugin.connect(config).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), hub)
        .await
        .expect("hub finished")
        .unwrap();
    assert_eq!(
        handled.load(Ordering::SeqCst),
        1,
        "only the delivery before parking was handled"
    );
}
