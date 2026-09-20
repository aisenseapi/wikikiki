//! Coalescing for liveness bookkeeping (`actors.last_seen_at`,
//! `credentials.last_used_at`).
//!
//! Both columns answer "roughly when did we last hear from this?", and both
//! were being written on *every* request. SQLite has one writer at a time
//! (DESIGN.md §11), so that turned every read — an anonymous page view, a
//! search, an agent polling — into a write-lock acquisition, competing with
//! the actual content writes the system exists to serve.
//!
//! The observation is that the value's usefulness has a resolution of
//! minutes, not milliseconds. Keeping the last written timestamp in memory
//! and skipping the round-trip inside that window removes the contention
//! without changing what an observer sees.
//!
//! Losing the map on restart is harmless: the next request writes again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Throttle {
    interval: i64,
    last: Arc<Mutex<HashMap<i64, i64>>>,
}

impl Throttle {
    pub fn new(interval_secs: i64) -> Self {
        Self {
            interval: interval_secs.max(0),
            last: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Claim the right to write for `key` at time `now`.
    ///
    /// Returns `true` at most once per interval per key, and records the
    /// claim as it returns — so two concurrent requests for the same key
    /// yield exactly one write. A poisoned lock degrades to "write anyway",
    /// which is the safe direction: correct data, lost optimisation.
    pub fn claim(&self, key: i64, now: i64) -> bool {
        let Ok(mut map) = self.last.lock() else {
            return true;
        };
        match map.get(&key) {
            // `now >= prev` matters: a clock stepped backwards (NTP
            // correction, restored snapshot) makes the difference negative,
            // which would otherwise read as "well inside the window" and
            // wedge the key shut until real time caught up again.
            Some(&prev) if now >= prev && now - prev < self.interval => false,
            _ => {
                map.insert(key, now);
                true
            }
        }
    }

    /// Number of keys currently held. Bounded by active actors in an
    /// interval; there is no unbounded growth to sweep at Stage 2 scale.
    pub fn tracked(&self) -> usize {
        self.last.lock().map(|m| m.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_claim_always_wins() {
        let t = Throttle::new(60);
        assert!(t.claim(1, 1_000));
    }

    #[test]
    fn second_claim_inside_window_is_refused() {
        let t = Throttle::new(60);
        assert!(t.claim(1, 1_000));
        assert!(!t.claim(1, 1_030));
        assert!(!t.claim(1, 1_059));
    }

    #[test]
    fn claim_after_window_is_allowed_again() {
        let t = Throttle::new(60);
        assert!(t.claim(1, 1_000));
        assert!(t.claim(1, 1_060));
        assert!(!t.claim(1, 1_061));
    }

    #[test]
    fn keys_are_independent() {
        let t = Throttle::new(60);
        assert!(t.claim(1, 1_000));
        assert!(t.claim(2, 1_000));
        assert_eq!(t.tracked(), 2);
    }

    #[test]
    fn zero_interval_never_throttles() {
        let t = Throttle::new(0);
        assert!(t.claim(1, 1_000));
        assert!(t.claim(1, 1_000));
    }

    /// Clock adjustments must not wedge a key shut forever.
    #[test]
    fn backwards_clock_recovers() {
        let t = Throttle::new(60);
        assert!(t.claim(1, 5_000));
        assert!(t.claim(1, 1_000), "a now earlier than the record re-claims");
    }
}
