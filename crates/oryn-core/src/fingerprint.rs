//! Content fingerprinting of a crate and its dependency closure.
//!
//! A crate's test outcome is a pure function of: its own sources, the sources of
//! every crate it (transitively) depends on, and the compiler. We capture that
//! with a **Merkle fingerprint**: `fp(c) = H(salt ‖ own(c) ‖ fp(d₁) ‖ … ‖ fp(dₙ))`
//! over the crate's workspace dependencies `dᵢ`.
//!
//! If — and only if — a crate or anything in its dependency closure changes, its
//! fingerprint changes. That is exactly the soundness condition for **caching
//! test results**: a crate whose fingerprint matches a previously-recorded green
//! run cannot have a different test outcome (modulo flakiness, handled
//! separately), so its tests can be skipped.

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

/// Compute the digest of a crate's *own* tracked source files: every `*.rs`
/// file plus `Cargo.toml` and `build.rs` under `manifest_dir`, excluding any
/// nested `target/` directory and hidden directories. Files are hashed in a
/// path-sorted, deterministic order.
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

fn is_source(name: &str) -> bool {
    name.ends_with(".rs") || name == "Cargo.toml"
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Digest)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            // Skip build outputs and hidden directories.
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect(root, &path, out)?;
        } else if is_source(&name) {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, hash_file(&path)?));
        }
    }
    Ok(())
}

/// Combine per-crate own-digests into Merkle fingerprints over the dependency
/// closure. Pure (no I/O) so it is exhaustively testable.
///
/// `salt` should encode anything global that affects compilation (e.g. the
/// `rustc -vV` string). Returns hex fingerprints keyed by crate name.
#[must_use]
pub fn merkle_fingerprints(
    graph: &WorkspaceGraph,
    own: &BTreeMap<String, Digest>,
    salt: &[u8],
) -> BTreeMap<String, String> {
    let mut memo: BTreeMap<String, Digest> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for m in &graph.members {
        let d = resolve(&m.name, graph, own, salt, &mut memo);
        out.insert(m.name.clone(), to_hex(&d));
    }
    out
}

fn resolve(
    name: &str,
    graph: &WorkspaceGraph,
    own: &BTreeMap<String, Digest>,
    salt: &[u8],
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
    hasher.update(salt);
    if let Some(i) = idx {
        hasher.update(own.get(name).unwrap_or(&[0u8; 32]));
        let mut deps = graph.members[i].deps.clone();
        deps.sort();
        for dep in &deps {
            let dd = resolve(dep, graph, own, salt, memo);
            hasher.update(dep.as_bytes());
            hasher.update(&dd);
        }
    }
    let digest = *hasher.finalize().as_bytes();
    memo.insert(name.to_string(), digest);
    digest
}

/// Compute fingerprints for every crate in `graph` from the filesystem, salted
/// with `rustc_version`.
///
/// # Errors
/// Propagates I/O errors reading crate sources.
pub fn compute(
    graph: &WorkspaceGraph,
    rustc_version: &str,
) -> io::Result<BTreeMap<String, String>> {
    let mut own = BTreeMap::new();
    for m in &graph.members {
        own.insert(m.name.clone(), crate_own_digest(&m.manifest_dir)?);
    }
    Ok(merkle_fingerprints(graph, &own, rustc_version.as_bytes()))
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
    }
}
