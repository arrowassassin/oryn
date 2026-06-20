//! Parse `git diff -U0` into per-file, **base-revision** (old-side) line hunks.
//!
//! Coverage is recorded against a base revision, so selection keys off *old*
//! line numbers. A hunk header `@@ -os,oc +ns,nc @@` gives `oc` old lines
//! starting at `os` (oc defaults to 1; `oc == 0` means a pure insertion after
//! old line `os`).

use crate::{OrynError, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// An old-side change region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// First affected old line (1-based).
    pub old_start: usize,
    /// Number of old lines affected; `0` for a pure insertion.
    pub old_count: usize,
}

impl Hunk {
    /// The set of old lines this hunk touches. A pure insertion touches the two
    /// lines straddling the insertion point (so the enclosing function is
    /// correctly flagged).
    #[must_use]
    pub fn touched_old_lines(&self) -> (usize, usize) {
        if self.old_count == 0 {
            (self.old_start, self.old_start + 1)
        } else {
            (self.old_start, self.old_start + self.old_count - 1)
        }
    }
}

/// Parse unified-diff text (produced with `-U0`) into per-file hunks.
#[must_use]
pub fn parse_unified_diff(diff: &str) -> BTreeMap<String, Vec<Hunk>> {
    let mut out: BTreeMap<String, Vec<Hunk>> = BTreeMap::new();
    let mut a_path: Option<String> = None;
    let mut file: Option<String> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            a_path = strip_ab(rest);
            file = None;
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            // Prefer the new path; for deletions (+++ /dev/null) use the old one.
            file = strip_ab(rest).or_else(|| a_path.clone());
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if let (Some(f), Some(h)) = (file.as_ref(), parse_hunk_header(rest)) {
                out.entry(f.clone()).or_default().push(h);
            }
        }
    }
    out
}

/// `a/foo/bar.rs` / `b/foo/bar.rs` -> `foo/bar.rs`; `/dev/null` -> None.
fn strip_ab(s: &str) -> Option<String> {
    let s = s.split('\t').next().unwrap_or(s).trim();
    if s == "/dev/null" {
        return None;
    }
    Some(
        s.strip_prefix("a/")
            .or_else(|| s.strip_prefix("b/"))
            .unwrap_or(s)
            .to_string(),
    )
}

/// Parse `-os[,oc] +ns[,nc] @@ ...` (the part after `@@ `).
fn parse_hunk_header(rest: &str) -> Option<Hunk> {
    let old = rest.split_whitespace().find(|t| t.starts_with('-'))?;
    let old = old.trim_start_matches('-');
    let mut it = old.split(',');
    let old_start: usize = it.next()?.parse().ok()?;
    let old_count: usize = match it.next() {
        Some(c) => c.parse().ok()?,
        None => 1,
    };
    Some(Hunk {
        old_start,
        old_count,
    })
}

/// Run `git diff -U0 <base>` in `dir` and parse the result.
///
/// # Errors
/// Fails if git cannot run.
pub fn changed_hunks(dir: &Path, base: &str) -> Result<BTreeMap<String, Vec<Hunk>>> {
    let out = Command::new("git")
        // `--no-renames`: a moved file becomes delete+add so its old-side lines
        // map to the base path coverage was recorded under (renames would key
        // hunks to the new path and silently miss the test). `--end-of-options`
        // guards a `base` that begins with `-`.
        .args([
            "diff",
            "-U0",
            "--no-renames",
            "--end-of-options",
            base,
            "--",
        ])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(OrynError::Process(format!(
            "git diff -U0 {base} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(parse_unified_diff(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\n\
--- a/src/lib.rs\n\
+++ b/src/lib.rs\n\
@@ -3 +3 @@\n\
-    let x = 1;\n\
+    let x = 2;\n\
@@ -10,0 +11,3 @@\n\
+    new();\n\
+    lines();\n\
+    here();\n\
@@ -20,2 +23,0 @@\n\
-    gone();\n\
-    too();\n";

    #[test]
    fn parses_modification_insertion_deletion() {
        let m = parse_unified_diff(DIFF);
        let hunks = &m["src/lib.rs"];
        assert_eq!(hunks.len(), 3);
        // modification of old line 3
        assert_eq!(
            hunks[0],
            Hunk {
                old_start: 3,
                old_count: 1
            }
        );
        // insertion after old line 10
        assert_eq!(
            hunks[1],
            Hunk {
                old_start: 10,
                old_count: 0
            }
        );
        // deletion of old lines 20..=21
        assert_eq!(
            hunks[2],
            Hunk {
                old_start: 20,
                old_count: 2
            }
        );
    }

    #[test]
    fn touched_lines() {
        assert_eq!(
            Hunk {
                old_start: 3,
                old_count: 1
            }
            .touched_old_lines(),
            (3, 3)
        );
        assert_eq!(
            Hunk {
                old_start: 10,
                old_count: 0
            }
            .touched_old_lines(),
            (10, 11)
        );
        assert_eq!(
            Hunk {
                old_start: 20,
                old_count: 2
            }
            .touched_old_lines(),
            (20, 21)
        );
    }

    #[test]
    fn new_file_uses_b_path() {
        let d = "--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n+a\n+b\n";
        let m = parse_unified_diff(d);
        assert!(m.contains_key("src/new.rs"));
    }
}
