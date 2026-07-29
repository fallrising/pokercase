mod admin;
mod claude;
mod config;
mod cooldown;
mod error;
mod proxy;
mod resolve;
mod secrets;
mod server;
mod store;
mod templates;
mod tui_app;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::AppConfig;

#[derive(Parser, Debug)]
#[command(
    name = "thinrouter",
    about = "Thin OpenAI-compatible LLM proxy (GitHub: fallrising/pokercase)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start HTTP proxy + web admin
    Serve {
        /// Bind address
        #[arg(long, env = "THINROUTER_HOST", default_value = "127.0.0.1")]
        host: String,
        /// Bind port
        #[arg(long, env = "THINROUTER_PORT", default_value_t = 20128)]
        port: u16,
        /// Data directory (SQLite, etc.)
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        /// Optional admin token for /admin (UI login + x-admin-token / cookie)
        #[arg(long, env = "THINROUTER_ADMIN_TOKEN")]
        admin_token: Option<String>,
        /// Optional passphrase to encrypt connection API keys at rest
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
        /// SSE stall timeout in seconds (abort stream if no chunk)
        #[arg(long, env = "THINROUTER_SSE_STALL_SECS", default_value_t = 90)]
        sse_stall_secs: u64,
    },
    /// Print paths and basic status
    Doctor {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
    },
    /// Terminal UI for connections / routes / usage
    Tui {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Serve {
            host,
            port,
            data_dir,
            admin_token,
            secrets_key,
            sse_stall_secs,
        } => {
            let cfg = AppConfig::new(
                host,
                port,
                data_dir,
                admin_token,
                secrets_key,
                Some(sse_stall_secs),
            )?;
            server::run(cfg).await
        }
        Commands::Doctor {
            data_dir,
            secrets_key,
        } => {
            let cfg = AppConfig::new("127.0.0.1".into(), 20128, data_dir, None, secrets_key, None)?;
            doctor(&cfg)
        }
        Commands::Tui {
            data_dir,
            secrets_key,
        } => {
            let cfg = AppConfig::new("127.0.0.1".into(), 20128, data_dir, None, secrets_key, None)?;
            tui_app::run_tui(&cfg)
        }
    }
}

fn doctor(cfg: &AppConfig) -> Result<()> {
    println!("thinrouter doctor");
    println!("  crate    : thinrouter");
    println!("  repo     : fallrising/pokercase (working name: thinrouter)");
    println!("  data_dir : {}", cfg.data_dir.display());
    println!("  db_path  : {}", cfg.db_path().display());
    println!(
        "  secrets  : {}",
        if cfg.secrets_key.is_some() {
            "encryption enabled"
        } else {
            "plaintext api keys"
        }
    );
    let store = store::Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;
    let conns = store.list_connections()?;
    let routes = store.list_routes()?;
    let keys = store.list_api_keys()?;
    let cost = store.usage_cost_total()?;
    println!("  connections: {}", conns.len());
    println!("  routes     : {}", routes.len());
    println!("  api_keys   : {}", keys.len());
    println!("  est. cost  : ${cost:.6}");
    println!("ok");
    Ok(())
}
