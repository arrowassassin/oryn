//! Classical statistics for flaky-test scoring — no ML, no models.
//!
//! A flaky test is a Bernoulli trial whose unknown failure probability we want to
//! bound. The Wilson score interval gives a sound two-sided interval for that
//! proportion — far better than the Wald interval near 0 or 1, which is exactly
//! where flake rates live. Everything here is deterministic.

/// A two-sided confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConfidenceInterval {
    /// Point estimate (e.g. the observed flake rate).
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

/// Wilson score interval for a binomial proportion — far better than the Wald
/// interval for rate-style (0/1) outcomes, especially near 0 or 1.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn z_critical_95() {
        assert!(approx(z_critical(0.95), 1.959_964, 1e-3));
    }

    #[test]
    fn ppf_is_monotonic_and_centered() {
        assert!(approx(normal_ppf(0.5), 0.0, 1e-9));
        assert!(normal_ppf(0.1) < normal_ppf(0.9));
        assert!(approx(normal_ppf(0.975), 1.959_964, 1e-3));
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
    fn margin_is_half_width() {
        let ci = ConfidenceInterval {
            estimate: 0.5,
            low: 0.4,
            high: 0.6,
            level: 0.95,
        };
        assert!(approx(ci.margin(), 0.1, 1e-12));
    }
}
