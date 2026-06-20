//! Bisection — isolate the first failing element of a monotone sequence in
//! O(log n) evaluations.
//!
//! This is the engine behind **batch testing with bisection** (Beheshtian et
//! al., EMSE 2024): run a batch of changes/tests together; if the batch fails,
//! binary-search to the first failing one instead of running each individually.
//! It finds the culprit with zero missed failures and exponentially fewer runs.

/// Find the smallest index `i` in `0..n` for which `failing(i)` is true,
/// assuming `failing` is **monotone** (once true, stays true). Returns `None`
/// if nothing fails. Evaluates `failing` `O(log n)` times.
pub fn first_failing<F: FnMut(usize) -> bool>(n: usize, mut failing: F) -> Option<usize> {
    if n == 0 || !failing(n - 1) {
        return None;
    }
    let (mut lo, mut hi) = (0usize, n - 1); // invariant: failing(hi) == true
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if failing(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn finds_first_failing() {
        assert_eq!(first_failing(8, |i| i >= 3), Some(3));
        assert_eq!(first_failing(8, |_| true), Some(0));
        assert_eq!(first_failing(8, |i| i >= 7), Some(7));
    }

    #[test]
    fn none_failing_is_none() {
        assert_eq!(first_failing(5, |_| false), None);
        assert_eq!(first_failing(0, |_| true), None);
    }

    #[test]
    fn is_logarithmic() {
        // 1024 elements should take ~11 evaluations, never linear.
        let calls = Cell::new(0usize);
        let idx = first_failing(1024, |i| {
            calls.set(calls.get() + 1);
            i >= 700
        });
        assert_eq!(idx, Some(700));
        assert!(calls.get() <= 12, "took {} evals", calls.get());
    }
}
