//! # oryn-cuda — batch-invariant kernels for reproducible inference
//!
//! Nondeterminism in LLM inference at temperature 0 comes from reduction kernels
//! changing their floating-point **reduction order** with batch size / tiling.
//! Because float addition is not associative, a different grouping changes the
//! low bits, which can flip a token. (Thinking Machines Lab, 2025.)
//!
//! This crate ships the fix two ways:
//!
//! * **Real CUDA kernels** (`kernels/batch_invariant.cu`) compiled and linked
//!   when the `cuda` feature is on and `nvcc` is present.
//! * A **CPU reference** with identical semantics so the behavior is testable on
//!   any machine — including the demonstration that tiling-dependent reductions
//!   diverge while the batch-invariant reduction does not.
//!
//! Call [`backend`] to see which path is active.

/// Which compute backend is active for this build.
#[must_use]
pub fn backend() -> &'static str {
    if cuda_available() {
        "cuda"
    } else {
        "cpu-reference"
    }
}

/// Whether real CUDA kernels were compiled and linked.
#[must_use]
pub const fn cuda_available() -> bool {
    cfg!(cuda_built)
}

/// Batch-**invariant** reduction: accumulate strictly in index order.
///
/// The result is independent of how the data would be tiled across threads /
/// batches, so it is bitwise reproducible. Mirrors `batch_invariant_sum_kernel`.
#[must_use]
pub fn batch_invariant_sum(xs: &[f32]) -> f32 {
    #[cfg(cuda_built)]
    {
        if !xs.is_empty() {
            return unsafe { ffi::oryn_cuda_batch_invariant_sum(xs.as_ptr(), xs.len() as i32) };
        }
    }
    let mut acc = 0.0f32;
    for &x in xs {
        acc += x;
    }
    acc
}

/// Batch-**variant** reduction: sum contiguous chunks of size `chunk` into
/// partials, then sum the partials. This models how GPU tiling / batch size
/// changes the floating-point grouping. Different `chunk` values can yield
/// different bits for the same data — the bug we are fixing.
#[must_use]
pub fn noninvariant_sum(xs: &[f32], chunk: usize) -> f32 {
    let chunk = chunk.max(1);
    let mut partials = Vec::new();
    for c in xs.chunks(chunk) {
        let mut p = 0.0f32;
        for &x in c {
            p += x;
        }
        partials.push(p);
    }
    let mut acc = 0.0f32;
    for p in partials {
        acc += p;
    }
    acc
}

/// Returns true if the (non-invariant) reduction produces *different* results
/// under any of the given tiling `chunks` — i.e. the data is sensitive to batch
/// size. The batch-invariant sum, by contrast, is constant across all tilings.
#[must_use]
pub fn reduction_varies_under_tiling(xs: &[f32], chunks: &[usize]) -> bool {
    let mut seen: Option<u32> = None;
    for &c in chunks {
        let bits = noninvariant_sum(xs, c).to_bits();
        match seen {
            None => seen = Some(bits),
            Some(prev) if prev != bits => return true,
            _ => {}
        }
    }
    false
}

/// Batch-invariant matrix–vector product `y = A·x` where `a` is `m×k` row-major.
///
/// Each row's K-reduction is in fixed index order regardless of tiling, so `y`
/// is reproducible. Mirrors `batch_invariant_matvec_kernel`.
///
/// # Panics
/// Panics if `a.len() != m*k` or `x.len() != k`.
#[must_use]
pub fn batch_invariant_matvec(a: &[f32], x: &[f32], m: usize, k: usize) -> Vec<f32> {
    assert_eq!(a.len(), m * k, "matrix shape mismatch");
    assert_eq!(x.len(), k, "vector length mismatch");

    #[cfg(cuda_built)]
    {
        if m > 0 && k > 0 {
            let mut y = vec![0.0f32; m];
            unsafe {
                ffi::oryn_cuda_batch_invariant_matvec(
                    a.as_ptr(),
                    x.as_ptr(),
                    m as i32,
                    k as i32,
                    y.as_mut_ptr(),
                );
            }
            return y;
        }
    }

    let mut y = vec![0.0f32; m];
    for (row, yi) in y.iter_mut().enumerate() {
        let base = row * k;
        let mut acc = 0.0f32;
        for j in 0..k {
            acc += a[base + j] * x[j];
        }
        *yi = acc;
    }
    y
}

#[cfg(cuda_built)]
mod ffi {
    extern "C" {
        pub fn oryn_cuda_batch_invariant_sum(x: *const f32, n: i32) -> f32;
        pub fn oryn_cuda_batch_invariant_matvec(
            a: *const f32,
            x: *const f32,
            m: i32,
            k: i32,
            y: *mut f32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2^24 in f32 has a ulp of 2, so `2^24 + 1 == 2^24`. Grouping the small
    /// values first makes them survive — a clean batch-variance demonstrator.
    const BIG: f32 = 16_777_216.0; // 2^24

    #[test]
    fn batch_invariant_sum_is_index_order() {
        let xs = [BIG, 1.0, 1.0, 1.0];
        // Each +1 is lost against BIG: stays at 2^24.
        assert_eq!(batch_invariant_sum(&xs), BIG);
    }

    #[test]
    fn tiling_changes_noninvariant_result() {
        let xs = [BIG, 1.0, 1.0, 1.0];
        let seq = noninvariant_sum(&xs, 1); // == sequential == BIG
        let tiled = noninvariant_sum(&xs, 2); // groups the 1s: BIG + 2
        assert_ne!(seq.to_bits(), tiled.to_bits());
        assert_eq!(seq, BIG);
        assert_eq!(tiled, BIG + 2.0);
    }

    #[test]
    fn invariant_sum_unaffected_by_tiling_choice() {
        // The whole point: the invariant reduction has no tiling parameter and
        // equals the fixed index-order result no matter what.
        let xs = [BIG, 1.0, 1.0, 1.0];
        assert_eq!(batch_invariant_sum(&xs), noninvariant_sum(&xs, 1));
    }

    #[test]
    fn reduction_varies_flag() {
        let sensitive = [BIG, 1.0, 1.0, 1.0];
        assert!(reduction_varies_under_tiling(&sensitive, &[1, 2, 4]));

        let benign = [1.0, 2.0, 3.0, 4.0];
        assert!(!reduction_varies_under_tiling(&benign, &[1, 2, 4]));
    }

    #[test]
    fn matvec_correct_and_reproducible() {
        // A = [[1,2],[3,4]], x = [1,1] -> y = [3,7]
        let a = [1.0, 2.0, 3.0, 4.0];
        let x = [1.0, 1.0];
        let y1 = batch_invariant_matvec(&a, &x, 2, 2);
        let y2 = batch_invariant_matvec(&a, &x, 2, 2);
        assert_eq!(y1, vec![3.0, 7.0]);
        assert_eq!(y1, y2);
    }

    #[test]
    fn backend_is_cpu_without_nvcc() {
        // In CI without a GPU toolkit, we must be on the reference path.
        if !cuda_available() {
            assert_eq!(backend(), "cpu-reference");
        }
    }
}
