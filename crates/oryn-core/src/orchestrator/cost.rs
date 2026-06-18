//! USD cost accounting with prompt-cache economics.
//!
//! Pricing classes are billed per **million** tokens (see [`Pricing`]). The
//! [`TokenUsage`] fields are reported separately and never double-count: `input`
//! is non-cached prompt tokens, `cache_read`/`cache_write` are the cached prompt
//! tokens billed at their own rates, and `output` is the completion.
//!
//! Two figures drive Oryn's "cache-stable prefix" value story:
//!
//! - [`cost_usd`] — what the request actually costs given the cache split.
//! - [`baseline_usd`] — what the *same* request would have cost with no cache, i.e.
//!   every prompt token (cached or not) billed at the full input rate.
//!
//! Their difference, [`cache_savings_usd`], is the dollar value the cache-stable
//! prefix discipline buys, and is always ≥ 0 because cache rates never exceed the
//! input rate in any sane price sheet (and the functions clamp to 0 regardless).

use crate::event::TokenUsage;
use crate::orchestrator::provider::Pricing;

/// Tokens are priced per this many tokens.
const PER_MILLION: f64 = 1_000_000.0;

// ── cost_usd ──────────────────────────────────────────────────────────────────

/// Actual USD cost of `usage` under `pricing`, honouring the cache split.
///
/// Each token class is billed at its own per-million rate:
/// `input·input_rate + output·output_rate + cache_read·cache_read_rate +
/// cache_write·cache_write_rate`, divided by one million.
pub fn cost_usd(usage: &TokenUsage, pricing: &Pricing) -> f64 {
    let micro = usage.input as f64 * pricing.input
        + usage.output as f64 * pricing.output
        + usage.cache_read as f64 * pricing.cache_read
        + usage.cache_write as f64 * pricing.cache_write;
    micro / PER_MILLION
}

// ── baseline_usd ────────────────────────────────────────────────────────────────

/// USD cost the *same* `usage` would incur with **no** prompt cache.
///
/// Without a cache every prompt token — whether it was read from or written to the
/// cache — is billed at the full `input` rate; only `output` is unchanged:
/// `(input + cache_read + cache_write)·input_rate + output·output_rate`, per million.
///
/// This is the counterfactual against which [`cache_savings_usd`] is measured.
pub fn baseline_usd(usage: &TokenUsage, pricing: &Pricing) -> f64 {
    let prompt_tokens = usage
        .input
        .saturating_add(usage.cache_read)
        .saturating_add(usage.cache_write);
    let micro = prompt_tokens as f64 * pricing.input + usage.output as f64 * pricing.output;
    micro / PER_MILLION
}

// ── cache_savings_usd ───────────────────────────────────────────────────────────

/// Dollars saved by serving prompt tokens from the cache instead of paying the
/// full input rate: `baseline_usd - cost_usd`, clamped to `0.0`.
///
/// The clamp guards against pathological price sheets where a cache rate exceeds
/// the input rate, which would otherwise report a negative "saving".
pub fn cache_savings_usd(usage: &TokenUsage, pricing: &Pricing) -> f64 {
    (baseline_usd(usage, pricing) - cost_usd(usage, pricing)).max(0.0)
}

// ── Spend ───────────────────────────────────────────────────────────────────────

/// Running USD tally across many completions.
///
/// `gross_usd` is the sum of [`cost_usd`]; `saved_usd` is the sum of
/// [`cache_savings_usd`]. The pre-cache baseline is recoverable as
/// `gross_usd + saved_usd`. `Eq` is not derived because the fields are `f64`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spend {
    /// Total actual cost paid, in USD.
    pub gross_usd: f64,
    /// Total saved by the prompt cache, in USD.
    pub saved_usd: f64,
}

impl Spend {
    /// A zeroed tally.
    pub const ZERO: Self = Self { gross_usd: 0.0, saved_usd: 0.0 };

    /// Accumulate one completion's `usage` priced at `pricing`.
    pub fn add(&mut self, usage: &TokenUsage, pricing: &Pricing) {
        self.gross_usd += cost_usd(usage, pricing);
        self.saved_usd += cache_savings_usd(usage, pricing);
    }

    /// The counterfactual no-cache cost: `gross_usd + saved_usd`.
    pub fn baseline_usd(&self) -> f64 {
        self.gross_usd + self.saved_usd
    }

    /// Fraction of the baseline cost saved by the cache, in `0.0..=1.0`.
    ///
    /// Returns `0.0` when nothing has been spent (baseline is zero), guarding the
    /// division.
    pub fn fraction_saved(&self) -> f64 {
        let baseline = self.baseline_usd();
        if baseline == 0.0 {
            0.0
        } else {
            self.saved_usd / baseline
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Anthropic-like price sheet (USD per million tokens).
    fn anthropic_pricing() -> Pricing {
        Pricing { input: 3.0, output: 15.0, cache_read: 0.30, cache_write: 3.75 }
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    // ── cost_usd ────────────────────────────────────────────────────────────────

    #[test]
    fn cost_usd_known_numbers() {
        // 1M input, 1M output, 1M cache_read, 1M cache_write under the sheet above.
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        // 3.0 + 15.0 + 0.30 + 3.75 = 22.05
        approx(cost_usd(&usage, &anthropic_pricing()), 22.05);
    }

    #[test]
    fn cost_usd_zero_usage_is_zero() {
        approx(cost_usd(&TokenUsage::default(), &anthropic_pricing()), 0.0);
    }

    #[test]
    fn cost_usd_local_zero_pricing_is_zero() {
        let usage = TokenUsage { input: 5_000, output: 9_000, cache_read: 1_000, cache_write: 2_000 };
        approx(cost_usd(&usage, &Pricing::ZERO), 0.0);
    }

    #[test]
    fn cache_read_is_roughly_a_tenth_of_input() {
        // cache_read rate (0.30) is 0.1x the input rate (3.0) in the Anthropic sheet.
        let p = anthropic_pricing();
        approx(p.cache_read, p.input * 0.1);
    }

    // ── baseline_usd ──────────────────────────────────────────────────────────────

    #[test]
    fn baseline_usd_bills_all_prompt_tokens_at_input_rate() {
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        // (1M + 1M + 1M) * 3.0 + 1M * 15.0 = 9.0 + 15.0 = 24.0
        approx(baseline_usd(&usage, &anthropic_pricing()), 24.0);
    }

    #[test]
    fn baseline_equals_cost_when_no_cache_tokens() {
        // With no cache_read/cache_write there is nothing to discount.
        let usage = TokenUsage { input: 500_000, output: 250_000, cache_read: 0, cache_write: 0 };
        let p = anthropic_pricing();
        approx(baseline_usd(&usage, &p), cost_usd(&usage, &p));
    }

    #[test]
    fn baseline_usd_zero_usage_is_zero() {
        approx(baseline_usd(&TokenUsage::default(), &anthropic_pricing()), 0.0);
    }

    // ── cache_savings_usd ─────────────────────────────────────────────────────────

    #[test]
    fn cache_savings_is_baseline_minus_cost() {
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        let p = anthropic_pricing();
        // 24.0 - 22.05 = 1.95
        approx(cache_savings_usd(&usage, &p), baseline_usd(&usage, &p) - cost_usd(&usage, &p));
        approx(cache_savings_usd(&usage, &p), 1.95);
    }

    #[test]
    fn cache_savings_is_never_negative() {
        // Pathological sheet: cache rates above the input rate would imply a
        // "negative saving" — must clamp to 0.
        let usage = TokenUsage { input: 0, output: 0, cache_read: 1_000_000, cache_write: 0 };
        let bad = Pricing { input: 1.0, output: 1.0, cache_read: 5.0, cache_write: 5.0 };
        assert_eq!(cache_savings_usd(&usage, &bad), 0.0);
    }

    #[test]
    fn cache_savings_zero_usage_is_zero() {
        approx(cache_savings_usd(&TokenUsage::default(), &anthropic_pricing()), 0.0);
    }

    // ── Spend ─────────────────────────────────────────────────────────────────────

    #[test]
    fn spend_zero_is_all_zero() {
        let s = Spend::ZERO;
        approx(s.gross_usd, 0.0);
        approx(s.saved_usd, 0.0);
        approx(s.baseline_usd(), 0.0);
    }

    #[test]
    fn spend_default_matches_zero() {
        assert_eq!(Spend::default(), Spend::ZERO);
    }

    #[test]
    fn spend_accumulates_across_adds() {
        let p = anthropic_pricing();
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        let mut s = Spend::ZERO;
        s.add(&usage, &p);
        s.add(&usage, &p);
        approx(s.gross_usd, 2.0 * 22.05);
        approx(s.saved_usd, 2.0 * 1.95);
        approx(s.baseline_usd(), 2.0 * 24.0);
    }

    #[test]
    fn fraction_saved_guards_zero_baseline() {
        assert_eq!(Spend::ZERO.fraction_saved(), 0.0);
    }

    #[test]
    fn fraction_saved_is_saved_over_baseline() {
        let p = anthropic_pricing();
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        let mut s = Spend::ZERO;
        s.add(&usage, &p);
        // 1.95 / 24.0
        approx(s.fraction_saved(), 1.95 / 24.0);
    }

    #[test]
    fn fraction_saved_within_unit_interval() {
        let p = anthropic_pricing();
        let usage = TokenUsage { input: 100, output: 50, cache_read: 9_000, cache_write: 800 };
        let mut s = Spend::ZERO;
        s.add(&usage, &p);
        let f = s.fraction_saved();
        assert!((0.0..=1.0).contains(&f), "fraction_saved {f} out of range");
    }
}
