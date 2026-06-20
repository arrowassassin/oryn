//! Pure logic for `oryn tune`: which sound, stable compile-acceleration config
//! applies to this toolchain/workspace. Kept free of I/O so it is unit-testable;
//! the CLI gathers the inputs (host triple, rustc version, tool presence) and
//! writes the file.

/// A semantic version triple.
pub type Semver = (u32, u32, u32);

/// Parse the `release: x.y.z` line out of `rustc -vV` output.
#[must_use]
pub fn parse_rustc_semver(vv: &str) -> Option<Semver> {
    let rel = vv.lines().find_map(|l| l.strip_prefix("release: "))?.trim();
    let core = rel.split(['-', '+']).next().unwrap_or(rel);
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next().unwrap_or("0").parse().unwrap_or(0);
    Some((a, b, c))
}

/// Is rust-lld already the *default* linker for this toolchain? True only for
/// `x86_64-unknown-linux-gnu` on Rust ≥ 1.90.0 — the single target/version where
/// it was stabilized as the default (rust-lld stable, 1.90.0, 2025-09-18). Any
/// other target/version still uses the system linker by default.
#[must_use]
pub fn rust_lld_is_default(host: &str, version: Semver) -> bool {
    host == "x86_64-unknown-linux-gnu" && version >= (1, 90, 0)
}

/// Does this workspace look large enough to benefit from a `workspace-hack`
/// (cargo-hakari) and not already have one? Coarse, advice-only heuristic: a
/// workspace-hack unifies dependency feature sets so shared deps build once.
#[must_use]
pub fn hakari_advice(member_names: &[String]) -> Option<String> {
    let has_hack = member_names
        .iter()
        .any(|n| n == "workspace-hack" || n.ends_with("-workspace-hack"));
    if member_names.len() >= 4 && !has_hack {
        Some(format!(
            "{} workspace members and no workspace-hack: dependencies may be \
             compiled with several feature sets. Consider `cargo install \
             cargo-hakari` then `cargo hakari init` — it unifies features so \
             shared deps build once (measured ~1.7× on large workspaces).",
            member_names.len()
        ))
    } else {
        None
    }
}

/// The sound, stable `.cargo/config.toml` Oryn writes with `tune --apply`.
///
/// * `debug = "line-tables-only"` on dev — keeps backtraces, ~20–40% dev build
///   win; sound (never changes program behaviour).
/// * `split-debuginfo = "unpacked"` on Linux/macOS — saves link time.
/// * a mold linker stanza *only* when mold (and clang to drive it) are present
///   and rust-lld isn't already the default — never points at a linker that
///   isn't installed.
#[must_use]
pub fn cargo_config(
    host: &str,
    os: &str,
    version: Semver,
    mold_present: bool,
    clang_present: bool,
) -> String {
    let mut s = String::from(
        "# Written by `oryn tune --apply`. Sound, stable compile-speed defaults.\n\n\
         [profile.dev]\n\
         # Keep line numbers in backtraces, drop the rest of debuginfo (~20–40% faster dev builds).\n\
         debug = \"line-tables-only\"\n",
    );
    if os == "linux" || os == "macos" {
        s.push_str(
            "# Store debuginfo beside the binary so the linker skips copying it in.\n\
             split-debuginfo = \"unpacked\"\n",
        );
    }
    let add_mold = mold_present && clang_present && os == "linux";
    if add_mold {
        s.push('\n');
        if rust_lld_is_default(host, version) {
            s.push_str("# rust-lld is already the default here; mold is an extra step for link-heavy builds.\n");
        }
        s.push_str(&format!(
            "[target.{host}]\n\
             linker = \"clang\"\n\
             rustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rustc_release() {
        assert_eq!(
            parse_rustc_semver("rustc 1.96.0\nrelease: 1.96.0\nhost: x"),
            Some((1, 96, 0))
        );
        assert_eq!(
            parse_rustc_semver("release: 1.90.0-nightly\n"),
            Some((1, 90, 0))
        );
        assert_eq!(parse_rustc_semver("no release line"), None);
    }

    #[test]
    fn rust_lld_default_is_target_and_version_gated() {
        assert!(rust_lld_is_default("x86_64-unknown-linux-gnu", (1, 96, 0)));
        assert!(rust_lld_is_default("x86_64-unknown-linux-gnu", (1, 90, 0)));
        // older version → not default
        assert!(!rust_lld_is_default("x86_64-unknown-linux-gnu", (1, 89, 0)));
        // other targets → not default even on new rustc
        assert!(!rust_lld_is_default(
            "aarch64-unknown-linux-gnu",
            (1, 96, 0)
        ));
        assert!(!rust_lld_is_default("x86_64-pc-windows-msvc", (1, 96, 0)));
        assert!(!rust_lld_is_default("aarch64-apple-darwin", (1, 96, 0)));
    }

    #[test]
    fn hakari_only_for_large_workspaces_without_a_hack() {
        let small = vec!["a".into(), "b".into()];
        assert!(hakari_advice(&small).is_none());
        let big: Vec<String> = (0..5).map(|i| format!("c{i}")).collect();
        assert!(hakari_advice(&big).is_some());
        let big_with_hack = vec![
            "a".into(),
            "b".into(),
            "c".into(),
            "d".into(),
            "workspace-hack".into(),
        ];
        assert!(hakari_advice(&big_with_hack).is_none());
    }

    #[test]
    fn config_is_sound_and_conditional() {
        // line-tables-only always present
        let c = cargo_config("x86_64-unknown-linux-gnu", "linux", (1, 96, 0), true, true);
        assert!(c.contains("debug = \"line-tables-only\""));
        assert!(c.contains("split-debuginfo = \"unpacked\""));
        assert!(c.contains("fuse-ld=mold"));
        // no mold on PATH → never write a mold stanza pointing at a missing tool
        let c2 = cargo_config("x86_64-unknown-linux-gnu", "linux", (1, 96, 0), false, true);
        assert!(!c2.contains("mold"));
        // macOS: split-debuginfo yes, mold no (linux-only)
        let c3 = cargo_config("aarch64-apple-darwin", "macos", (1, 96, 0), true, true);
        assert!(c3.contains("split-debuginfo"));
        assert!(!c3.contains("mold"));
    }
}
