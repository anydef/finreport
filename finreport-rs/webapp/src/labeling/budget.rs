//! The webapp process's LLM allowance: one [`CostGuard`] shared by every
//! request, refilled on a time window. Lives in `labeling`, not `graphql`,
//! because `graphql` depends on `labeling` and not the reverse (same reasoning
//! as `cache_publish`).
//!
//! `APP_llm_max_requests_per_run` caps uncached LLM calls. The labeler refills
//! its own guards per sweep; the webapp has no sweep, so it refills on
//! `APP_llm_budget_window_secs` instead. The labeler is a separate process and
//! keeps its own, separate budget - the two do not share an allowance.
//!
//! Semantics:
//! - One instance per process (built once in `create_schema`), shared by every
//!   request via `Arc`. Building a guard per call would bound one call, not the
//!   process.
//! - [`SharedLlmBudget::guard`] resets the guard and starts a new window only
//!   when `window` has elapsed since the current window began; otherwise
//!   callers keep drawing down the same remaining allowance.
//! - `window == Duration::ZERO` means **the budget never refills for the
//!   process lifetime**. That is the deliberate reading of `0`.
//! - `max_requests == 0` is [`CostGuard`]'s "unlimited"; the window is then
//!   irrelevant.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::labeling::processor::CostGuard;

/// A process-wide [`CostGuard`] plus the start of its current budget window.
pub struct SharedLlmBudget {
    guard: CostGuard,
    window: Duration,
    window_start: Mutex<Instant>,
}

impl SharedLlmBudget {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self::starting_at(max_requests, window, Instant::now())
    }

    fn starting_at(max_requests: u32, window: Duration, start: Instant) -> Self {
        Self { guard: CostGuard::new(max_requests), window, window_start: Mutex::new(start) }
    }

    /// The process-wide guard, refilled first if the current window elapsed.
    pub fn guard(&self) -> &CostGuard {
        self.guard_at(Instant::now())
    }

    fn guard_at(&self, now: Instant) -> &CostGuard {
        // The lock is held across the decision and the reset, so two
        // concurrent callers cannot both refill the same window.
        let mut start = self.window_start.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.window.is_zero() && now.saturating_duration_since(*start) >= self.window {
            self.guard.reset();
            *start = now;
        }
        &self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(60);

    #[test]
    fn is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedLlmBudget>();
    }

    #[test]
    fn calls_inside_one_window_share_one_allowance() {
        let t0 = Instant::now();
        let budget = SharedLlmBudget::starting_at(2, WINDOW, t0);
        assert!(budget.guard_at(t0).try_consume());
        assert!(budget.guard_at(t0 + Duration::from_secs(10)).try_consume());
        assert!(!budget.guard_at(t0 + Duration::from_secs(59)).try_consume());
    }

    #[test]
    fn an_elapsed_window_refills() {
        let t0 = Instant::now();
        let budget = SharedLlmBudget::starting_at(1, WINDOW, t0);
        assert!(budget.guard_at(t0).try_consume());
        assert!(!budget.guard_at(t0 + Duration::from_secs(30)).try_consume());
        assert!(budget.guard_at(t0 + WINDOW).try_consume());
        // The new window started at the refill, not at t0.
        assert!(!budget.guard_at(t0 + WINDOW + Duration::from_secs(30)).try_consume());
        assert!(budget.guard_at(t0 + WINDOW * 2).try_consume());
    }

    #[test]
    fn a_zero_window_never_refills() {
        let t0 = Instant::now();
        let budget = SharedLlmBudget::starting_at(1, Duration::ZERO, t0);
        assert!(budget.guard_at(t0).try_consume());
        assert!(!budget.guard_at(t0 + Duration::from_secs(1_000_000)).try_consume());
    }

    #[test]
    fn zero_max_requests_stays_unlimited() {
        let t0 = Instant::now();
        for window in [Duration::ZERO, WINDOW] {
            let budget = SharedLlmBudget::starting_at(0, window, t0);
            for i in 0..100 {
                assert!(budget.guard_at(t0 + Duration::from_secs(i * 30)).try_consume());
            }
        }
    }

    #[test]
    fn public_constructor_and_guard_work_without_sleeping() {
        let budget = SharedLlmBudget::new(1, WINDOW);
        assert!(budget.guard().try_consume());
        assert!(!budget.guard().try_consume());
    }
}
