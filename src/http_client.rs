//! Shared reqwest client: timeouts + env proxy (HTTP_PROXY / HTTPS_PROXY / ALL_PROXY).

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, Proxy};
use tracing::info;

pub fn build_http_client() -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(32)
        .danger_accept_invalid_certs(false);

    // Prefer HTTPS_PROXY, then HTTP_PROXY, then ALL_PROXY (common corporate setup).
    let proxy_url = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .or_else(|_| std::env::var("all_proxy"))
        .ok();

    if let Some(url) = proxy_url {
        info!(%url, "using upstream HTTP proxy from env");
        let proxy = Proxy::all(&url).with_context(|| format!("invalid proxy URL {url}"))?;
        builder = builder.proxy(proxy);
    }

    // NO_PROXY is handled by reqwest when using system proxy on some builds;
    // with explicit Proxy::all, no_proxy list can be set if present.
    if let Ok(no) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) {
        // reqwest Proxy::all doesn't chain no_proxy easily without system proxy;
        // document: set proxy that honors no_proxy, or leave unset for local upstreams.
        let _ = no;
    }

    builder.build().context("build reqwest client")
}
