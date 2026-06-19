//! Build script: compile the real CUDA kernels *only* when the `cuda` feature is
//! enabled and `nvcc` is available. Otherwise the crate builds with its CPU
//! reference path so the product is complete and testable on any machine.

use std::process::Command;

fn main() {
    // Declare the custom cfg we may set, so rustc's check-cfg lint stays quiet.
    println!("cargo:rustc-check-cfg=cfg(cuda_built)");
    println!("cargo:rerun-if-changed=kernels/batch_invariant.cu");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_FEATURE_CUDA").is_err() {
        // Feature off: nothing to build, CPU reference is used.
        return;
    }

    let nvcc = which_nvcc();
    let Some(nvcc) = nvcc else {
        println!(
            "cargo:warning=oryn-cuda: `cuda` feature requested but `nvcc` was not found on PATH; \
             falling back to the CPU reference implementation."
        );
        return;
    };

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let obj = format!("{out_dir}/oryn_kernels.o");
    let lib = format!("{out_dir}/liborynkernels.a");

    // Compile the .cu to a relocatable object.
    let status = Command::new(&nvcc)
        .args([
            "-O3",
            "-Xcompiler",
            "-fPIC",
            "--compiler-options",
            "-Wall",
            "-c",
            "kernels/batch_invariant.cu",
            "-o",
            &obj,
        ])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            println!("cargo:warning=oryn-cuda: nvcc failed ({s}); using CPU reference.");
            return;
        }
        Err(e) => {
            println!("cargo:warning=oryn-cuda: could not run nvcc ({e}); using CPU reference.");
            return;
        }
    }

    // Archive into a static lib.
    let ar = Command::new("ar").args(["crus", &lib, &obj]).status();
    if !matches!(ar, Ok(s) if s.success()) {
        println!("cargo:warning=oryn-cuda: `ar` failed to build static lib; using CPU reference.");
        return;
    }

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=orynkernels");
    // Link the CUDA runtime from the conventional install location.
    for cand in ["/usr/local/cuda/lib64", "/usr/local/cuda/lib"] {
        if std::path::Path::new(cand).exists() {
            println!("cargo:rustc-link-search=native={cand}");
        }
    }
    println!("cargo:rustc-link-lib=dylib=cudart");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    // Signal the Rust code that real kernels are linked.
    println!("cargo:rustc-cfg=cuda_built");
}

fn which_nvcc() -> Option<String> {
    let out = Command::new("nvcc").arg("--version").output().ok()?;
    out.status.success().then(|| "nvcc".to_string())
}
