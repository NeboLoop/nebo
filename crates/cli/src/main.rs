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
    /// First-run setup wizard
    Onboard,
    /// List platform capabilities
    Capabilities,
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List,
    /// Delete a session
    Delete { id: String },
}

#[derive(Subcommand)]
enum SkillsCommands {
    /// List available skills
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

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
            println!("Nebo Doctor v{VERSION}");
            println!("Data dir: {:?}", config::data_dir()?);
            println!("Setup complete: {}", config::is_setup_complete()?);
            println!("Bot ID: {:?}", config::read_bot_id());
            println!("DB path: {}", cfg.database.sqlite_path);

            // Test database connection
            match db::Store::new(&cfg.database.sqlite_path) {
                Ok(store) => {
                    let count = store.count_users().unwrap_or(0);
                    println!("Database: OK ({count} users)");
                }
                Err(e) => println!("Database: ERROR - {e}"),
            }
        }
        Some(Commands::Chat { interactive: _, dangerously: _, prompt }) => {
            println!("Chat mode (not yet implemented in Rust port)");
            if let Some(p) = prompt {
                println!("Prompt: {p}");
            }
        }
        Some(Commands::Agent) => {
            println!("Agent mode (not yet implemented in Rust port)");
        }
        Some(Commands::Session { command }) => match command {
            SessionCommands::List => {
                println!("Session list (not yet implemented in Rust port)");
            }
            SessionCommands::Delete { id } => {
                println!("Delete session {id} (not yet implemented in Rust port)");
            }
        },
        Some(Commands::Skills { command }) => match command {
            SkillsCommands::List => {
                println!("Skills list (not yet implemented in Rust port)");
            }
        },
        Some(Commands::Onboard) => {
            println!("Onboard wizard (not yet implemented in Rust port)");
        }
        Some(Commands::Capabilities) => {
            println!("Platform capabilities (not yet implemented in Rust port)");
        }
    }

    Ok(())
}
