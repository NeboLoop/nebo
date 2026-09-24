//! Live BotLease harness: one test process = one bot instance, driving the
//! real `NeboAIPlugin` against a hub. Run it twice (two processes) to watch
//! the hub fence the second one. Never point it at production, and never
//! use a real bot's id or token: a scratch hub and a scratch bot only.
//!
//! ```text
//! NEBO_LEASE_LIVE_GATEWAY=ws://127.0.0.1:19400/ws \
//! NEBO_LEASE_LIVE_BOT=<scratch bot id> NEBO_LEASE_LIVE_TOKEN_FILE=/tmp/token \
//! NEBO_LEASE_LIVE_HOLD_SECS=90 [NEBO_LEASE_LIVE_RETRY=1] \
//!   cargo test -p nebo-comm --test lease_live -- --ignored --nocapture
//! ```
//!
//! It prints one line per event: `GRANTED epoch=N after=Ss`, `REFUSED
//! lease_held`, and every 5 s `STATE …` while it holds. The token file is
//! rewritten with the rotated token so a second instance presents the
//! token the hub currently honours.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use comm::CommPlugin;
use nebo_comm as comm;

/// A scratch bot's streams start from nothing: every JOIN carries offset 0,
/// as a first run does.
struct NoOffsets;

impl comm::StreamOffsets for NoOffsets {
    fn acked(&self, _bot_id: &str, _stream: &str) -> u64 {
        0
    }
    fn record(&self, _bot_id: &str, _stream: &str, _seq: u64) {}
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

#[tokio::test]
#[ignore = "live: needs a scratch hub (see the module docs)"]
async fn hold_the_bot() {
    let gateway = env("NEBO_LEASE_LIVE_GATEWAY");
    assert!(
        gateway.starts_with("ws://127.0.0.1") || gateway.starts_with("ws://localhost"),
        "this harness only talks to a local scratch hub"
    );
    let token_file = env("NEBO_LEASE_LIVE_TOKEN_FILE");
    let hold = Duration::from_secs(env("NEBO_LEASE_LIVE_HOLD_SECS").parse().expect("seconds"));
    let retry = std::env::var("NEBO_LEASE_LIVE_RETRY").is_ok();
    let lease = comm::lease::process();
    lease.set_fenced(true);
    println!("INSTANCE {}", lease.instance_id());

    let plugin = comm::NeboAIPlugin::new(Arc::new(NoOffsets));
    let started = Instant::now();
    loop {
        let mut config = HashMap::new();
        config.insert("gateway".to_string(), gateway.clone());
        config.insert("api_server".to_string(), "http://127.0.0.1:9".to_string());
        config.insert("bot_id".to_string(), env("NEBO_LEASE_LIVE_BOT"));
        config.insert(
            "token".to_string(),
            std::fs::read_to_string(&token_file)
                .expect("token")
                .trim()
                .to_string(),
        );
        match plugin.connect(config).await {
            Ok(()) => break,
            Err(comm::CommError::LeaseHeld) => {
                println!(
                    "REFUSED lease_held after={}s frozen={}",
                    started.elapsed().as_secs(),
                    lease.frozen()
                );
                if !retry {
                    return;
                }
                tokio::time::sleep(comm::lease::RENEW_EVERY).await;
            }
            Err(e) => panic!("connect: {e}"),
        }
    }
    println!(
        "GRANTED epoch={} after={}s",
        lease.epoch(),
        started.elapsed().as_secs()
    );
    if let Some(token) = plugin.take_rotated_token().await {
        std::fs::write(&token_file, token).expect("write rotated token");
    }

    let until = Instant::now() + hold;
    while Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(5)).await;
        println!(
            "STATE {:?} frozen={} connected={} t={}s",
            lease.state(),
            lease.frozen(),
            plugin.is_connected(),
            started.elapsed().as_secs()
        );
    }
}
