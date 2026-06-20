//! Load the workspace crate graph from `cargo metadata`.

use crate::graph::{Member, WorkspaceGraph};
use crate::{OrynError, Result};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    workspace_members: Vec<String>,
    workspace_root: String,
}

#[derive(Deserialize)]
struct RawPackage {
    id: String,
    name: String,
    manifest_path: String,
    #[serde(default)]
    dependencies: Vec<RawDep>,
}

#[derive(Deserialize)]
struct RawDep {
    name: String,
}

/// Run `cargo metadata` in `dir` and build the [`WorkspaceGraph`].
///
/// Uses `--no-deps` so only workspace members are returned; intra-workspace
/// dependency edges are recovered by matching dependency names against member
/// names.
///
/// # Errors
/// Fails if `cargo metadata` cannot run or its output cannot be parsed.
pub fn load(dir: &Path) -> Result<WorkspaceGraph> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(dir)
        .output()?;
    if !output.status.success() {
        return Err(OrynError::Process(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse(&output.stdout)
}

/// Parse raw `cargo metadata` JSON into a [`WorkspaceGraph`] (separated for
/// testability).
fn parse(json: &[u8]) -> Result<WorkspaceGraph> {
    let raw: RawMetadata = serde_json::from_slice(json)?;
    let member_ids: BTreeSet<&str> = raw.workspace_members.iter().map(String::as_str).collect();
    let names: BTreeSet<&str> = raw
        .packages
        .iter()
        .filter(|p| member_ids.contains(p.id.as_str()))
        .map(|p| p.name.as_str())
        .collect();

    let mut members = Vec::new();
    for p in &raw.packages {
        if !member_ids.contains(p.id.as_str()) {
            continue;
        }
        let manifest_dir = PathBuf::from(&p.manifest_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut deps: Vec<String> = p
            .dependencies
            .iter()
            .filter(|d| names.contains(d.name.as_str()))
            .map(|d| d.name.clone())
            .collect();
        deps.sort();
        deps.dedup();
        members.push(Member {
            name: p.name.clone(),
            manifest_dir,
            deps,
        });
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(WorkspaceGraph::new(
        PathBuf::from(raw.workspace_root),
        members,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_members_and_intra_workspace_edges() {
        let json = br#"{
          "workspace_root": "/ws",
          "workspace_members": ["core 0.1.0 (path+file:///ws/crates/core)",
                                 "cli 0.1.0 (path+file:///ws/crates/cli)"],
          "packages": [
            {"id":"core 0.1.0 (path+file:///ws/crates/core)","name":"core",
             "manifest_path":"/ws/crates/core/Cargo.toml","dependencies":[{"name":"serde"}]},
            {"id":"cli 0.1.0 (path+file:///ws/crates/cli)","name":"cli",
             "manifest_path":"/ws/crates/cli/Cargo.toml",
             "dependencies":[{"name":"core"},{"name":"clap"}]}
          ]
        }"#;
        let g = parse(json).unwrap();
        assert_eq!(g.members.len(), 2);
        let cli = &g.members[g.index_of("cli").unwrap()];
        // External deps (clap) dropped; intra-workspace dep (core) kept.
        assert_eq!(cli.deps, vec!["core".to_string()]);
        assert_eq!(
            g.members[g.index_of("core").unwrap()].manifest_dir,
            PathBuf::from("/ws/crates/core")
        );
    }
}
