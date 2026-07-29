//! Refresh OAuth / session tokens for personal subscription providers.

use chrono::{Duration, Utc};
use serde::Deserialize;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::store::{ConnectionRow, Store};

#[derive(Debug, Clone)]
pub struct RefreshedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
}

/// True if access token should be refreshed (missing expiry, or within lead window).
pub fn needs_refresh(conn: &ConnectionRow, lead_secs: i64) -> bool {
    let Some(oauth) = &conn.oauth else {
        return false;
    };
    if oauth.refresh_token.as_deref().unwrap_or("").is_empty() {
        return false;
    }
    let Some(exp) = oauth.expires_at.as_deref() else {
        // No expiry recorded — refresh if access looks empty or always for agy after import.
        return oauth.access_token.is_empty()
            || conn.oauth.as_ref().map(|o| o.provider.as_str()) == Some("agy");
    };
    // Support RFC3339 or unix ms / seconds as string numbers
    if let Ok(ms) = exp.parse::<i64>() {
        let exp_ms = if ms > 1_000_000_000_000 { ms } else { ms * 1000 };
        let now = Utc::now().timestamp_millis();
        return exp_ms - now < lead_secs * 1000;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp) {
        let deadline = Utc::now() + Duration::seconds(lead_secs);
        return dt.with_timezone(&Utc) <= deadline;
    }
    // unparsed expiry → try refresh
    true
}

pub async fn refresh_connection(store: &Store, conn: &ConnectionRow) -> AppResult<ConnectionRow> {
    let oauth = conn
        .oauth
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("not an oauth connection".into()))?;
    let refresh = oauth
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("no refresh_token stored".into()))?;

    let provider = oauth.provider.as_str();
    info!(connection = %conn.name, provider, "refreshing oauth token");

    let tokens = match provider {
        "claude" | "anthropic" | "cc" => refresh_claude(refresh).await?,
        "codex" | "cx" | "openai_codex" => refresh_codex(refresh).await?,
        "grok" | "xai" | "grok_cli" => refresh_xai(refresh).await?,
        "agy" | "ag" | "antigravity" => refresh_google_antigravity(refresh).await?,
        "cursor" | "cu" => refresh_cursor(refresh).await?,
        other => {
            return Err(AppError::BadRequest(format!(
                "refresh not implemented for provider '{other}'"
            )));
        }
    };

    store
        .update_oauth_tokens(
            &conn.id,
            &tokens.access_token,
            tokens.refresh_token.as_deref(),
            tokens.expires_at.as_deref(),
        )
        .map_err(AppError::Internal)?;

    store
        .get_connection(&conn.id)
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("connection missing after refresh".into()))
}

/// Ensure connection has a fresh access token when possible.
pub async fn ensure_fresh(store: &Store, conn: ConnectionRow) -> AppResult<ConnectionRow> {
    if conn.auth_type != "oauth_import" {
        return Ok(conn);
    }
    if !needs_refresh(&conn, 300) {
        return Ok(conn);
    }
    match refresh_connection(store, &conn).await {
        Ok(c) => Ok(c),
        Err(e) => {
            warn!(
                connection = %conn.name,
                error = %e,
                "oauth refresh failed; continuing with existing token"
            );
            Ok(conn)
        }
    }
}

async fn refresh_claude(refresh_token: &str) -> AppResult<RefreshedTokens> {
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    }
    let client = crate::http_client::build_http_client()
        .map_err(|e| AppError::Internal(e))?;
    let resp = client
        .post("https://api.anthropic.com/v1/oauth/token")
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        }))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("claude refresh: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 401,
            body: format!("claude refresh failed: {t}"),
        });
    }
    let tok: Tok = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("claude refresh json: {e}")))?;
    Ok(RefreshedTokens {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|s| {
            (Utc::now() + Duration::seconds(s)).to_rfc3339()
        }),
    })
}

async fn refresh_codex(refresh_token: &str) -> AppResult<RefreshedTokens> {
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    }
    let client = crate::http_client::build_http_client()
        .map_err(|e| AppError::Internal(e))?;
    let resp = client
        .post("https://auth.openai.com/oauth/token")
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("codex refresh: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 401,
            body: format!("codex refresh failed: {t}"),
        });
    }
    let tok: Tok = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("codex refresh json: {e}")))?;
    Ok(RefreshedTokens {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|s| {
            (Utc::now() + Duration::seconds(s)).to_rfc3339()
        }),
    })
}

async fn refresh_xai(refresh_token: &str) -> AppResult<RefreshedTokens> {
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    }
    let client = crate::http_client::build_http_client()
        .map_err(|e| AppError::Internal(e))?;
    let resp = client
        .post("https://auth.x.ai/oauth2/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}",
            urlencoding_minimal(refresh_token),
            "b1a00492-073a-47ea-816f-4c329264a828"
        ))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("xai refresh: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 401,
            body: format!("xai/grok refresh failed: {t}"),
        });
    }
    let tok: Tok = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("xai refresh json: {e}")))?;
    Ok(RefreshedTokens {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|s| {
            (Utc::now() + Duration::seconds(s)).to_rfc3339()
        }),
    })
}

async fn refresh_cursor(refresh_token: &str) -> AppResult<RefreshedTokens> {
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        expires_in: Option<i64>,
        #[serde(default)]
        should_logout: Option<bool>,
    }
    let client = crate::http_client::build_http_client().map_err(AppError::Internal)?;
    let resp = client
        .post("https://api2.cursor.sh/oauth/token")
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("cursor refresh: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 401,
            body: format!("cursor refresh failed: {t}"),
        });
    }
    let tok: Tok = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("cursor refresh json: {e}")))?;
    if tok.should_logout == Some(true) {
        return Err(AppError::Upstream {
            status: 401,
            body: "cursor refresh says shouldLogout — re-login in Cursor".into(),
        });
    }
    Ok(RefreshedTokens {
        access_token: tok.access_token,
        // Cursor often does not rotate refresh_token; keep old one.
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|s| (Utc::now() + Duration::seconds(s)).to_rfc3339()),
    })
}

async fn refresh_google_antigravity(refresh_token: &str) -> AppResult<RefreshedTokens> {
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
    }
    // Do not hardcode Google OAuth client secrets in source (GitHub push protection).
    // Set THINROUTER_AGY_CLIENT_ID + THINROUTER_AGY_CLIENT_SECRET (Antigravity IDE public client).
    let client_id = std::env::var("THINROUTER_AGY_CLIENT_ID").map_err(|_| {
        AppError::BadRequest(
            "agy refresh needs THINROUTER_AGY_CLIENT_ID (see docs/PROVIDERS.md)".into(),
        )
    })?;
    let client_secret = std::env::var("THINROUTER_AGY_CLIENT_SECRET").map_err(|_| {
        AppError::BadRequest(
            "agy refresh needs THINROUTER_AGY_CLIENT_SECRET (see docs/PROVIDERS.md)".into(),
        )
    })?;
    let client = crate::http_client::build_http_client()
        .map_err(|e| AppError::Internal(e))?;
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
        urlencoding_minimal(refresh_token),
        urlencoding_minimal(&client_id),
        urlencoding_minimal(&client_secret),
    );
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("google refresh: {e}")))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(AppError::Upstream {
            status: 401,
            body: format!("antigravity/google refresh failed: {t}"),
        });
    }
    let tok: Tok = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("google refresh json: {e}")))?;
    Ok(RefreshedTokens {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: tok.expires_in.map(|s| {
            (Utc::now() + Duration::seconds(s)).to_rfc3339()
        }),
    })
}

fn urlencoding_minimal(s: &str) -> String {
    // percent-encode reserved
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
