//! Per-session token accounting with an optional hard limit.
//!
//! The budget is the mechanism behind pillar 3 (per-agent hard-stop budgets):
//! the engine charges token usage as it arrives and, once [`exceeded`] becomes
//! true, kills the agent process.
//!
//! [`exceeded`]: Budget::exceeded

/// Tracks tokens spent in a session against an optional cap.
///
/// `Default` is an unlimited budget (no cap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    limit_tokens: Option<u64>,
    spent_tokens: u64,
}

impl Budget {
    /// Create a budget with an optional hard limit. `None` means unlimited.
    pub fn new(limit_tokens: Option<u64>) -> Self {
        Self {
            limit_tokens,
            spent_tokens: 0,
        }
    }

    /// Charge `tokens` to the budget. Saturates rather than overflowing, so the
    /// count can only ever over-state (fail-closed), never wrap to a low value.
    pub fn add(&mut self, tokens: u64) {
        self.spent_tokens = self.spent_tokens.saturating_add(tokens);
    }

    /// Total tokens charged so far.
    #[must_use]
    pub fn spent(&self) -> u64 {
        self.spent_tokens
    }

    /// The configured limit, if any.
    #[must_use]
    pub fn limit(&self) -> Option<u64> {
        self.limit_tokens
    }

    /// True once spend has gone strictly past the limit. An unlimited budget
    /// never exceeds. Spending *exactly* the limit is allowed. Once true it
    /// stays true (spend is monotonic).
    #[must_use]
    pub fn exceeded(&self) -> bool {
        match self.limit_tokens {
            Some(limit) => self.spent_tokens > limit,
            None => false,
        }
    }

    /// Tokens remaining before the limit (saturating at zero), or `None` for an
    /// unlimited budget.
    #[must_use]
    pub fn remaining(&self) -> Option<u64> {
        self.limit_tokens
            .map(|limit| limit.saturating_sub(self.spent_tokens))
    }

    /// Fraction of the budget consumed in `[0.0, 1.0+]`, for a UI gauge. `None`
    /// for an unlimited budget. A zero limit reads as fully consumed (`1.0`)
    /// once anything is spent, avoiding a divide-by-zero.
    #[must_use]
    pub fn fraction_used(&self) -> Option<f64> {
        self.limit_tokens.map(|limit| {
            if limit == 0 {
                if self.spent_tokens == 0 { 0.0 } else { 1.0 }
            } else {
                self.spent_tokens as f64 / limit as f64
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_exceeds() {
        let mut b = Budget::new(None);
        b.add(1_000_000);
        assert!(!b.exceeded());
        assert_eq!(b.remaining(), None);
        assert_eq!(b.limit(), None);
        assert_eq!(b.spent(), 1_000_000);
        assert_eq!(b.fraction_used(), None);
    }

    #[test]
    fn accumulates_and_exceeds_past_limit() {
        let mut b = Budget::new(Some(1000));
        b.add(600);
        assert!(!b.exceeded());
        assert_eq!(b.remaining(), Some(400));
        assert_eq!(b.fraction_used(), Some(0.6));
        b.add(500); // 1100 > 1000
        assert!(b.exceeded());
        assert_eq!(b.remaining(), Some(0));
        assert_eq!(b.spent(), 1100);
        assert_eq!(b.fraction_used(), Some(1.1));
    }

    #[test]
    fn exactly_at_limit_is_not_exceeded() {
        let mut b = Budget::new(Some(1000));
        b.add(1000);
        assert!(!b.exceeded());
        assert_eq!(b.remaining(), Some(0));
        assert_eq!(b.fraction_used(), Some(1.0));
    }

    #[test]
    fn add_saturates_without_overflow() {
        let mut b = Budget::new(Some(u64::MAX));
        b.add(u64::MAX);
        b.add(10);
        assert_eq!(b.spent(), u64::MAX);
        assert!(!b.exceeded());
        assert_eq!(b.remaining(), Some(0));
    }

    #[test]
    fn zero_limit_exceeds_on_any_spend() {
        let mut b = Budget::new(Some(0));
        assert!(!b.exceeded());
        assert_eq!(b.fraction_used(), Some(0.0));
        b.add(1);
        assert!(b.exceeded());
        assert_eq!(b.fraction_used(), Some(1.0));
    }

    #[test]
    fn exceeded_stays_true_after_crossing() {
        let mut b = Budget::new(Some(10));
        b.add(11);
        assert!(b.exceeded());
        b.add(0);
        assert!(b.exceeded());
    }

    #[test]
    fn add_order_is_independent() {
        let mut small_first = Budget::new(Some(100));
        small_first.add(10);
        small_first.add(95);
        let mut large_first = Budget::new(Some(100));
        large_first.add(95);
        large_first.add(10);
        assert_eq!(small_first.spent(), large_first.spent());
        assert_eq!(small_first.exceeded(), large_first.exceeded());
    }

    #[test]
    fn default_is_unlimited() {
        let b = Budget::default();
        assert_eq!(b.limit(), None);
        assert!(!b.exceeded());
        assert_eq!(b.fraction_used(), None);
    }

    #[test]
    fn budget_is_copy_and_eq() {
        let b = Budget::new(Some(5));
        let c = b;
        assert_eq!(b, c);
    }
}
