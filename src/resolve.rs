use crate::error::{AppError, AppResult};
use crate::store::{ResolvedTarget, Store};

#[derive(Debug, Clone)]
pub struct ResolveResult {
    pub targets: Vec<ResolvedTarget>,
    pub strategy: String,
}

/// Resolve client model string into ordered upstream targets.
///
/// Rules:
/// 1. Exact match on routes.public_model → route targets (fallback order)
/// 2. `connection_name/model` or `connection_id/model` → single target
/// 3. bare name matching a connection with default_model → that connection
pub fn resolve_targets(store: &Store, model: &str) -> AppResult<ResolveResult> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AppError::BadRequest("model is required".into()));
    }

    if let Some(route) = store.get_route_by_public_model(model)? {
        let strategy = route.strategy.clone();
        let mut out = Vec::new();
        for t in route.targets {
            let Some(conn) = store.get_connection(&t.connection_id)? else {
                continue;
            };
            if !conn.enabled {
                continue;
            }
            let upstream_model = t
                .model_override
                .or_else(|| conn.default_model.clone())
                .ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "route target {} has no model_override and connection has no default_model",
                        conn.name
                    ))
                })?;
            out.push(ResolvedTarget {
                connection: conn,
                upstream_model,
            });
        }
        if out.is_empty() {
            return Err(AppError::NoUpstream(format!(
                "route '{model}' has no enabled targets"
            )));
        }
        return Ok(ResolveResult {
            targets: out,
            strategy,
        });
    }

    if let Some((left, right)) = model.split_once('/') {
        let conn = store
            .get_connection_by_name(left)?
            .or(store.get_connection(left)?)
            .ok_or_else(|| AppError::NotFound(format!("connection '{left}' not found")))?;
        if !conn.enabled {
            return Err(AppError::NoUpstream(format!(
                "connection '{}' is disabled",
                conn.name
            )));
        }
        return Ok(ResolveResult {
            targets: vec![ResolvedTarget {
                connection: conn,
                upstream_model: right.to_string(),
            }],
            strategy: "fallback".into(),
        });
    }

    // bare connection name with default model
    if let Some(conn) = store.get_connection_by_name(model)? {
        if !conn.enabled {
            return Err(AppError::NoUpstream(format!(
                "connection '{}' is disabled",
                conn.name
            )));
        }
        let upstream_model = conn.default_model.clone().ok_or_else(|| {
            AppError::BadRequest(format!(
                "connection '{}' has no default_model; use name/model",
                conn.name
            ))
        })?;
        return Ok(ResolveResult {
            targets: vec![ResolvedTarget {
                connection: conn,
                upstream_model,
            }],
            strategy: "fallback".into(),
        });
    }

    Err(AppError::NotFound(format!(
        "model '{model}' not found (define a route or use connection/model)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, Store) {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db"), None).unwrap();
        let c1 = store
            .upsert_connection(
                None,
                "openai",
                "https://api.openai.com/v1",
                "sk-test",
                Some("gpt-4o-mini"),
                10,
                true,
                None,
                None,
            )
            .unwrap();
        let c2 = store
            .upsert_connection(
                None,
                "deepseek",
                "https://api.deepseek.com",
                "sk-ds",
                Some("deepseek-chat"),
                20,
                true,
                None,
                None,
            )
            .unwrap();
        store
            .upsert_route(
                None,
                "cheap",
                "fallback",
                &[
                    (c1.id.clone(), Some("gpt-4o-mini".into())),
                    (c2.id, None),
                ],
            )
            .unwrap();
        (dir, store)
    }

    #[test]
    fn resolve_route_fallback_chain() {
        let (_dir, store) = setup();
        let r = resolve_targets(&store, "cheap").unwrap();
        assert_eq!(r.targets.len(), 2);
        assert_eq!(r.targets[0].upstream_model, "gpt-4o-mini");
        assert_eq!(r.targets[1].upstream_model, "deepseek-chat");
        assert_eq!(r.strategy, "fallback");
    }

    #[test]
    fn resolve_connection_slash_model() {
        let (_dir, store) = setup();
        let r = resolve_targets(&store, "openai/gpt-4o").unwrap();
        assert_eq!(r.targets.len(), 1);
        assert_eq!(r.targets[0].upstream_model, "gpt-4o");
    }
}
