use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::secrets::{decrypt_secret, encrypt_secret};

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    secrets_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub default_model: Option<String>,
    pub priority: i64,
    pub enabled: bool,
    /// USD per 1M input tokens (optional, for cost estimate).
    pub input_price_per_m: Option<f64>,
    /// USD per 1M output tokens.
    pub output_price_per_m: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionPublic {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub default_model: Option<String>,
    pub priority: i64,
    pub enabled: bool,
    pub input_price_per_m: Option<f64>,
    pub output_price_per_m: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ConnectionRow> for ConnectionPublic {
    fn from(c: ConnectionRow) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api_key_masked: mask_secret(&c.api_key),
            default_model: c.default_model,
            priority: c.priority,
            enabled: c.enabled,
            input_price_per_m: c.input_price_per_m,
            output_price_per_m: c.output_price_per_m,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRow {
    pub id: String,
    pub public_model: String,
    pub strategy: String,
    pub created_at: String,
    pub targets: Vec<RouteTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTarget {
    pub connection_id: String,
    pub connection_name: Option<String>,
    pub model_override: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: i64,
    pub ts: String,
    pub public_model: Option<String>,
    pub connection_id: Option<String>,
    pub status: Option<i64>,
    pub latency_ms: Option<i64>,
    pub error: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub connection: ConnectionRow,
    pub upstream_model: String,
}

impl Store {
    pub fn open(path: &Path, secrets_key: Option<String>) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite {}", path.display()))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            ",
        )?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            secrets_key,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS connections (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                base_url TEXT NOT NULL,
                api_key TEXT NOT NULL,
                default_model TEXT,
                priority INTEGER NOT NULL DEFAULT 100,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                public_model TEXT NOT NULL UNIQUE,
                strategy TEXT NOT NULL DEFAULT 'fallback',
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS route_targets (
                route_id TEXT NOT NULL,
                connection_id TEXT NOT NULL,
                model_override TEXT,
                position INTEGER NOT NULL,
                PRIMARY KEY (route_id, position),
                FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE,
                FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL UNIQUE,
                key_prefix TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS usage_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                public_model TEXT,
                connection_id TEXT,
                status INTEGER,
                latency_ms INTEGER,
                error TEXT
            );
            "#,
        )?;
        // Additive columns for older DBs
        let _ = conn.execute(
            "ALTER TABLE connections ADD COLUMN input_price_per_m REAL",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE connections ADD COLUMN output_price_per_m REAL",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE usage_events ADD COLUMN prompt_tokens INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE usage_events ADD COLUMN completion_tokens INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE usage_events ADD COLUMN estimated_cost_usd REAL",
            [],
        );
        Ok(())
    }

    fn dec_key(&self, stored: &str) -> String {
        decrypt_secret(stored, self.secrets_key.as_deref())
    }

    fn enc_key(&self, plain: &str) -> String {
        encrypt_secret(plain, self.secrets_key.as_deref())
    }

    #[allow(clippy::too_many_arguments)]
    fn map_connection_row(
        &self,
        id: String,
        name: String,
        base_url: String,
        api_key: String,
        default_model: Option<String>,
        priority: i64,
        enabled: bool,
        input_price_per_m: Option<f64>,
        output_price_per_m: Option<f64>,
        created_at: String,
        updated_at: String,
    ) -> ConnectionRow {
        ConnectionRow {
            id,
            name,
            base_url,
            api_key: self.dec_key(&api_key),
            default_model,
            priority,
            enabled,
            input_price_per_m,
            output_price_per_m,
            created_at,
            updated_at,
        }
    }

    pub fn list_connections(&self) -> Result<Vec<ConnectionRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, base_url, api_key, default_model, priority, enabled,
                    input_price_per_m, output_price_per_m, created_at, updated_at
             FROM connections ORDER BY priority ASC, name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)? != 0,
                row.get::<_, Option<f64>>(7)?,
                row.get::<_, Option<f64>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua) = r?;
            out.push(self.map_connection_row(
                id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua,
            ));
        }
        Ok(out)
    }

    pub fn get_connection(&self, id: &str) -> Result<Option<ConnectionRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, base_url, api_key, default_model, priority, enabled,
                        input_price_per_m, output_price_per_m, created_at, updated_at
                 FROM connections WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)? != 0,
                        row.get::<_, Option<f64>>(7)?,
                        row.get::<_, Option<f64>>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua)| {
            self.map_connection_row(id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua)
        }))
    }

    pub fn get_connection_by_name(&self, name: &str) -> Result<Option<ConnectionRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, base_url, api_key, default_model, priority, enabled,
                        input_price_per_m, output_price_per_m, created_at, updated_at
                 FROM connections WHERE name = ?1",
                params![name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)? != 0,
                        row.get::<_, Option<f64>>(7)?,
                        row.get::<_, Option<f64>>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua)| {
            self.map_connection_row(id, name, base_url, api_key, dm, pr, en, ip, op, ca, ua)
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_connection(
        &self,
        id: Option<String>,
        name: &str,
        base_url: &str,
        api_key: &str,
        default_model: Option<&str>,
        priority: i64,
        enabled: bool,
        input_price_per_m: Option<f64>,
        output_price_per_m: Option<f64>,
    ) -> Result<ConnectionRow> {
        let now = Utc::now().to_rfc3339();
        let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let stored_key = if api_key.is_empty() {
            String::new()
        } else {
            self.enc_key(api_key)
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO connections (id, name, base_url, api_key, default_model, priority, enabled,
                                      input_price_per_m, output_price_per_m, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               base_url = excluded.base_url,
               api_key = CASE WHEN excluded.api_key = '' THEN connections.api_key ELSE excluded.api_key END,
               default_model = excluded.default_model,
               priority = excluded.priority,
               enabled = excluded.enabled,
               input_price_per_m = excluded.input_price_per_m,
               output_price_per_m = excluded.output_price_per_m,
               updated_at = excluded.updated_at",
            params![
                id,
                name,
                base_url.trim_end_matches('/'),
                stored_key,
                default_model,
                priority,
                enabled as i64,
                input_price_per_m,
                output_price_per_m,
                now
            ],
        )?;
        drop(conn);
        self.get_connection(&id)?
            .context("connection missing after upsert")
    }

    pub fn delete_connection(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM connections WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn list_routes(&self) -> Result<Vec<RouteRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, public_model, strategy, created_at FROM routes ORDER BY public_model",
        )?;
        let route_iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut routes = Vec::new();
        for r in route_iter {
            let (id, public_model, strategy, created_at) = r?;
            let targets = Self::load_targets_locked(&conn, &id)?;
            routes.push(RouteRow {
                id,
                public_model,
                strategy,
                created_at,
                targets,
            });
        }
        Ok(routes)
    }

    fn load_targets_locked(conn: &Connection, route_id: &str) -> Result<Vec<RouteTarget>> {
        let mut stmt = conn.prepare(
            "SELECT rt.connection_id, c.name, rt.model_override, rt.position
             FROM route_targets rt
             LEFT JOIN connections c ON c.id = rt.connection_id
             WHERE rt.route_id = ?1
             ORDER BY rt.position ASC",
        )?;
        let rows = stmt.query_map(params![route_id], |row| {
            Ok(RouteTarget {
                connection_id: row.get(0)?,
                connection_name: row.get(1)?,
                model_override: row.get(2)?,
                position: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_route(&self, id: &str) -> Result<Option<RouteRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, public_model, strategy, created_at FROM routes WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, public_model, strategy, created_at)) = row else {
            return Ok(None);
        };
        let targets = Self::load_targets_locked(&conn, &id)?;
        Ok(Some(RouteRow {
            id,
            public_model,
            strategy,
            created_at,
            targets,
        }))
    }

    pub fn get_route_by_public_model(&self, public_model: &str) -> Result<Option<RouteRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, public_model, strategy, created_at FROM routes WHERE public_model = ?1",
                params![public_model],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, public_model, strategy, created_at)) = row else {
            return Ok(None);
        };
        let targets = Self::load_targets_locked(&conn, &id)?;
        Ok(Some(RouteRow {
            id,
            public_model,
            strategy,
            created_at,
            targets,
        }))
    }

    pub fn upsert_route(
        &self,
        id: Option<String>,
        public_model: &str,
        strategy: &str,
        targets: &[(String, Option<String>)],
    ) -> Result<RouteRow> {
        let now = Utc::now().to_rfc3339();
        // Prefer existing id for public_model when creating without id
        let id = if let Some(id) = id {
            id
        } else if let Some(existing) = self.get_route_by_public_model(public_model)? {
            existing.id
        } else {
            Uuid::new_v4().to_string()
        };
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO routes (id, public_model, strategy, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               public_model = excluded.public_model,
               strategy = excluded.strategy",
            params![id, public_model, strategy, now],
        )?;
        tx.execute("DELETE FROM route_targets WHERE route_id = ?1", params![id])?;
        for (pos, (connection_id, model_override)) in targets.iter().enumerate() {
            tx.execute(
                "INSERT INTO route_targets (route_id, connection_id, model_override, position)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, connection_id, model_override, pos as i64],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_route(&id)?.context("route missing after upsert")
    }

    pub fn delete_route(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM routes WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn create_api_key(&self, name: &str) -> Result<(ApiKeyRow, String)> {
        let raw = format!("tr-{}", Uuid::new_v4());
        let hash = hash_key(&raw);
        let prefix = raw.chars().take(10).collect::<String>();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_keys (id, name, key_hash, key_prefix, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![id, name, hash, prefix, now],
        )?;
        Ok((
            ApiKeyRow {
                id,
                name: name.to_string(),
                key_prefix: prefix,
                enabled: true,
                created_at: now,
            },
            raw,
        ))
    }

    pub fn list_api_keys(&self) -> Result<Vec<ApiKeyRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, key_prefix, enabled, created_at FROM api_keys ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ApiKeyRow {
                id: row.get(0)?,
                name: row.get(1)?,
                key_prefix: row.get(2)?,
                enabled: row.get::<_, i64>(3)? != 0,
                created_at: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn set_api_key_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE api_keys SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id],
        )?;
        Ok(n > 0)
    }

    pub fn delete_api_key(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM api_keys WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Returns true if key is valid. Empty key store → allow (bootstrap mode).
    pub fn verify_api_key(&self, raw: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM api_keys WHERE enabled = 1", [], |r| {
                r.get(0)
            })?;
        if count == 0 {
            return Ok(true);
        }
        let hash = hash_key(raw);
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM api_keys WHERE key_hash = ?1 AND enabled = 1",
                params![hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_usage(
        &self,
        public_model: Option<&str>,
        connection_id: Option<&str>,
        status: Option<i64>,
        latency_ms: Option<i64>,
        error: Option<&str>,
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
        estimated_cost_usd: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage_events (ts, public_model, connection_id, status, latency_ms, error,
                                       prompt_tokens, completion_tokens, estimated_cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                Utc::now().to_rfc3339(),
                public_model,
                connection_id,
                status,
                latency_ms,
                error,
                prompt_tokens,
                completion_tokens,
                estimated_cost_usd,
            ],
        )?;
        Ok(())
    }

    pub fn recent_usage(&self, limit: i64) -> Result<Vec<UsageEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ts, public_model, connection_id, status, latency_ms, error,
                    prompt_tokens, completion_tokens, estimated_cost_usd
             FROM usage_events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(UsageEvent {
                id: row.get(0)?,
                ts: row.get(1)?,
                public_model: row.get(2)?,
                connection_id: row.get(3)?,
                status: row.get(4)?,
                latency_ms: row.get(5)?,
                error: row.get(6)?,
                prompt_tokens: row.get(7)?,
                completion_tokens: row.get(8)?,
                estimated_cost_usd: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn usage_cost_total(&self) -> Result<f64> {
        let conn = self.conn.lock().unwrap();
        let total: Option<f64> = conn.query_row(
            "SELECT SUM(estimated_cost_usd) FROM usage_events",
            [],
            |r| r.get(0),
        )?;
        Ok(total.unwrap_or(0.0))
    }

    pub fn stats(&self) -> Result<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let connections: i64 =
            conn.query_row("SELECT COUNT(*) FROM connections", [], |r| r.get(0))?;
        let routes: i64 = conn.query_row("SELECT COUNT(*) FROM routes", [], |r| r.get(0))?;
        let keys: i64 = conn.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))?;
        let usage: i64 = conn.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))?;
        let cost: Option<f64> = conn.query_row(
            "SELECT SUM(estimated_cost_usd) FROM usage_events",
            [],
            |r| r.get(0),
        )?;
        Ok(serde_json::json!({
            "connections": connections,
            "routes": routes,
            "api_keys": keys,
            "usage_events": usage,
            "estimated_cost_usd": cost.unwrap_or(0.0),
        }))
    }
}

pub fn hash_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn mask_secret(s: &str) -> String {
    if s.len() <= 8 {
        return "****".into();
    }
    format!("{}…{}", &s[..4], &s[s.len() - 4..])
}
