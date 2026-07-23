mod mcp_serve;

use std::sync::Mutex;

use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

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
    /// MCP (Model Context Protocol) server for Claude Desktop, Cursor, etc.
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// Prompt testing and optimization harness
    Test {
        #[command(subcommand)]
        command: TestCommands,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    /// Start MCP stdio bridge (requires a running Nebo server)
    Serve {
        /// Comma-separated tool allowlist (e.g. "system,web,bot")
        #[arg(long)]
        tools: Option<String>,
        /// Comma-separated tool denylist (e.g. "desktop,organizer")
        #[arg(long)]
        exclude_tools: Option<String>,
    },
    /// Print MCP configuration for a target application
    Config {
        /// Target application
        #[arg(long, value_enum, default_value = "claude-desktop")]
        target: mcp_serve::ConfigTarget,
    },
}

#[derive(Subcommand)]
enum TestCommands {
    /// Inspect the assembled system prompt
    Prompt {
        /// Fixture YAML file (populates agent name from fixture)
        #[arg(long)]
        fixture: Option<String>,
        /// Component overrides: "tool.shell:./overrides/shell-v2.md"
        #[arg(long = "override")]
        overrides: Option<Vec<String>>,
    },
    /// Run fixture(s) live against a running Nebo server
    Run {
        /// Single fixture YAML
        #[arg(long)]
        fixture: Option<String>,
        /// Suite YAML (list of fixture paths)
        #[arg(long)]
        suite: Option<String>,
        /// Component overrides
        #[arg(long = "override")]
        overrides: Option<Vec<String>>,
        /// Model to use (overrides server default)
        #[arg(long)]
        model: Option<String>,
        /// Grader model for LLM-as-judge evaluation
        #[arg(long)]
        grader: Option<String>,
        /// Number of runs per fixture (for variance measurement)
        #[arg(long, default_value = "1")]
        runs: usize,
        /// Nebo server URL
        #[arg(long, default_value = "localhost:27895")]
        server: String,
        /// Save trace JSON to this directory
        #[arg(long)]
        output: Option<String>,
        /// Baseline trace directory for comparison
        #[arg(long)]
        baseline: Option<String>,
        /// Output JSON instead of tables
        #[arg(long)]
        json: bool,
        /// Experiment name (enables statistical analysis and history tracking)
        #[arg(long)]
        experiment: Option<String>,
    },
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
    // The `nebo relay` subcommand also reaches the relay via clap.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a.starts_with("chrome-extension://")) {
            return browser::extension_relay::run(config::read_extension_secret()).await;
        }
    }

    // Initialize tracing — terminal + file
    let env_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = fmt::layer().with_filter(env_filter());

    let file_layer = config::data_dir().ok().and_then(|dir| {
        let log_dir = dir.join("logs");
        std::fs::create_dir_all(&log_dir).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("nebo.log"))
            .ok()?;
        Some(
            fmt::layer()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .with_filter(env_filter()),
        )
    });

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();

    // Install panic hook so panics are logged before the process dies
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".into()
        };
        tracing::error!(location = %location, "PANIC: {}", payload);
        eprintln!("PANIC at {}: {}", location, payload);
        // Also write to log file directly in case tracing is broken
        if let Ok(dir) = config::data_dir() {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("logs/nebo-crash.log"))
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "PANIC at {}: {}", location, payload)
                });
        }
    }));

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
        Some(Commands::Chat {
            interactive,
            dangerously: _,
            prompt,
        }) => {
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
                        println!(
                            "{:<36}  {:<20}  {:<10}  {}",
                            "ID", "Name", "Messages", "Created"
                        );
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
                    if !path.is_dir() {
                        continue;
                    }
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
                        println!(
                            "{:<36}  {:<15}  {:<12}  {:<8}  {}",
                            "ID", "Name", "Provider", "Active", "Model"
                        );
                        println!("{}", "-".repeat(90));
                        for p in &profiles {
                            let active = if p.is_active.unwrap_or(0) != 0 {
                                "yes"
                            } else {
                                "no"
                            };
                            let model = p.model.as_deref().unwrap_or("-");
                            println!(
                                "{:<36}  {:<15}  {:<12}  {:<8}  {}",
                                p.id, p.name, p.provider, active, model
                            );
                        }
                    }
                }
                ProviderCommands::Test { id } => match store.get_auth_profile(&id)? {
                    Some(profile) => {
                        println!("Testing provider: {} ({})", profile.name, profile.provider);
                        if profile.api_key.is_empty()
                            && profile.auth_type.as_deref() != Some("local")
                        {
                            println!("  FAIL: No API key configured");
                        } else {
                            println!("  OK: Configuration looks valid");
                        }
                    }
                    None => println!("Provider not found: {id}"),
                },
            }
        }
        Some(Commands::Onboard) => {
            run_onboard(&cfg)?;
        }
        Some(Commands::Relay) => {
            browser::extension_relay::run(config::read_extension_secret()).await?;
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
        Some(Commands::Mcp { command }) => match command {
            McpCommands::Config { target } => {
                mcp_serve::print_config(&target);
            }
            McpCommands::Serve {
                tools,
                exclude_tools,
            } => {
                let server_url = format!("http://{}:{}", cfg.host, cfg.port);
                let bridge = mcp_serve::McpStdioBridge::new(server_url, tools, exclude_tools);
                bridge.run().await?;
            }
        },
        Some(Commands::Test { command }) => {
            run_test_command(command).await?;
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
            println!(
                "  Sessions:  {}",
                if sessions_list.is_empty() { 0 } else { 1 }
            );
            println!("  Providers: {}", providers.len());
        }
        Err(e) => println!("Database: ERROR - {e}"),
    }
    println!();

    // Skills
    let skills_dir = data_dir.join("skills");
    if skills_dir.exists() {
        let count = std::fs::read_dir(&skills_dir)
            .map(|d| {
                d.flatten()
                    .filter(|e| {
                        let p = e.path();
                        p.is_dir()
                            && (p.join("SKILL.md").exists() || p.join("SKILL.md.disabled").exists())
                    })
                    .count()
            })
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
    // The CLI is a thin client of the RUNNING server's one chat pipeline
    // (ws → dispatch_chat → Runner), exactly like the web UI — so the agent
    // has its full tool registry, memory, and steering. The previous
    // implementation called the provider directly with zero tools and no
    // system prompt, a competing pathway on which tool use was impossible.
    let url = format!("ws://{}:{}/ws", cfg.host, cfg.port);
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| {
        anyhow::anyhow!("cannot reach nebo server at {url} ({e}) — is `nebo serve` running?")
    })?;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    let (mut tx, mut rx) = ws.split();

    // Auth handshake (the local server trusts the connection; it replies auth_ok).
    tx.send(WsMessage::Text(r#"{"type":"auth"}"#.to_string().into()))
        .await?;

    // Unique session per invocation so test runs never bleed into each other
    // or into the user's UI chats.
    let session_id = format!(
        "agent:assistant:cli-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    let send_prompt = |text: String| {
        let msg = serde_json::json!({
            "type": "chat",
            "message_id": format!("cli-{}-{}", session_id, uuid::Uuid::new_v4()),
            "data": {
                "session_id": session_id,
                "prompt": text,
                "agent_id": "assistant",
                "channel": "cli",
            },
        });
        msg.to_string()
    };

    // Stream events for OUR session until the run completes. Returns false on
    // socket close.
    async fn pump(
        rx: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
              + Unpin),
        session_id: &str,
    ) -> anyhow::Result<bool> {
        let timeout = std::time::Duration::from_secs(300);
        loop {
            let Ok(next) = tokio::time::timeout(timeout, rx.next()).await else {
                anyhow::bail!("timed out after {}s waiting for the run", timeout.as_secs());
            };
            let Some(msg) = next else { return Ok(false) };
            let WsMessage::Text(text) = msg? else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let data = &v["data"];
            if data["session_id"].as_str() != Some(session_id) {
                continue;
            }
            match v["type"].as_str().unwrap_or("") {
                "chat_stream" => {
                    if let Some(c) = data["content"].as_str() {
                        print!("{c}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                }
                "activity" => {
                    if let Some(label) = data["label"].as_str() {
                        eprintln!("[tool] {label}");
                    }
                }
                "chat_error" => {
                    println!();
                    anyhow::bail!(
                        "chat error: {}",
                        data["error"].as_str().unwrap_or("unknown")
                    );
                }
                "chat_complete" => {
                    println!();
                    return Ok(true);
                }
                _ => {}
            }
        }
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
                    tx.send(WsMessage::Text(send_prompt(line).into())).await?;
                    if !pump(&mut rx, &session_id).await? {
                        break;
                    }
                }
                None => break,
            }
        }
    } else if let Some(prompt) = prompt {
        tx.send(WsMessage::Text(send_prompt(prompt).into())).await?;
        pump(&mut rx, &session_id).await?;
    } else {
        println!("Usage: nebo chat 'your prompt' or nebo chat -i for interactive mode");
    }

    Ok(())
}

async fn run_test_command(command: TestCommands) -> anyhow::Result<()> {
    use agent::testing::{engine, fixture, grader, reporter, trace};
    use std::path::Path;

    match command {
        TestCommands::Prompt { fixture, overrides } => {
            let overrides = engine::parse_overrides(&overrides.unwrap_or_default())
                .map_err(|e| anyhow::anyhow!(e))?;
            let fix = fixture
                .as_deref()
                .map(|p| fixture::load_fixture(Path::new(p)))
                .transpose()
                .map_err(|e| anyhow::anyhow!(e))?;
            engine::inspect_prompt(fix.as_ref(), &overrides);
        }
        TestCommands::Run {
            fixture: fixture_path,
            suite,
            overrides,
            model,
            grader: grader_model,
            runs,
            server,
            output,
            baseline,
            json,
            experiment,
        } => {
            let overrides = engine::parse_overrides(&overrides.unwrap_or_default())
                .map_err(|e| anyhow::anyhow!(e))?;

            let fixtures = resolve_fixtures(fixture_path.as_deref(), suite.as_deref())?;
            if fixtures.is_empty() {
                anyhow::bail!("No fixtures specified. Use --fixture or --suite.");
            }

            let mut all_traces: Vec<trace::Trace> = Vec::new();
            let mut all_candidate_scores: Vec<trace::FixtureScores> = Vec::new();

            let mut failed_fixtures: Vec<String> = Vec::new();

            for fix in &fixtures {
                println!("Running fixture: {} ({}x)", fix.id, runs);

                let mut traces = match engine::run_live(fix, &server, model.as_deref(), &overrides, runs).await {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("  FAILED: {}", e);
                        failed_fixtures.push(fix.id.clone());
                        continue;
                    }
                };

                if let Some(ref grader_model) = grader_model {
                    for trace in &mut traces {
                        match grader::grade(trace, fix, &server, grader_model).await {
                            Ok(grade) => trace.grade = Some(grade),
                            Err(e) => eprintln!("  grading failed: {}", e),
                        }
                    }
                }

                if json {
                    reporter::print_json_report(&traces);
                } else {
                    reporter::print_report(fix, &traces);
                }

                if let Some(ref output_dir) = output {
                    let dir = Path::new(output_dir);
                    for trace in &traces {
                        if let Err(e) = trace.save(dir) {
                            eprintln!("  save failed: {}", e);
                        }
                    }
                    println!("  Traces saved to {}", dir.display());
                }

                if baseline.is_some() || experiment.is_some() {
                    all_candidate_scores.push(trace::FixtureScores::from_traces(&fix.id, &traces));
                }

                if let Some(ref baseline_dir) = baseline {
                    if let Ok(baseline_traces) = trace::Trace::load_dir(Path::new(baseline_dir)) {
                        if !baseline_traces.is_empty() {
                            reporter::print_comparison(&baseline_traces, &traces);
                        }
                    }
                }

                all_traces.extend(traces);
            }

            if !failed_fixtures.is_empty() {
                eprintln!("\n  {} fixture(s) failed: {}", failed_fixtures.len(), failed_fixtures.join(", "));
            }

            if let Some(ref exp_name) = experiment {
                let metadata = engine::build_experiment_metadata(exp_name, &overrides, runs);

                // Load baseline scores if provided
                let baseline_scores = if let Some(ref baseline_dir) = baseline {
                    let baseline_traces = trace::Trace::load_dir(Path::new(baseline_dir))
                        .map_err(|e| anyhow::anyhow!(e))?;
                    // Group baseline traces by fixture_id
                    let mut by_fixture: std::collections::HashMap<String, Vec<&trace::Trace>> = std::collections::HashMap::new();
                    for t in &baseline_traces {
                        by_fixture.entry(t.fixture_id.clone()).or_default().push(t);
                    }
                    by_fixture.into_iter().map(|(fid, traces)| {
                        let owned: Vec<trace::Trace> = traces.into_iter().cloned().collect();
                        trace::FixtureScores::from_traces(&fid, &owned)
                    }).collect::<Vec<_>>()
                } else {
                    Vec::new()
                };

                let result = reporter::compute_experiment_result(
                    metadata,
                    &baseline_scores,
                    &all_candidate_scores,
                );

                reporter::print_experiment_result(&result);

                if let Some(ref output_dir) = output {
                    let exp_dir = Path::new(output_dir).join(exp_name);
                    engine::save_experiment(&exp_dir, &result, &all_traces)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("  Experiment saved to {}", exp_dir.display());

                    reporter::append_history(Path::new(output_dir), &result)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("  History updated: {}/history.jsonl", output_dir);
                }
            }
        }
    }
    Ok(())
}

fn resolve_fixtures(
    fixture_path: Option<&str>,
    suite_path: Option<&str>,
) -> anyhow::Result<Vec<agent::testing::fixture::Fixture>> {
    use agent::testing::fixture;
    use std::path::Path;

    let mut fixtures = Vec::new();

    if let Some(path) = fixture_path {
        fixtures.push(
            fixture::load_fixture(Path::new(path)).map_err(|e| anyhow::anyhow!(e))?,
        );
    }

    if let Some(path) = suite_path {
        let suite_dir = Path::new(path).parent().unwrap_or(Path::new("."));
        let suite =
            fixture::load_suite(Path::new(path)).map_err(|e| anyhow::anyhow!(e))?;
        for fixture_rel in &suite.fixtures {
            let full_path = suite_dir.join(fixture_rel);
            fixtures.push(
                fixture::load_fixture(&full_path).map_err(|e| anyhow::anyhow!(e))?,
            );
        }
    }

    Ok(fixtures)
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
