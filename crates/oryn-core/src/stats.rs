//! Classical statistics for rigorous evaluation — no ML, no models.
//!
//! Implements the machinery behind "Adding Error Bars to Evals"
//! (Miller, 2024, arXiv:2411.00640): treat eval items as a sample from a
//! super-population, report confidence intervals and statistical power, and
//! compare models with *paired* differences that account for question-level
//! correlation. Everything is deterministic; the bootstrap is seeded.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// A two-sided confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceInterval {
    /// Point estimate (e.g. the mean score).
    pub estimate: f64,
    /// Lower bound.
    pub low: f64,
    /// Upper bound.
    pub high: f64,
    /// Confidence level used, e.g. 0.95.
    pub level: f64,
}

impl ConfidenceInterval {
    /// Half-width of the interval (the "± error bar").
    #[must_use]
    pub fn margin(&self) -> f64 {
        (self.high - self.low) / 2.0
    }
}

/// Error function approximation (Abramowitz & Stegun 7.1.26), max abs err ~1.5e-7.
#[must_use]
pub fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Standard normal CDF, Φ(x).
#[must_use]
pub fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Inverse standard normal CDF (Acklam's algorithm), Φ⁻¹(p) for p in (0,1).
#[must_use]
pub fn normal_ppf(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    // Coefficients.
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239e0,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838e0,
        -2.549_732_539_343_734e0,
        4.374_664_141_464_968e0,
        2.938_163_982_698_783e0,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996e0,
        3.754_408_661_907_416e0,
    ];
    let plow = 0.024_25;
    let phigh = 1.0 - plow;
    if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= phigh {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// z critical value for a two-sided interval at `level` (e.g. 0.95 → 1.959964).
#[must_use]
pub fn z_critical(level: f64) -> f64 {
    normal_ppf(1.0 - (1.0 - level) / 2.0)
}

/// Mean of a slice.
#[must_use]
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Sample variance (Bessel-corrected, n-1).
#[must_use]
pub fn sample_variance(xs: &[f64]) -> f64 {
    let n = xs.len();
    if n < 2 {
        return 0.0;
    }
    let m = mean(xs);
    xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n as f64 - 1.0)
}

/// Standard error of the mean.
#[must_use]
pub fn standard_error(xs: &[f64]) -> f64 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    (sample_variance(xs) / n as f64).sqrt()
}

/// Normal-approximation CI for the mean of `xs`.
#[must_use]
pub fn mean_ci(xs: &[f64], level: f64) -> ConfidenceInterval {
    let est = mean(xs);
    let se = standard_error(xs);
    let z = z_critical(level);
    ConfidenceInterval {
        estimate: est,
        low: est - z * se,
        high: est + z * se,
        level,
    }
}

/// Wilson score interval for a binomial proportion — far better than the Wald
/// interval for accuracy-style (0/1) eval scores, especially near 0 or 1.
#[must_use]
pub fn wilson_interval(successes: u64, n: u64, level: f64) -> ConfidenceInterval {
    let est = if n == 0 {
        0.0
    } else {
        successes as f64 / n as f64
    };
    if n == 0 {
        return ConfidenceInterval {
            estimate: 0.0,
            low: 0.0,
            high: 1.0,
            level,
        };
    }
    let z = z_critical(level);
    let n = n as f64;
    let p = est;
    let denom = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denom;
    let half = (z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt()) / denom;
    ConfidenceInterval {
        estimate: est,
        low: (center - half).max(0.0),
        high: (center + half).min(1.0),
        level,
    }
}

/// Seeded percentile bootstrap CI for the mean — robust to non-normal scores.
///
/// `seed` makes the resampling fully reproducible.
#[must_use]
pub fn bootstrap_mean_ci(
    xs: &[f64],
    level: f64,
    resamples: usize,
    seed: u64,
) -> ConfidenceInterval {
    let est = mean(xs);
    if xs.len() < 2 || resamples == 0 {
        return ConfidenceInterval {
            estimate: est,
            low: est,
            high: est,
            level,
        };
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n = xs.len();
    let mut means = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut acc = 0.0;
        for _ in 0..n {
            // Deterministic index draw from the seeded stream.
            let idx = (next_u64(&mut rng) % n as u64) as usize;
            acc += xs[idx];
        }
        means.push(acc / n as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let alpha = 1.0 - level;
    let lo_idx = ((alpha / 2.0) * resamples as f64).floor() as usize;
    let hi_idx = (((1.0 - alpha / 2.0) * resamples as f64).ceil() as usize)
        .saturating_sub(1)
        .min(resamples - 1);
    ConfidenceInterval {
        estimate: est,
        low: means[lo_idx.min(resamples - 1)],
        high: means[hi_idx],
        level,
    }
}

fn next_u64(rng: &mut ChaCha8Rng) -> u64 {
    use rand::RngCore;
    rng.next_u64()
}

/// Two-sided p-value (normal approx) for a z statistic.
#[must_use]
pub fn two_sided_p(z: f64) -> f64 {
    2.0 * (1.0 - normal_cdf(z.abs()))
}

/// Result of a paired comparison between two models on the same items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedComparison {
    /// Mean score of system A.
    pub mean_a: f64,
    /// Mean score of system B.
    pub mean_b: f64,
    /// Mean of (A − B) over items.
    pub mean_diff: f64,
    /// Standard error of the paired difference (uses item-level correlation).
    pub se_diff: f64,
    /// CI for the paired difference.
    pub diff_ci: ConfidenceInterval,
    /// z statistic for H0: mean_diff = 0.
    pub z: f64,
    /// Two-sided p-value.
    pub p_value: f64,
    /// Whether the difference is significant at (1 − level).
    pub significant: bool,
}

/// Compare two systems item-by-item (paired). `a[i]` and `b[i]` are the scores
/// of system A and B on the same item `i`.
///
/// # Errors
/// Returns an error if the slices differ in length or are empty.
pub fn paired_compare(a: &[f64], b: &[f64], level: f64) -> crate::Result<PairedComparison> {
    if a.len() != b.len() {
        return Err(crate::OrynError::LengthMismatch(format!(
            "paired_compare: a={} b={}",
            a.len(),
            b.len()
        )));
    }
    if a.is_empty() {
        return Err(crate::OrynError::EmptyInput("paired_compare".into()));
    }
    let diffs: Vec<f64> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    let mean_diff = mean(&diffs);
    let se_diff = standard_error(&diffs);
    let z = if se_diff > 0.0 {
        mean_diff / se_diff
    } else if mean_diff == 0.0 {
        0.0
    } else {
        f64::INFINITY
    };
    let zc = z_critical(level);
    let diff_ci = ConfidenceInterval {
        estimate: mean_diff,
        low: mean_diff - zc * se_diff,
        high: mean_diff + zc * se_diff,
        level,
    };
    let p_value = two_sided_p(z);
    Ok(PairedComparison {
        mean_a: mean(a),
        mean_b: mean(b),
        mean_diff,
        se_diff,
        diff_ci,
        z,
        p_value,
        significant: p_value < (1.0 - level),
    })
}

/// Statistical-power planning for a two-sided test of a single mean / paired diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerAnalysis {
    /// Significance level α (e.g. 0.05).
    pub alpha: f64,
    /// Target power 1 − β (e.g. 0.8).
    pub power: f64,
    /// Standardized effect size (Cohen's d = effect / sd).
    pub effect_size: f64,
    /// Sample size required to reach `power` at `alpha` for `effect_size`.
    pub required_n: u64,
    /// Minimal detectable effect (in score units) for the *current* n & sd.
    pub mde: f64,
}

/// Compute required-N and the minimal detectable effect.
///
/// `effect_size` is Cohen's d (raw effect divided by the score sd). `current_n`
/// and `current_sd` describe the eval you already ran, used to report the MDE.
#[must_use]
pub fn power_analysis(
    effect_size: f64,
    alpha: f64,
    power: f64,
    current_n: u64,
    current_sd: f64,
) -> PowerAnalysis {
    let za = normal_ppf(1.0 - alpha / 2.0);
    let zb = normal_ppf(power);
    let d = effect_size.abs().max(1e-9);
    let required_n = (((za + zb) / d).powi(2)).ceil().max(1.0) as u64;
    let mde = if current_n > 0 {
        (za + zb) * current_sd / (current_n as f64).sqrt()
    } else {
        f64::INFINITY
    };
    PowerAnalysis {
        alpha,
        power,
        effect_size,
        required_n,
        mde,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn normal_cdf_known_values() {
        assert!(approx(normal_cdf(0.0), 0.5, 1e-6));
        assert!(approx(normal_cdf(1.959_964), 0.975, 1e-3));
    }

    #[test]
    fn z_critical_95() {
        assert!(approx(z_critical(0.95), 1.959_964, 1e-3));
    }

    #[test]
    fn ppf_cdf_roundtrip() {
        for &p in &[0.01, 0.1, 0.5, 0.9, 0.99] {
            let x = normal_ppf(p);
            assert!(approx(normal_cdf(x), p, 1e-3), "p={p}");
        }
    }

    #[test]
    fn wilson_within_unit_interval() {
        let ci = wilson_interval(7, 10, 0.95);
        assert!(ci.low >= 0.0 && ci.high <= 1.0);
        assert!(ci.low < 0.7 && ci.high > 0.7);
    }

    #[test]
    fn wilson_extreme_is_bounded() {
        let ci = wilson_interval(10, 10, 0.95);
        assert!(ci.high <= 1.0);
        assert!(ci.low > 0.6); // not the degenerate [1,1] of Wald
    }

    #[test]
    fn bootstrap_is_reproducible() {
        let xs: Vec<f64> = (0..50).map(|i| (i % 2) as f64).collect();
        let a = bootstrap_mean_ci(&xs, 0.95, 1000, 42);
        let b = bootstrap_mean_ci(&xs, 0.95, 1000, 42);
        assert_eq!(a.low, b.low);
        assert_eq!(a.high, b.high);
    }

    #[test]
    fn paired_detects_clear_difference() {
        let a = vec![1.0; 100];
        let b = vec![0.0; 100];
        let cmp = paired_compare(&a, &b, 0.95).unwrap();
        assert!(approx(cmp.mean_diff, 1.0, 1e-9));
        assert!(cmp.significant);
    }

    #[test]
    fn paired_no_difference_not_significant() {
        let a = vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        let b = a.clone();
        let cmp = paired_compare(&a, &b, 0.95).unwrap();
        assert!(!cmp.significant);
        assert!(approx(cmp.mean_diff, 0.0, 1e-9));
    }

    #[test]
    fn paired_length_mismatch_errors() {
        assert!(paired_compare(&[1.0, 2.0], &[1.0], 0.95).is_err());
    }

    #[test]
    fn power_required_n_grows_as_effect_shrinks() {
        let small = power_analysis(0.1, 0.05, 0.8, 100, 0.5);
        let large = power_analysis(0.5, 0.05, 0.8, 100, 0.5);
        assert!(small.required_n > large.required_n);
        // d=0.5, two-sided 0.05, power 0.8 -> ~31-32 per classic tables.
        assert!(large.required_n >= 30 && large.required_n <= 34);
    }
}
