//! Import OAuth/session tokens from local agent CLI installs.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::providers;
use crate::store::Store;

#[derive(Debug)]
pub struct ImportResult {
    pub provider: String,
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Scan known agent credential paths and upsert oauth connections + simple routes.
pub fn import_all_local(store: &Store, make_routes: bool) -> Result<Vec<ImportResult>> {
    let mut out = Vec::new();
    out.push(import_codex(store, make_routes));
    out.push(import_claude(store, make_routes));
    out.push(import_grok(store, make_routes));
    out.push(import_cursor(store, make_routes));
    out.push(import_opencode(store, make_routes));
    out.push(import_agy(store, make_routes));
    Ok(out)
}

fn upsert(
    store: &Store,
    name: &str,
    provider: &str,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
    meta: Option<&str>,
    default_model: Option<&str>,
    make_routes: bool,
) -> ImportResult {
    let profile = providers::resolve(provider);
    let base = profile
        .map(|p| p.default_base_url)
        .unwrap_or("https://api.openai.com/v1");
    let model = default_model.or_else(|| profile.and_then(|p| p.default_model));
    match store.upsert_oauth_connection(
        None,
        name,
        base,
        profile.map(|p| p.id).unwrap_or(provider),
        access,
        refresh,
        expires,
        meta,
        model,
        100,
        true,
    ) {
        Ok(row) => {
            if make_routes {
                let public = format!("rt-{}", profile.map(|p| p.id).unwrap_or(provider));
                let _ = store.upsert_route(
                    None,
                    &public,
                    "fallback",
                    &[(row.id.clone(), None)],
                );
            }
            ImportResult {
                provider: provider.into(),
                name: name.into(),
                ok: true,
                detail: format!("id={} model={}", &row.id[..8.min(row.id.len())], model.unwrap_or("-")),
            }
        }
        Err(e) => ImportResult {
            provider: provider.into(),
            name: name.into(),
            ok: false,
            detail: e.to_string(),
        },
    }
}

fn import_codex(store: &Store, make_routes: bool) -> ImportResult {
    let path = home().join(".codex/auth.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return ImportResult {
                provider: "codex".into(),
                name: "local-codex".into(),
                ok: false,
                detail: format!("missing {}: {e}", path.display()),
            };
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return ImportResult {
                provider: "codex".into(),
                name: "local-codex".into(),
                ok: false,
                detail: format!("parse error: {e}"),
            };
        }
    };
    let access = v
        .pointer("/tokens/access_token")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let refresh = v
        .pointer("/tokens/refresh_token")
        .and_then(|t| t.as_str());
    let account_id = v
        .pointer("/tokens/account_id")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if access.is_empty() {
        return ImportResult {
            provider: "codex".into(),
            name: "local-codex".into(),
            ok: false,
            detail: "no access_token".into(),
        };
    }
    let meta = serde_json::json!({"account_id": account_id, "source": path.display().to_string()});
    // Prefer model from config.toml if present
    let model = read_codex_model().or(Some("gpt-5.6-sol".into()));
    upsert(
        store,
        "local-codex",
        "codex",
        access,
        refresh,
        None,
        Some(&meta.to_string()),
        model.as_deref(),
        make_routes,
    )
}

fn read_codex_model() -> Option<String> {
    let p = home().join(".codex/config.toml");
    let s = std::fs::read_to_string(p).ok()?;
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("model") {
            let rest = rest.trim().trim_start_matches('=').trim();
            let m = rest.trim_matches('"').trim_matches('\'').to_string();
            if !m.is_empty() {
                return Some(m);
            }
        }
    }
    None
}

fn import_claude(store: &Store, make_routes: bool) -> ImportResult {
    let path = home().join(".claude/.credentials.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return ImportResult {
                provider: "claude".into(),
                name: "local-claude".into(),
                ok: false,
                detail: format!("missing {}: {e}", path.display()),
            };
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return ImportResult {
                provider: "claude".into(),
                name: "local-claude".into(),
                ok: false,
                detail: format!("parse: {e}"),
            };
        }
    };
    let oauth = &v["claudeAiOauth"];
    let access = oauth.get("accessToken").and_then(|t| t.as_str()).unwrap_or("");
    let refresh = oauth.get("refreshToken").and_then(|t| t.as_str());
    let exp = oauth
        .get("expiresAt")
        .map(|e| match e {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty());
    if access.is_empty() {
        return ImportResult {
            provider: "claude".into(),
            name: "local-claude".into(),
            ok: false,
            detail: "no accessToken".into(),
        };
    }
    upsert(
        store,
        "local-claude",
        "claude",
        access,
        refresh,
        exp.as_deref(),
        Some(r#"{"source":"~/.claude/.credentials.json"}"#),
        Some("claude-sonnet-4-5-20250929"),
        make_routes,
    )
}

fn import_grok(store: &Store, make_routes: bool) -> ImportResult {
    let path = home().join(".grok/auth.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return ImportResult {
                provider: "grok".into(),
                name: "local-grok".into(),
                ok: false,
                detail: format!("missing {}: {e}", path.display()),
            };
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return ImportResult {
                provider: "grok".into(),
                name: "local-grok".into(),
                ok: false,
                detail: format!("parse: {e}"),
            };
        }
    };
    let entry = v.as_object().and_then(|m| {
        m.values().find(|v| v.get("key").and_then(|k| k.as_str()).is_some())
    });
    let Some(entry) = entry else {
        return ImportResult {
            provider: "grok".into(),
            name: "local-grok".into(),
            ok: false,
            detail: "no key entry".into(),
        };
    };
    let access = entry.get("key").and_then(|k| k.as_str()).unwrap_or("");
    let refresh = entry.get("refresh_token").and_then(|t| t.as_str());
    let exp = entry.get("expires_at").and_then(|t| t.as_str());
    upsert(
        store,
        "local-grok",
        "grok",
        access,
        refresh,
        exp,
        Some(r#"{"source":"~/.grok/auth.json"}"#),
        Some("grok-build"),
        make_routes,
    )
}

fn import_cursor(store: &Store, make_routes: bool) -> ImportResult {
    let path = home().join(".config/cursor/auth.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return ImportResult {
                provider: "cursor".into(),
                name: "local-cursor".into(),
                ok: false,
                detail: format!("missing {}: {e}", path.display()),
            };
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return ImportResult {
                provider: "cursor".into(),
                name: "local-cursor".into(),
                ok: false,
                detail: format!("parse: {e}"),
            };
        }
    };
    let access = v.get("accessToken").and_then(|t| t.as_str()).unwrap_or("");
    let refresh = v.get("refreshToken").and_then(|t| t.as_str());
    if access.is_empty() {
        return ImportResult {
            provider: "cursor".into(),
            name: "local-cursor".into(),
            ok: false,
            detail: "no accessToken".into(),
        };
    }
    upsert(
        store,
        "local-cursor",
        "cursor",
        access,
        refresh,
        None,
        Some(r#"{"source":"~/.config/cursor/auth.json"}"#),
        Some("default"),
        make_routes,
    )
}

fn import_opencode(store: &Store, make_routes: bool) -> ImportResult {
    upsert(
        store,
        "local-opencode",
        "opencode",
        "",
        None,
        None,
        Some(r#"{"source":"builtin-free"}"#),
        Some("big-pickle"),
        make_routes,
    )
}

fn import_agy(store: &Store, make_routes: bool) -> ImportResult {
    let path = home().join(".gemini/antigravity-cli/antigravity-oauth-token");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return ImportResult {
                provider: "agy".into(),
                name: "local-agy".into(),
                ok: false,
                detail: format!("missing {}: {e}", path.display()),
            };
        }
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return ImportResult {
                provider: "agy".into(),
                name: "local-agy".into(),
                ok: false,
                detail: format!("parse: {e}"),
            };
        }
    };
    let tok = &v["token"];
    let access = tok.get("access_token").and_then(|t| t.as_str()).unwrap_or("");
    let refresh = tok.get("refresh_token").and_then(|t| t.as_str());
    let exp = tok.get("expiry").and_then(|t| t.as_str());
    if access.is_empty() && refresh.unwrap_or("").is_empty() {
        return ImportResult {
            provider: "agy".into(),
            name: "local-agy".into(),
            ok: false,
            detail: "no tokens".into(),
        };
    }
    upsert(
        store,
        "local-agy",
        "agy",
        access,
        refresh,
        exp,
        Some(r#"{"source":"~/.gemini/antigravity-cli/antigravity-oauth-token"}"#),
        Some("gemini-3-flash"),
        make_routes,
    )
}

/// Also try refreshing all oauth connections that need it.
pub async fn refresh_all_oauth(store: &Store) -> Result<Vec<(String, bool, String)>> {
    use crate::oauth_refresh;
    let mut out = Vec::new();
    for c in store.list_connections().context("list")? {
        if c.auth_type != "oauth_import" {
            continue;
        }
        if c.oauth.as_ref().and_then(|o| o.refresh_token.as_ref()).is_none() {
            continue;
        }
        match oauth_refresh::refresh_connection(store, &c).await {
            Ok(_) => out.push((c.name, true, "refreshed".into())),
            Err(e) => out.push((c.name, false, e.to_string())),
        }
    }
    Ok(out)
}
