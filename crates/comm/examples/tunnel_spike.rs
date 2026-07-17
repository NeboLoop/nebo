//! Manual spike for the management tunnel (docs/plans/nebo-cloud-architecture.md
//! "Verification"): dial a hub and proxy its streams to a local server.
//!
//! Usage: cargo run -p nebo-comm --example tunnel_spike -- <hub_ws_url> <local_addr>

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let hub_url = args.next().expect("usage: tunnel_spike <hub_ws_url> <local_addr>");
    let local_addr = args.next().expect("usage: tunnel_spike <hub_ws_url> <local_addr>");
    match nebo_comm::tunnel::run(&hub_url, "spike-token", &local_addr).await {
        Ok(()) => println!("tunnel closed cleanly"),
        Err(e) => eprintln!("tunnel error: {e}"),
    }
}
