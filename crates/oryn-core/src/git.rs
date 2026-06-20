//! Detect changed files via git.

use crate::{OrynError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    if !out.status.success() {
        return Err(OrynError::Process(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Absolute path of the git repository root containing `dir`.
///
/// # Errors
/// Fails if `dir` is not inside a git repository.
pub fn repo_root(dir: &Path) -> Result<PathBuf> {
    let out = run(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Files changed relative to `since` (a git ref/commit). If `since` is `None`,
/// reports the *working-tree* changes: tracked modifications versus `HEAD` plus
/// untracked-but-not-ignored files. Returned paths are relative to the repo root.
///
/// # Errors
/// Fails if the underlying git commands fail.
pub fn changed_files(dir: &Path, since: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut files: Vec<String> = Vec::new();
    match since {
        Some(reference) => {
            // Changes between `since` and the current working tree.
            // `--end-of-options` + `--` stop a ref that begins with `-` from
            // being parsed as a git option (argument/option injection).
            let diff = run(
                dir,
                &["diff", "--name-only", "--end-of-options", reference, "--"],
            )?;
            files.extend(diff.lines().map(str::to_string));
        }
        None => {
            let tracked = run(dir, &["diff", "--name-only", "HEAD"])?;
            files.extend(tracked.lines().map(str::to_string));
            let untracked = run(dir, &["ls-files", "--others", "--exclude-standard"])?;
            files.extend(untracked.lines().map(str::to_string));
        }
    }
    files.retain(|f| !f.trim().is_empty());
    files.sort();
    files.dedup();
    Ok(files.into_iter().map(PathBuf::from).collect())
}
