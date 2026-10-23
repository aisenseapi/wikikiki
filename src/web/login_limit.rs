//! Bounded, process-local protection for the password endpoint.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub(super) struct LoginLimit(Arc<Mutex<HashMap<String, (Instant, u32)>>>);

impl LoginLimit {
    pub(super) fn allow(&self, peer: &str, handle: &str) -> bool {
        self.allow_at(peer, handle, Instant::now())
    }

    fn allow_at(&self, peer: &str, handle: &str, now: Instant) -> bool {
        let Ok(mut entries) = self.0.lock() else {
            return false;
        };
        entries.retain(|_, (started, _)| now.duration_since(*started) < Duration::from_secs(60));
        let keys = [(format!("ip:{peer}"), 20), (format!("account:{handle}"), 5)];
        for (key, limit) in &keys {
            if entries.get(key).is_some_and(|(_, count)| count >= limit)
                || (!entries.contains_key(key) && entries.len() >= 4096)
            {
                return false;
            }
        }
        for (key, _) in keys {
            entries.entry(key).or_insert((now, 0)).1 += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_peer_budgets_expire_independently_of_input_identity() {
        let limiter = LoginLimit::default();
        let now = Instant::now();
        for i in 0..5 {
            assert!(limiter.allow_at(&format!("peer{i}"), "alice", now));
        }
        assert!(!limiter.allow_at("new-peer", "alice", now));
        for i in 0..20 {
            assert!(limiter.allow_at("shared-peer", &format!("user{i}"), now));
        }
        assert!(!limiter.allow_at("shared-peer", "another", now));
        assert!(limiter.allow_at("shared-peer", "alice", now + Duration::from_secs(60)));
    }
}
