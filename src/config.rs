use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub admin_token: Option<String>,
}

impl AppConfig {
    pub fn new(
        host: String,
        port: u16,
        data_dir: Option<PathBuf>,
        admin_token: Option<String>,
    ) -> Result<Self> {
        let data_dir = match data_dir {
            Some(p) => p,
            None => dirs::home_dir()
                .context("cannot resolve home dir")?
                .join(".thinrouter"),
        };
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700));
        }
        Ok(Self {
            host,
            port,
            data_dir,
            admin_token,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("thinrouter.db")
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
