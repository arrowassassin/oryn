// Batch-invariant reduction kernels for reproducible LLM inference.
//
// The user-visible nondeterminism in LLM inference at temperature 0 is not RNG:
// it is that reduction kernels (sum, matmul-K, RMSNorm) change their
// floating-point *reduction order* depending on the batch size / tiling the
// request happens to land in. Floating-point addition is not associative, so a
// different grouping yields different bits, which can flip a token and cascade.
// (See Thinking Machines Lab, "Defeating Nondeterminism in LLM Inference", 2025.)
//
// The fix is a kernel whose reduction order is fixed regardless of batch/tiling.
// Below: a deliberately NON-invariant kernel (atomicAdd, order depends on warp
// scheduling) to demonstrate the problem, and a batch-INVARIANT kernel that
// always reduces in the same canonical order.
//
// These are compiled to a static library and linked into `oryn-cuda` only when
// the `cuda` feature is set and `nvcc` is present. The Rust CPU reference path
// mirrors the exact same semantics for machines without a GPU.

#include <cuda_runtime.h>

extern "C" {

// ---------------------------------------------------------------------------
// NON-invariant reduction: every thread atomically adds into one accumulator.
// The order of atomicAdd is determined by runtime warp scheduling, so the
// floating-point grouping — and therefore the low bits of the result — varies
// run to run and with occupancy (which itself depends on batch size).
// ---------------------------------------------------------------------------
__global__ void naive_atomic_sum_kernel(const float *x, int n, float *out) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) {
        atomicAdd(out, x[i]);
    }
}

float oryn_cuda_naive_sum(const float *host_x, int n) {
    float *d_x = nullptr, *d_out = nullptr;
    float zero = 0.0f, result = 0.0f;
    cudaMalloc(&d_x, n * sizeof(float));
    cudaMalloc(&d_out, sizeof(float));
    cudaMemcpy(d_x, host_x, n * sizeof(float), cudaMemcpyHostToDevice);
    cudaMemcpy(d_out, &zero, sizeof(float), cudaMemcpyHostToDevice);
    int threads = 256;
    int blocks = (n + threads - 1) / threads;
    naive_atomic_sum_kernel<<<blocks, threads>>>(d_x, n, d_out);
    cudaMemcpy(&result, d_out, sizeof(float), cudaMemcpyDeviceToHost);
    cudaFree(d_x);
    cudaFree(d_out);
    return result;
}

// ---------------------------------------------------------------------------
// Batch-INVARIANT reduction: a single thread accumulates strictly in index
// order. The reduction order is therefore independent of block/grid/batch
// configuration — the result is bitwise identical on every launch. (A real
// production kernel would use a fixed-shape hierarchical tree reduction with a
// split-K of 1; single-thread index order is the simplest provably-invariant
// form and is what the CPU reference mirrors.)
// ---------------------------------------------------------------------------
__global__ void batch_invariant_sum_kernel(const float *x, int n, float *out) {
    if (threadIdx.x == 0 && blockIdx.x == 0) {
        float acc = 0.0f;
        for (int i = 0; i < n; ++i) {
            acc += x[i];
        }
        *out = acc;
    }
}

float oryn_cuda_batch_invariant_sum(const float *host_x, int n) {
    float *d_x = nullptr, *d_out = nullptr;
    float result = 0.0f;
    cudaMalloc(&d_x, n * sizeof(float));
    cudaMalloc(&d_out, sizeof(float));
    cudaMemcpy(d_x, host_x, n * sizeof(float), cudaMemcpyHostToDevice);
    // Launch with an arbitrary, "batchy" configuration to prove invariance:
    // the result does not depend on these numbers.
    batch_invariant_sum_kernel<<<8, 128>>>(d_x, n, d_out);
    cudaMemcpy(&result, d_out, sizeof(float), cudaMemcpyDeviceToHost);
    cudaFree(d_x);
    cudaFree(d_out);
    return result;
}

// ---------------------------------------------------------------------------
// Batch-invariant matrix-vector product y = A x, A is (m x k) row-major.
// The K-reduction for each output row is in fixed index order, independent of
// how rows are tiled across threads — so y is bitwise reproducible.
// ---------------------------------------------------------------------------
__global__ void batch_invariant_matvec_kernel(const float *a, const float *x,
                                              int m, int k, float *y) {
    int row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row < m) {
        float acc = 0.0f;
        const float *arow = a + (long)row * k;
        for (int j = 0; j < k; ++j) {
            acc += arow[j] * x[j];
        }
        y[row] = acc;
    }
}

void oryn_cuda_batch_invariant_matvec(const float *host_a, const float *host_x,
                                      int m, int k, float *host_y) {
    float *d_a = nullptr, *d_x = nullptr, *d_y = nullptr;
    cudaMalloc(&d_a, (long)m * k * sizeof(float));
    cudaMalloc(&d_x, k * sizeof(float));
    cudaMalloc(&d_y, m * sizeof(float));
    cudaMemcpy(d_a, host_a, (long)m * k * sizeof(float), cudaMemcpyHostToDevice);
    cudaMemcpy(d_x, host_x, k * sizeof(float), cudaMemcpyHostToDevice);
    int threads = 128;
    int blocks = (m + threads - 1) / threads;
    batch_invariant_matvec_kernel<<<blocks, threads>>>(d_a, d_x, m, k, d_y);
    cudaMemcpy(host_y, d_y, m * sizeof(float), cudaMemcpyDeviceToHost);
    cudaFree(d_a);
    cudaFree(d_x);
    cudaFree(d_y);
}

} // extern "C"
