//! In-memory connection cooldown after rate limits / retryable failures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Tracks per-connection cool-down windows (skip until Instant).
#[derive(Clone, Default)]
pub struct CooldownMap {
    inner: Arc<Mutex<HashMap<String, Instant>>>,
}

impl CooldownMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cooled(&self, connection_id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if let Some(until) = guard.get(connection_id).copied() {
            if Instant::now() < until {
                return true;
            }
            guard.remove(connection_id);
        }
        false
    }

    /// Mark connection cooled for `secs` (extends if already cooled later).
    pub fn mark(&self, connection_id: &str, secs: u64) {
        let until = Instant::now() + Duration::from_secs(secs);
        let mut guard = self.inner.lock().unwrap();
        guard
            .entry(connection_id.to_string())
            .and_modify(|e| {
                if until > *e {
                    *e = until;
                }
            })
            .or_insert(until);
    }

    pub fn clear(&self, connection_id: &str) {
        self.inner.lock().unwrap().remove(connection_id);
    }
}

/// Default cooldown after HTTP 429.
pub const COOLDOWN_429_SECS: u64 = 60;
/// Shorter cooldown for other retryable upstream errors.
pub const COOLDOWN_RETRYABLE_SECS: u64 = 15;
/// Soft lock after repeated failures (same connection).
pub const COOLDOWN_HARD_SECS: u64 = 120;

pub fn cooldown_secs_for_status(status: u16) -> u64 {
    match status {
        429 => COOLDOWN_429_SECS,
        401..=403 => COOLDOWN_HARD_SECS,
        408 => COOLDOWN_RETRYABLE_SECS,
        s if (500..600).contains(&s) => COOLDOWN_RETRYABLE_SECS,
        _ => 0,
    }
}
