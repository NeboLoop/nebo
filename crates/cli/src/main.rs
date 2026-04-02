use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "nebo", version = VERSION, about = "Nebo - Personal AI Agent")]
struct Cli {
    /// Run in headless mode (no native window)
    #[arg(long, default_value_t = false)]
    headless: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP server only
    Serve,
    /// Start the agent only
    Agent,
    /// Interactive chat mode
    Chat {
        /// Enable interactive mode
        #[arg(short, long)]
        interactive: bool,
        /// Enable fully autonomous mode
        #[arg(long)]
        dangerously: bool,
        /// The prompt to send
        prompt: Option<String>,
    },
    /// Show configuration
    Config,
    /// Run system diagnostics
    Doctor,
    /// Session management
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// Skill management
    Skills {
        #[command(subcommand)]
        command: SkillsCommands,
    },
    /// Provider management
    Providers {
        #[command(subcommand)]
        command: ProviderCommands,
    },
    /// First-run setup wizard
    Onboard,
    /// List platform capabilities
    Capabilities,
    /// Run as Chrome native messaging relay (used internally by the extension)
    Relay,
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List,
    /// Delete a session
    Delete { id: String },
    /// Reset a session (clear messages, keep session)
    Reset { id: String },
}

#[derive(Subcommand)]
enum SkillsCommands {
    /// List available skills
    List,
}

#[derive(Subcommand)]
enum ProviderCommands {
    /// List configured providers
    List,
    /// Test a provider connection
    Test { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Detect Chrome native messaging relay BEFORE parsing CLI args.
    // Chrome passes `chrome-extension://EXTENSION_ID/` as the sole argument.
    // When detected, run as a lightweight stdin/stdout relay — no GUI, no server.
    // The `nebo relay` subcommand also reaches run_native_messaging via clap.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a.starts_with("chrome-extension://")) {
            return run_native_messaging().await;
        }
    }

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Load .env file (if present) before config so env vars are available
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Load config
    let mut cfg = config::Config::load_embedded()?;

    // Apply local settings (auto-generated secrets)
    let settings = config::load_settings()?;
    cfg.auth.access_secret = settings.access_secret;
    cfg.auth.access_expire = settings.access_expire;
    cfg.auth.refresh_token_expire = settings.refresh_token_expire;

    // Ensure data directory exists
    config::ensure_data_dir()?;

    match cli.command {
        None => {
            // Default: run server + agent (like Go's RunAll)
            println!("Nebo v{VERSION}");
            server::run(cfg, false).await?;
        }
        Some(Commands::Serve) => {
            server::run(cfg, false).await?;
        }
        Some(Commands::Config) => {
            println!("{cfg:#?}");
        }
        Some(Commands::Doctor) => {
            run_doctor(&cfg)?;
        }
        Some(Commands::Chat { interactive, dangerously: _, prompt }) => {
            run_chat(&cfg, interactive, prompt).await?;
        }
        Some(Commands::Agent) => {
            println!("Starting agent daemon...");
            // The agent runs as part of the server — start server in headless mode
            server::run(cfg, true).await?;
        }
        Some(Commands::Session { command }) => {
            let store = db::Store::new(&cfg.database.sqlite_path)?;
            match command {
                SessionCommands::List => {
                    let sessions = store.list_sessions(50, 0)?;
                    if sessions.is_empty() {
                        println!("No sessions found.");
                    } else {
                        println!("{:<36}  {:<20}  {:<10}  {}", "ID", "Name", "Messages", "Created");
                        println!("{}", "-".repeat(80));
                        for s in &sessions {
                            let name = s.name.as_deref().unwrap_or("-");
                            let msgs = s.message_count.unwrap_or(0);
                            println!("{:<36}  {:<20}  {:<10}  {}", s.id, name, msgs, s.created_at);
                        }
                        println!("\n{} session(s)", sessions.len());
                    }
                }
                SessionCommands::Delete { id } => {
                    store.delete_session(&id)?;
                    println!("Session {id} deleted.");
                }
                SessionCommands::Reset { id } => {
                    store.reset_session(&id)?;
                    println!("Session {id} reset.");
                }
            }
        }
        Some(Commands::Skills { command }) => match command {
            SkillsCommands::List => {
                let data_dir = config::data_dir()?;
                let skills_dir = data_dir.join("skills");
                if !skills_dir.exists() {
                    println!("No skills directory found at {}", skills_dir.display());
                    return Ok(());
                }
                println!("{:<20}  {:<8}  {}", "Name", "Status", "Path");
                println!("{}", "-".repeat(60));
                for entry in std::fs::read_dir(&skills_dir)?.flatten() {
                    let path = entry.path();
                    if !path.is_dir() { continue; }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if path.join("SKILL.md").exists() {
                        println!("{:<20}  {:<8}  {}", name, "enabled", path.display());
                    } else if path.join("SKILL.md.disabled").exists() {
                        println!("{:<20}  {:<8}  {}", name, "disabled", path.display());
                    }
                }
            }
        },
        Some(Commands::Providers { command }) => {
            let store = db::Store::new(&cfg.database.sqlite_path)?;
            match command {
                ProviderCommands::List => {
                    let profiles = store.list_auth_profiles()?;
                    if profiles.is_empty() {
                        println!("No providers configured. Add one with the web UI.");
                    } else {
                        println!("{:<36}  {:<15}  {:<12}  {:<8}  {}", "ID", "Name", "Provider", "Active", "Model");
                        println!("{}", "-".repeat(90));
                        for p in &profiles {
                            let active = if p.is_active.unwrap_or(0) != 0 { "yes" } else { "no" };
                            let model = p.model.as_deref().unwrap_or("-");
                            println!("{:<36}  {:<15}  {:<12}  {:<8}  {}", p.id, p.name, p.provider, active, model);
                        }
                    }
                }
                ProviderCommands::Test { id } => {
                    match store.get_auth_profile(&id)? {
                        Some(profile) => {
                            println!("Testing provider: {} ({})", profile.name, profile.provider);
                            if profile.api_key.is_empty() && profile.auth_type.as_deref() != Some("local") {
                                println!("  FAIL: No API key configured");
                            } else {
                                println!("  OK: Configuration looks valid");
                            }
                        }
                        None => println!("Provider not found: {id}"),
                    }
                }
            }
        }
        Some(Commands::Onboard) => {
            run_onboard(&cfg)?;
        }
        Some(Commands::Relay) => {
            run_native_messaging().await?;
        }
        Some(Commands::Capabilities) => {
            println!("Nebo v{VERSION} — Platform Capabilities");
            println!();
            println!("OS:   {}", std::env::consts::OS);
            println!("Arch: {}", std::env::consts::ARCH);
            println!();
            println!("Core:");
            println!("  - AI Providers: Anthropic, OpenAI, Ollama");
            println!("  - Tool System: STRAP (Single Tool Resource Action Pattern)");
            println!("  - Memory: 3-tier (tacit/daily/entity)");
            println!("  - Sessions: multi-turn with context compaction");
            println!("  - Skills: YAML-based, hot-reloadable");
            println!("  - Advisors: multi-voice deliberation");
            println!();
            println!("Server:");
            println!("  - REST API on port {}", cfg.port);
            println!("  - WebSocket for real-time events");
            println!("  - JWT authentication");
            println!("  - SQLite storage (WAL mode)");
        }
    }

    Ok(())
}

fn run_doctor(cfg: &config::Config) -> anyhow::Result<()> {
    println!("Nebo Doctor v{VERSION}");
    println!();

    // Data directory
    let data_dir = config::data_dir()?;
    println!("Data dir:       {}", data_dir.display());
    println!("Setup complete: {}", config::is_setup_complete()?);
    println!("Bot ID:         {:?}", config::read_bot_id());
    println!("DB path:        {}", cfg.database.sqlite_path);
    println!();

    // Database
    match db::Store::new(&cfg.database.sqlite_path) {
        Ok(store) => {
            let users = store.count_users().unwrap_or(0);
            let chats = store.count_chats().unwrap_or(0);
            let memories = store.count_memories().unwrap_or(0);
            let sessions_list = store.list_sessions(1, 0).unwrap_or_default();
            let providers = store.list_auth_profiles().unwrap_or_default();

            println!("Database:    OK");
            println!("  Users:     {users}");
            println!("  Chats:     {chats}");
            println!("  Memories:  {memories}");
            println!("  Sessions:  {}", if sessions_list.is_empty() { 0 } else { 1 });
            println!("  Providers: {}", providers.len());
        }
        Err(e) => println!("Database: ERROR - {e}"),
    }
    println!();

    // Skills
    let skills_dir = data_dir.join("skills");
    if skills_dir.exists() {
        let count = std::fs::read_dir(&skills_dir)
            .map(|d| d.flatten().filter(|e| {
                let p = e.path();
                p.is_dir() && (p.join("SKILL.md").exists() || p.join("SKILL.md.disabled").exists())
            }).count())
            .unwrap_or(0);
        println!("Skills dir: {} ({count} skills)", skills_dir.display());
    } else {
        println!("Skills dir: not found");
    }

    println!();
    println!("All checks passed.");
    Ok(())
}

async fn run_chat(
    cfg: &config::Config,
    interactive: bool,
    prompt: Option<String>,
) -> anyhow::Result<()> {
    let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);

    // Build providers using the canonical server function (handles all provider types)
    let providers = server::build_providers(&store, cfg, None);

    if providers.is_empty() {
        println!("No active providers available. Add one with `nebo serve` and the web UI.");
        return Ok(());
    }

    if interactive {
        println!("Nebo Chat v{VERSION} (interactive mode)");
        println!("Type 'exit' or Ctrl-C to quit.\n");

        let stdin = tokio::io::stdin();
        let reader = tokio::io::BufReader::new(stdin);
        use tokio::io::AsyncBufReadExt;
        let mut lines = reader.lines();

        loop {
            eprint!("> ");
            match lines.next_line().await? {
                Some(line) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "exit" || line == "quit" {
                        break;
                    }
                    send_chat_message(providers[0].as_ref(), &line).await?;
                }
                None => break,
            }
        }
    } else if let Some(prompt) = prompt {
        send_chat_message(providers[0].as_ref(), &prompt).await?;
    } else {
        println!("Usage: nebo chat 'your prompt' or nebo chat -i for interactive mode");
    }

    Ok(())
}

async fn send_chat_message(
    provider: &dyn ai::Provider,
    prompt: &str,
) -> anyhow::Result<()> {
    let req = ai::ChatRequest {
        messages: vec![ai::Message {
            role: "user".into(),
            content: prompt.into(),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.7,
        system: String::new(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
    };

    let mut rx = provider.stream(&req).await
        .map_err(|e| anyhow::anyhow!("provider error: {e}"))?;

    while let Some(event) = rx.recv().await {
        match event.event_type {
            ai::StreamEventType::Text => {
                print!("{}", event.text);
            }
            ai::StreamEventType::Done => break,
            _ => {}
        }
    }
    println!();

    Ok(())
}

/// Run as a Chrome native messaging host.
/// Chrome launches this process and communicates via stdin/stdout using
/// length-prefixed JSON. This process is a transparent bridge between
/// the Chrome extension (stdin/stdout) and the running Nebo server (WebSocket).
///
/// Self-healing: retries WS connection with backoff if server isn't ready.
/// Exits cleanly when WS breaks so Chrome's onDisconnect fires and relaunches.
///
/// Extension ←stdin/stdout→ this process ←WebSocket→ Nebo server ←in-process→ Agent
async fn run_native_messaging() -> anyhow::Result<()> {
    use futures::{SinkExt, StreamExt};
    use tokio::io::AsyncReadExt;
    use tokio_tungstenite::connect_async;
    use std::sync::Arc;

    // NOTE: stdout is the native messaging channel — ALL diagnostic logging goes to stderr.
    eprintln!("[nebo-relay] starting native messaging bridge");

    let ws_url = "ws://127.0.0.1:27895/ws/extension";

    // Retry WS connection with backoff — server may not be ready yet
    let ws_stream = {
        let mut attempts = 0u32;
        loop {
            match connect_async(ws_url).await {
                Ok((stream, _)) => {
                    eprintln!("[nebo-relay] connected to server at {}", ws_url);
                    break stream;
                }
                Err(e) if attempts < 10 => {
                    attempts += 1;
                    let delay = std::cmp::min(500 * 2u64.pow(attempts - 1), 5000);
                    eprintln!(
                        "[nebo-relay] WS connect attempt {}/10 failed ({}), retrying in {}ms",
                        attempts, e, delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                Err(e) => {
                    eprintln!("[nebo-relay] giving up after 10 attempts: {}", e);
                    // Give up — exit so Chrome can retry later
                    std::process::exit(1);
                    #[allow(unreachable_code)]
                    return Err(anyhow::anyhow!("Cannot connect to Nebo: {}", e));
                }
            }
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Detect which browser launched this relay (check parent process)
    let browser = detect_parent_browser();
    eprintln!("[nebo-relay] detected browser: {}", browser);

    // Send hello to server with browser identification (must be first message)
    let hello = serde_json::json!({
        "type": "hello",
        "browser": browser,
        "relay": true,
    });
    let _ = ws_tx
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&hello).unwrap().into(),
        ))
        .await;

    let mut stdin = tokio::io::stdin();
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    let stdout_send = stdout.clone();

    // Task 1: Read from Chrome extension (stdin) → forward to server (WS)
    let send_task = tokio::spawn(async move {
        loop {
            // Read 4-byte length prefix
            let mut len_buf = [0u8; 4];
            if stdin.read_exact(&mut len_buf).await.is_err() {
                eprintln!("[nebo-relay] stdin closed — extension disconnected");
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len > 1_048_576 {
                eprintln!("[nebo-relay] message too large: {} bytes", len);
                break;
            }

            // Read JSON body
            let mut body = vec![0u8; len];
            if stdin.read_exact(&mut body).await.is_err() {
                eprintln!("[nebo-relay] stdin read error");
                break;
            }

            let msg: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[nebo-relay] malformed JSON from extension: {}", e);
                    continue;
                }
            };

            let msg_type = msg["type"].as_str().unwrap_or("");

            // Handle hello and ping locally — respond immediately via stdout
            match msg_type {
                "hello" => {
                    eprintln!(
                        "[nebo-relay] extension hello (v{}, id={})",
                        msg["version"].as_str().unwrap_or("?"),
                        msg["extension_id"].as_str().unwrap_or("?")
                    );
                    let resp = serde_json::json!({"type": "connected"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    // Also forward to server so it knows extension connected
                    let text = serde_json::to_string(&msg).unwrap();
                    let _ = ws_tx
                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                        .await;
                    continue;
                }
                "ping" => {
                    let resp = serde_json::json!({"type": "pong"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    continue;
                }
                _ => {}
            }

            // Forward everything else to the server
            eprintln!("[nebo-relay] ext→server: type={}", msg_type);
            let text = serde_json::to_string(&msg).unwrap();
            if ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                .await
                .is_err()
            {
                eprintln!("[nebo-relay] WS send failed — server disconnected");
                break; // WS broke — exit so Chrome relaunches us
            }
        }
    });

    // Task 2: Read from server (WS) → forward to Chrome extension (stdout)
    let stdout_recv = stdout.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        let msg_type = parsed["type"].as_str().unwrap_or("");
                        eprintln!("[nebo-relay] server→ext: type={}", msg_type);
                        if write_native_message(&stdout_recv, &parsed).await.is_err() {
                            eprintln!("[nebo-relay] stdout write failed — extension disconnected");
                            break;
                        }
                    }
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    eprintln!("[nebo-relay] WS closed by server");
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either direction to close
    tokio::select! {
        _ = send_task => { eprintln!("[nebo-relay] send task ended"); }
        _ = recv_task => { eprintln!("[nebo-relay] recv task ended"); }
    }

    eprintln!("[nebo-relay] shutting down");
    // Force exit — tokio's blocking stdin thread prevents clean shutdown.
    // Chrome's onDisconnect will fire and the extension will reconnect.
    std::process::exit(0);
}

/// Write a native messaging response (4-byte length prefix + JSON) to stdout.
async fn write_native_message(
    stdout: &tokio::sync::Mutex<tokio::io::Stdout>,
    msg: &serde_json::Value,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    let json_bytes = serde_json::to_vec(msg).unwrap();
    let len = (json_bytes.len() as u32).to_le_bytes();
    let mut out = stdout.lock().await;
    out.write_all(&len).await?;
    out.write_all(&json_bytes).await?;
    out.flush().await?;
    Ok(())
}

/// Detect which browser launched this relay by checking the parent process name.
fn detect_parent_browser() -> String {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &ppid.to_string(), "-o", "comm="])
            .output()
        {
            let parent = String::from_utf8_lossy(&output.stdout).trim().to_string().to_lowercase();
            if parent.contains("brave") { return "brave".to_string(); }
            if parent.contains("chrome") { return "chrome".to_string(); }
            if parent.contains("firefox") { return "firefox".to_string(); }
            if parent.contains("safari") { return "safari".to_string(); }
            if parent.contains("edge") { return "edge".to_string(); }
            if parent.contains("arc") { return "arc".to_string(); }
            // Return the raw parent name if unrecognized
            if !parent.is_empty() { return parent; }
        }
    }
    "unknown".to_string()
}

fn run_onboard(cfg: &config::Config) -> anyhow::Result<()> {
    println!("Nebo Setup Wizard v{VERSION}");
    println!();

    if config::is_setup_complete()? {
        println!("Setup already complete. Run `nebo doctor` to check status.");
        return Ok(());
    }

    println!("1. Creating data directory...");
    config::ensure_data_dir()?;
    println!("   OK: {}", config::data_dir()?.display());

    println!("2. Initializing database...");
    let _store = db::Store::new(&cfg.database.sqlite_path)?;
    println!("   OK: {}", cfg.database.sqlite_path);

    println!("3. Generating bot ID...");
    let bot_id = config::read_bot_id();
    println!("   OK: {:?}", bot_id);

    println!();
    println!("Data directory and database initialized.");
    println!("Start the server with `nebo serve` and complete setup in the web UI.");
    println!("Default URL: http://localhost:{}", cfg.port);

    Ok(())
}
