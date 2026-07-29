mod admin;
mod agy;
mod cancel;
mod claude;
mod config;
mod cooldown;
mod error;
mod http_client;
mod local_import;
mod oauth_refresh;
mod providers;
mod proxy;
mod resolve;
mod responses;
mod secrets;
mod server;
mod store;
mod templates;
mod token_saver;
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
        #[arg(long, env = "THINROUTER_HOST", default_value = "127.0.0.1")]
        host: String,
        #[arg(long, env = "THINROUTER_PORT", default_value_t = 20128)]
        port: u16,
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_ADMIN_TOKEN")]
        admin_token: Option<String>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
        #[arg(long, env = "THINROUTER_SSE_STALL_SECS", default_value_t = 90)]
        sse_stall_secs: u64,
        /// Enable token-saver (truncate tool / huge message content)
        #[arg(long, env = "THINROUTER_TOKEN_SAVER", default_value_t = false)]
        token_saver: bool,
        #[arg(long, env = "THINROUTER_TOKEN_SAVER_MAX_CHARS", default_value_t = 2000)]
        token_saver_max_chars: usize,
    },
    Doctor {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
    },
    Tui {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
    },
    /// Import tokens from local agent CLIs (codex/claude/grok/cursor/opencode/agy)
    ImportLocal {
        #[arg(long, env = "THINROUTER_DATA_DIR")]
        data_dir: Option<std::path::PathBuf>,
        #[arg(long, env = "THINROUTER_SECRETS_KEY")]
        secrets_key: Option<String>,
        /// Also create routes rt-<provider> for each import
        #[arg(long, default_value_t = true)]
        routes: bool,
        /// After import, try OAuth refresh for each connection
        #[arg(long, default_value_t = true)]
        refresh: bool,
    },
    /// Refresh OAuth tokens for imported connections
    Refresh {
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
            token_saver,
            token_saver_max_chars,
        } => {
            let cfg = AppConfig::new(
                host,
                port,
                data_dir,
                admin_token,
                secrets_key,
                Some(sse_stall_secs),
                Some(token_saver),
                Some(token_saver_max_chars),
            )?;
            server::run(cfg).await
        }
        Commands::Doctor {
            data_dir,
            secrets_key,
        } => {
            let cfg = AppConfig::new(
                "127.0.0.1".into(),
                20128,
                data_dir,
                None,
                secrets_key,
                None,
                None,
                None,
            )?;
            doctor(&cfg)
        }
        Commands::Tui {
            data_dir,
            secrets_key,
        } => {
            let cfg = AppConfig::new(
                "127.0.0.1".into(),
                20128,
                data_dir,
                None,
                secrets_key,
                None,
                None,
                None,
            )?;
            tui_app::run_tui(&cfg)
        }
        Commands::ImportLocal {
            data_dir,
            secrets_key,
            routes,
            refresh,
        } => {
            let cfg = AppConfig::new(
                "127.0.0.1".into(),
                20128,
                data_dir,
                None,
                secrets_key,
                None,
                None,
                None,
            )?;
            import_local_cmd(&cfg, routes, refresh).await
        }
        Commands::Refresh {
            data_dir,
            secrets_key,
        } => {
            let cfg = AppConfig::new(
                "127.0.0.1".into(),
                20128,
                data_dir,
                None,
                secrets_key,
                None,
                None,
                None,
            )?;
            refresh_cmd(&cfg).await
        }
    }
}

async fn import_local_cmd(cfg: &AppConfig, routes: bool, refresh: bool) -> Result<()> {
    let store = store::Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;
    println!("thinrouter import-local");
    println!("  data_dir: {}", cfg.data_dir.display());
    let results = local_import::import_all_local(&store, routes)?;
    for r in &results {
        println!(
            "  {:8} {:14} {} — {}",
            r.provider,
            r.name,
            if r.ok { "OK  " } else { "FAIL" },
            r.detail
        );
    }
    if refresh {
        println!("  refreshing oauth…");
        for (name, ok, detail) in local_import::refresh_all_oauth(&store).await? {
            println!(
                "  refresh {:14} {} — {}",
                name,
                if ok { "OK  " } else { "FAIL" },
                detail.chars().take(120).collect::<String>()
            );
        }
    }
    println!("ok");
    Ok(())
}

async fn refresh_cmd(cfg: &AppConfig) -> Result<()> {
    let store = store::Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;
    println!("thinrouter refresh");
    for (name, ok, detail) in local_import::refresh_all_oauth(&store).await? {
        println!(
            "  {:14} {} — {}",
            name,
            if ok { "OK  " } else { "FAIL" },
            detail.chars().take(160).collect::<String>()
        );
    }
    println!("ok");
    Ok(())
}

fn doctor(cfg: &AppConfig) -> Result<()> {
    println!("thinrouter doctor");
    println!("  crate    : thinrouter");
    println!("  repo     : fallrising/pokercase");
    println!("  data_dir : {}", cfg.data_dir.display());
    println!("  db_path  : {}", cfg.db_path().display());
    println!(
        "  secrets  : {}",
        if cfg.secrets_key.is_some() {
            "encryption enabled"
        } else {
            "plaintext secrets"
        }
    );
    let store = store::Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;
    let conns = store.list_connections()?;
    let oauth_n = conns.iter().filter(|c| c.auth_type == "oauth_import").count();
    let routes = store.list_routes()?;
    let keys = store.list_api_keys()?;
    let cost = store.usage_cost_total()?;
    println!("  connections: {} (oauth_import: {oauth_n})", conns.len());
    println!("  routes     : {}", routes.len());
    println!("  api_keys   : {}", keys.len());
    println!("  est. cost  : ${cost:.6}");
    println!("ok");
    Ok(())
}
