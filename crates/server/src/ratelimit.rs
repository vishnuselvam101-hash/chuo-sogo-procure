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
    buckets: Mutex<HashMap<String, (u32, Instant, Duration)>>,
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
            // Each bucket expires by its own window (AI buckets last an hour, login a minute).
            buckets.retain(|_, (_, start, window)| now.duration_since(*start) < *window);
        }
        let entry = buckets
            .entry(key.to_string())
            .or_insert((0, now, limit.window));
        if now.duration_since(entry.1) >= limit.window {
            *entry = (0, now, limit.window);
        }
        entry.0 += 1;
        entry.0 <= limit.max
    }
}

/// The rate-limit key for a client address: IPv6 clients are counted per /64 (one customer
/// network usually owns a whole /64, so per-address limits alone are easy to sidestep).
pub fn ip_key(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => ip.to_string(),
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
/// Second-factor attempts per account (on top of the shared lockout).
pub const MFA_PER_USER: Limit = Limit {
    max: 5,
    window: Duration::from_secs(300),
};
/// AI calls cost compute (or money with a hosted provider): per user.
pub const AI_PER_USER: Limit = Limit {
    max: 30,
    window: Duration::from_secs(3600),
};
pub const SET_PASSWORD_PER_IP: Limit = Limit {
    max: 20,
    window: Duration::from_secs(600),
};
/// Consumer sign-ups over all addresses (each one creates an account and sends mail).
pub const SIGNUP_GLOBAL: Limit = Limit {
    max: 300,
    window: Duration::from_secs(3600),
};
/// Consumer sign-ups from one address.
pub const SIGNUP_PER_IP: Limit = Limit {
    max: 10,
    window: Duration::from_secs(3600),
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

    #[test]
    fn ipv6_clients_share_their_64() {
        assert_eq!(
            ip_key("2001:db8:1:2:aaaa::1"),
            ip_key("2001:db8:1:2:ffff::9")
        );
        assert_ne!(ip_key("2001:db8:1:2::1"), ip_key("2001:db8:1:3::1"));
        assert_eq!(ip_key("203.0.113.7"), "203.0.113.7");
        assert_eq!(ip_key("unknown"), "unknown");
    }
}
