mod admin;
mod config;
mod error;
mod proxy;
mod resolve;
mod server;
mod store;
mod templates;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::AppConfig;

#[derive(Parser, Debug)]
#[command(name = "thinrouter", about = "Thin OpenAI-compatible LLM proxy")]
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
        /// Optional admin token for /admin APIs (loopback is open if unset)
        #[arg(long, env = "THINROUTER_ADMIN_TOKEN")]
        admin_token: Option<String>,
    },
    /// Print paths and basic status
    Doctor {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
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
        } => {
            let cfg = AppConfig::new(host, port, data_dir, admin_token)?;
            server::run(cfg).await
        }
        Commands::Doctor { data_dir } => {
            let cfg = AppConfig::new("127.0.0.1".into(), 20128, data_dir, None)?;
            doctor(&cfg)
        }
    }
}

fn doctor(cfg: &AppConfig) -> Result<()> {
    println!("thinrouter doctor");
    println!("  data_dir : {}", cfg.data_dir.display());
    println!("  db_path  : {}", cfg.db_path().display());
    let store = store::Store::open(&cfg.db_path())?;
    let conns = store.list_connections()?;
    let routes = store.list_routes()?;
    let keys = store.list_api_keys()?;
    println!("  connections: {}", conns.len());
    println!("  routes     : {}", routes.len());
    println!("  api_keys   : {}", keys.len());
    println!("ok");
    Ok(())
}
