//! Bayesian Beta-Binomial estimation for flake rates.
//!
//! Treating each rerun as a Bernoulli trial, the failure probability `p` under a
//! Beta(α, β) prior has a Beta(α + fails, β + passes) posterior. With the
//! Jeffreys prior (α = β = 0.5) this gives a principled small-sample estimate
//! and an exact equal-tailed **credible interval** — a Bayesian companion to the
//! frequentist Wilson interval in [`stats`](crate::stats).
//!
//! The numerics (log-gamma, regularized incomplete beta via Lentz's continued
//! fraction, and quantiles by bisection) are standard and self-contained.

use serde::{Deserialize, Serialize};

/// Lanczos log-gamma approximation (g = 7).
fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection formula.
        std::f64::consts::PI.ln() - (std::f64::consts::PI * x).sin().ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let mut a = C[0];
        let t = x + G + 0.5;
        for (i, &c) in C.iter().enumerate().skip(1) {
            a += c / (x + i as f64);
        }
        0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

/// Continued fraction for the incomplete beta function (Lentz's algorithm).
fn betacf(x: f64, a: f64, b: f64) -> f64 {
    const MAXIT: usize = 200;
    const EPS: f64 = 3.0e-12;
    const FPMIN: f64 = 1.0e-300;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=MAXIT {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// Regularized incomplete beta function `I_x(a, b)` — the CDF of Beta(a, b).
#[must_use]
pub fn reg_incomplete_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bt = (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * betacf(x, a, b) / a
    } else {
        1.0 - bt * betacf(1.0 - x, b, a) / b
    }
}

/// Quantile (inverse CDF) of Beta(a, b) for probability `p`, by bisection.
#[must_use]
pub fn beta_quantile(p: f64, a: f64, b: f64) -> f64 {
    if p <= 0.0 {
        return 0.0;
    }
    if p >= 1.0 {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if reg_incomplete_beta(mid, a, b) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Posterior estimate of a flake rate under a Beta prior.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Posterior {
    /// Posterior mean of the failure probability.
    pub mean: f64,
    /// Lower bound of the equal-tailed credible interval.
    pub low: f64,
    /// Upper bound of the credible interval.
    pub high: f64,
    /// Credible level (e.g. 0.95).
    pub level: f64,
}

/// Beta-Binomial posterior for `fails` failures out of `fails + passes` runs,
/// with prior Beta(`prior_a`, `prior_b`). Use `prior_a = prior_b = 0.5` for the
/// Jeffreys prior.
#[must_use]
pub fn beta_binomial(fails: u64, passes: u64, prior_a: f64, prior_b: f64, level: f64) -> Posterior {
    let a = prior_a + fails as f64;
    let b = prior_b + passes as f64;
    let alpha = (1.0 - level) / 2.0;
    Posterior {
        mean: a / (a + b),
        low: beta_quantile(alpha, a, b),
        high: beta_quantile(1.0 - alpha, a, b),
        level,
    }
}

/// Jeffreys-prior Beta-Binomial posterior (α = β = 0.5).
#[must_use]
pub fn jeffreys(fails: u64, passes: u64, level: f64) -> Posterior {
    beta_binomial(fails, passes, 0.5, 0.5, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn incomplete_beta_uniform_is_identity() {
        // Beta(1,1) is Uniform(0,1): I_x(1,1) = x.
        assert!(approx(reg_incomplete_beta(0.3, 1.0, 1.0), 0.3, 1e-9));
        assert!(approx(reg_incomplete_beta(0.75, 1.0, 1.0), 0.75, 1e-9));
    }

    #[test]
    fn incomplete_beta_symmetric_median() {
        // Beta(2,2) is symmetric about 0.5.
        assert!(approx(reg_incomplete_beta(0.5, 2.0, 2.0), 0.5, 1e-9));
    }

    #[test]
    fn quantile_inverts_cdf() {
        for &(a, b) in &[(2.0, 5.0), (0.5, 0.5), (10.0, 3.0)] {
            for &p in &[0.1, 0.5, 0.9] {
                let x = beta_quantile(p, a, b);
                assert!(
                    approx(reg_incomplete_beta(x, a, b), p, 1e-6),
                    "a={a} b={b} p={p}"
                );
            }
        }
    }

    #[test]
    fn ln_gamma_known_values() {
        // Gamma(5) = 24 -> ln 24; Gamma(0.5) = sqrt(pi).
        assert!(approx(ln_gamma(5.0), 24.0_f64.ln(), 1e-7));
        assert!(approx(
            ln_gamma(0.5),
            std::f64::consts::PI.sqrt().ln(),
            1e-7
        ));
    }

    #[test]
    fn posterior_brackets_the_rate() {
        // 5 failures in 100 runs, Jeffreys prior.
        let p = jeffreys(5, 95, 0.95);
        assert!(approx(p.mean, 5.5 / 101.0, 1e-9));
        assert!(p.low > 0.0 && p.low < p.mean && p.mean < p.high && p.high < 0.2);
    }

    #[test]
    fn clean_run_posterior_upper_bound_shrinks_with_more_runs() {
        // More clean runs -> tighter upper bound on the (unseen) flake rate.
        let few = jeffreys(0, 10, 0.95).high;
        let many = jeffreys(0, 200, 0.95).high;
        assert!(many < few);
        assert!(many > 0.0);
    }
}
