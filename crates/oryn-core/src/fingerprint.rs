//! Content fingerprinting of a crate and its dependency closure.
//!
//! A crate's test outcome is a pure function of: its own files, the files of
//! every crate it (transitively) depends on, the exact set of external
//! dependency versions (the lockfile), and the compiler. We capture that with a
//! **Merkle fingerprint**: `fp(c) = H(domain ‖ own(c) ‖ fp(d₁) ‖ … ‖ fp(dₙ))`
//! over the crate's workspace dependencies `dᵢ`, where `domain` is a
//! domain-separation prefix folding in the `rustc` version *and* the workspace
//! `Cargo.lock` (so a `cargo update` that bumps a transitive crates.io
//! dependency — changing no crate's own sources — still invalidates every
//! fingerprint). It is a deterministic content prefix, **not** a secret salt.
//!
//! `own(c)` hashes **every** file under the crate (not just `*.rs`), so assets
//! pulled in by `include_str!`/`include_bytes!`, `build.rs` inputs, fixtures,
//! etc. are covered too. This over-approximates (editing a crate's `README`
//! re-runs its tests) — the safe direction, matching the rest of the tool.
//!
//! If — and only if — a crate, anything in its dependency closure, the lockfile,
//! or the compiler changes, its fingerprint changes. That is exactly the
//! soundness condition for **caching test results**: a crate whose fingerprint
//! matches a previously-recorded green run cannot have a different test outcome
//! (modulo flakiness, handled separately), so its tests can be skipped.

use crate::graph::WorkspaceGraph;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// A 32-byte content digest.
pub type Digest = [u8; 32];

/// Hash arbitrary bytes (BLAKE3).
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> Digest {
    *blake3::hash(bytes).as_bytes()
}

/// Hash a file's contents.
///
/// # Errors
/// Propagates I/O errors reading the file.
pub fn hash_file(path: &Path) -> io::Result<Digest> {
    let mut hasher = blake3::Hasher::new();
    let mut f = std::fs::File::open(path)?;
    io::copy(&mut f, &mut hasher)?;
    Ok(*hasher.finalize().as_bytes())
}

/// Hex-encode a digest.
#[must_use]
pub fn to_hex(d: &Digest) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Compute the digest of a crate's *own* files: every regular file under
/// `manifest_dir`, excluding any nested `target/` directory and hidden
/// directories. Hashing all files (not just `*.rs`) keeps the cache sound for
/// crates that embed assets via `include_str!`/`include_bytes!` or read inputs
/// in `build.rs`. Files are hashed in a path-sorted, deterministic order.
///
/// # Errors
/// Propagates I/O errors walking or reading the tree.
pub fn crate_own_digest(manifest_dir: &Path) -> io::Result<Digest> {
    let mut entries: Vec<(String, Digest)> = Vec::new();
    collect(manifest_dir, manifest_dir, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = blake3::Hasher::new();
    for (rel, dig) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        hasher.update(dig);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Digest)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let ft = entry.file_type()?;
        // Skip symlinks entirely: following them can escape the crate, loop, or
        // (when dangling) abort the whole fingerprint. We record the link's
        // name + target so adding/removing/retargeting one still changes the
        // digest, without reading through it.
        if ft.is_symlink() {
            let target = std::fs::read_link(&path)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            let rel = rel_path(root, &path);
            out.push((rel, hash_bytes(format!("symlink:{target}").as_bytes())));
            continue;
        }
        if ft.is_dir() {
            // Skip build outputs and hidden directories.
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect(root, &path, out)?;
        } else {
            let rel = rel_path(root, &path);
            // A file that races away or is unreadable contributes a sentinel
            // rather than aborting the whole run; the path is still recorded so
            // its later (re)appearance changes the digest.
            let digest = hash_file(&path).unwrap_or([0u8; 32]);
            out.push((rel, digest));
        }
    }
    Ok(())
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Combine per-crate own-digests into Merkle fingerprints over the dependency
/// closure. Pure (no I/O) so it is exhaustively testable.
///
/// `domain` is a **domain-separation prefix** mixed into every digest — it
/// encodes anything global that affects compilation (e.g. the `rustc -vV`
/// string and the lockfile). It is *not* a cryptographic salt: it must be
/// deterministic, since the whole point is a reproducible content fingerprint.
/// Returns hex fingerprints keyed by crate name.
#[must_use]
pub fn merkle_fingerprints(
    graph: &WorkspaceGraph,
    own: &BTreeMap<String, Digest>,
    domain: &[u8],
) -> BTreeMap<String, String> {
    let mut memo: BTreeMap<String, Digest> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for m in &graph.members {
        let d = resolve(&m.name, graph, own, domain, &mut memo);
        out.insert(m.name.clone(), to_hex(&d));
    }
    out
}

fn resolve(
    name: &str,
    graph: &WorkspaceGraph,
    own: &BTreeMap<String, Digest>,
    domain: &[u8],
    memo: &mut BTreeMap<String, Digest>,
) -> Digest {
    if let Some(d) = memo.get(name) {
        return *d;
    }
    // Guard against cycles: insert a provisional zero so a cyclic edge resolves
    // to a stable value rather than recursing forever (Cargo forbids cycles,
    // but we stay total).
    memo.insert(name.to_string(), [0u8; 32]);

    let idx = graph.index_of(name);
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    if let Some(i) = idx {
        hasher.update(own.get(name).unwrap_or(&[0u8; 32]));
        let mut deps = graph.members[i].deps.clone();
        deps.sort();
        for dep in &deps {
            let dd = resolve(dep, graph, own, domain, memo);
            hasher.update(dep.as_bytes());
            hasher.update(&dd);
        }
    }
    let digest = *hasher.finalize().as_bytes();
    memo.insert(name.to_string(), digest);
    digest
}

/// Compute fingerprints for every crate in `graph` from the filesystem,
/// domain-separated by `rustc_version` (plus the lockfile and RUSTFLAGS).
///
/// # Errors
/// Propagates I/O errors reading crate sources.
pub fn compute(
    graph: &WorkspaceGraph,
    rustc_version: &str,
) -> io::Result<BTreeMap<String, String>> {
    // Hash each crate's tree in parallel — independent, IO-bound work. The
    // final Merkle fold is order-deterministic regardless of completion order.
    let own: BTreeMap<String, Digest> = std::thread::scope(|s| -> io::Result<_> {
        let handles: Vec<_> = graph
            .members
            .iter()
            .map(|m| s.spawn(|| crate_own_digest(&m.manifest_dir).map(|d| (m.name.clone(), d))))
            .collect();
        let mut own = BTreeMap::new();
        for h in handles {
            let (name, digest) = h.join().expect("fingerprint worker panicked")?;
            own.insert(name, digest);
        }
        Ok(own)
    })?;
    // Fold the workspace lockfile into the global domain prefix: a
    // dependency-version change (e.g. `cargo update`) alters no crate's own
    // sources but can change any crate's test outcome, so it must invalidate
    // every fingerprint. A missing lockfile (rare for an app, normal for a
    // library) contributes a fixed sentinel rather than failing.
    let lock_digest = match std::fs::read(graph.root.join("Cargo.lock")) {
        Ok(bytes) => hash_bytes(&bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => [0u8; 32],
        Err(e) => return Err(e),
    };
    // Compilation flags change codegen without touching any source. Fold the
    // ambient RUSTFLAGS (both spellings) into the domain prefix so a flags
    // change invalidates the cache.
    let rustflags = std::env::var("CARGO_ENCODED_RUSTFLAGS")
        .or_else(|_| std::env::var("RUSTFLAGS"))
        .unwrap_or_default();
    let mut domain = Vec::with_capacity(rustc_version.len() + 64);
    domain.extend_from_slice(rustc_version.as_bytes());
    domain.extend_from_slice(&lock_digest);
    domain.push(0);
    domain.extend_from_slice(rustflags.as_bytes());
    Ok(merkle_fingerprints(graph, &own, &domain))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Member;
    use std::path::PathBuf;

    fn graph() -> WorkspaceGraph {
        // util <- core <- cli  (cli depends on core depends on util)
        WorkspaceGraph::new(
            PathBuf::from("/ws"),
            vec![
                Member {
                    name: "util".into(),
                    manifest_dir: "/ws/util".into(),
                    deps: vec![],
                },
                Member {
                    name: "core".into(),
                    manifest_dir: "/ws/core".into(),
                    deps: vec!["util".into()],
                },
                Member {
                    name: "cli".into(),
                    manifest_dir: "/ws/cli".into(),
                    deps: vec!["core".into()],
                },
            ],
        )
    }

    fn digests(util: u8, core: u8, cli: u8) -> BTreeMap<String, Digest> {
        BTreeMap::from([
            ("util".to_string(), [util; 32]),
            ("core".to_string(), [core; 32]),
            ("cli".to_string(), [cli; 32]),
        ])
    }

    #[test]
    fn changing_a_leaf_changes_all_dependents() {
        let g = graph();
        let base = merkle_fingerprints(&g, &digests(1, 1, 1), b"rustc-x");
        let leaf_changed = merkle_fingerprints(&g, &digests(2, 1, 1), b"rustc-x");
        // util changed -> util, core, cli all differ.
        assert_ne!(base["util"], leaf_changed["util"]);
        assert_ne!(base["core"], leaf_changed["core"]);
        assert_ne!(base["cli"], leaf_changed["cli"]);
    }

    #[test]
    fn changing_top_changes_only_top() {
        let g = graph();
        let base = merkle_fingerprints(&g, &digests(1, 1, 1), b"rustc-x");
        let top_changed = merkle_fingerprints(&g, &digests(1, 1, 2), b"rustc-x");
        assert_eq!(base["util"], top_changed["util"]);
        assert_eq!(base["core"], top_changed["core"]);
        assert_ne!(base["cli"], top_changed["cli"]);
    }

    #[test]
    fn compiler_change_invalidates_everything() {
        let g = graph();
        let a = merkle_fingerprints(&g, &digests(1, 1, 1), b"rustc-1.96");
        let b = merkle_fingerprints(&g, &digests(1, 1, 1), b"rustc-1.97");
        assert_ne!(a["util"], b["util"]);
        assert_ne!(a["cli"], b["cli"]);
    }

    #[test]
    fn deterministic() {
        let g = graph();
        let a = merkle_fingerprints(&g, &digests(5, 6, 7), b"s");
        let b = merkle_fingerprints(&g, &digests(5, 6, 7), b"s");
        assert_eq!(a, b);
    }

    #[test]
    fn own_digest_reads_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), b"[package]\nname=\"x\"").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), b"pub fn a() {}").unwrap();
        let d1 = crate_own_digest(dir.path()).unwrap();
        // Editing a source file changes the digest.
        std::fs::write(dir.path().join("src/lib.rs"), b"pub fn b() {}").unwrap();
        let d2 = crate_own_digest(dir.path()).unwrap();
        assert_ne!(d1, d2);
        // A nested target/ dir is ignored.
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/junk.rs"), b"ignored").unwrap();
        let d3 = crate_own_digest(dir.path()).unwrap();
        assert_eq!(d2, d3);
        // A non-.rs asset (e.g. something `include_str!`-ed) is hashed too.
        std::fs::write(dir.path().join("src/template.html"), b"<p>v1</p>").unwrap();
        let d4 = crate_own_digest(dir.path()).unwrap();
        assert_ne!(d3, d4);
        std::fs::write(dir.path().join("src/template.html"), b"<p>v2</p>").unwrap();
        let d5 = crate_own_digest(dir.path()).unwrap();
        assert_ne!(d4, d5);
    }

    #[test]
    fn lockfile_change_invalidates_all_fingerprints() {
        let ws = tempfile::tempdir().unwrap();
        let croot = ws.path().join("c");
        std::fs::create_dir_all(croot.join("src")).unwrap();
        std::fs::write(croot.join("Cargo.toml"), b"[package]\nname=\"c\"").unwrap();
        std::fs::write(croot.join("src/lib.rs"), b"pub fn a() {}").unwrap();
        std::fs::write(ws.path().join("Cargo.lock"), b"# lock v1").unwrap();

        let g = WorkspaceGraph::new(
            ws.path().to_path_buf(),
            vec![Member {
                name: "c".into(),
                manifest_dir: croot.clone(),
                deps: vec![],
            }],
        );
        let before = compute(&g, "rustc-x").unwrap();
        // `cargo update` rewrites only the lockfile — no crate source changes.
        std::fs::write(ws.path().join("Cargo.lock"), b"# lock v2").unwrap();
        let after = compute(&g, "rustc-x").unwrap();
        assert_ne!(before["c"], after["c"]);
    }
}
