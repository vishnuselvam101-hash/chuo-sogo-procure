//! Fixed-window rate limiting for unauthenticated endpoints (login, password reset).
//! In-process: correct for the single-binary deployment. Behind a reverse proxy the
//! peer address is the proxy's, so per-IP limits then apply to all clients together
//! until trusted forwarding headers are configured (see docs/security).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Limit {
    pub max: u32,
    pub window: Duration,
}

pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

impl RateLimiter {
    /// Counts one attempt for `key`. Returns false when the limit is exceeded.
    pub fn check(&self, key: &str, limit: &Limit) -> bool {
        self.check_at(key, limit, Instant::now())
    }

    fn check_at(&self, key: &str, limit: &Limit, now: Instant) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock means a panic elsewhere; fail closed.
            return false;
        };
        if buckets.len() > 100_000 {
            buckets.retain(|_, (_, start)| now.duration_since(*start) < limit.window);
        }
        let entry = buckets.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= limit.window {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= limit.max
    }
}

pub const LOGIN_PER_IP: Limit = Limit {
    max: 20,
    window: Duration::from_secs(60),
};
/// Above the lockout threshold (10 failures), so lockout is reached before this limit.
pub const LOGIN_PER_EMAIL: Limit = Limit {
    max: 15,
    window: Duration::from_secs(300),
};
pub const RESET_PER_IP: Limit = Limit {
    max: 10,
    window: Duration::from_secs(600),
};
pub const RESET_PER_EMAIL: Limit = Limit {
    max: 3,
    window: Duration::from_secs(3600),
};
pub const SET_PASSWORD_PER_IP: Limit = Limit {
    max: 20,
    window: Duration::from_secs(600),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_and_resets_after_window() {
        let rl = RateLimiter::default();
        let limit = Limit {
            max: 2,
            window: Duration::from_secs(10),
        };
        let t0 = Instant::now();
        assert!(rl.check_at("k", &limit, t0));
        assert!(rl.check_at("k", &limit, t0));
        assert!(!rl.check_at("k", &limit, t0));
        assert!(rl.check_at("other", &limit, t0));
        assert!(rl.check_at("k", &limit, t0 + Duration::from_secs(11)));
    }
}
