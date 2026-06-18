//! Per-session git worktree management.
//!
//! Each agent session runs in its own worktree (pillar 2's substrate): this
//! cleanly attributes file changes, prevents parallel agents from stomping each
//! other, and gives a stable place to compute the session's diff. M1 is
//! single-agent, but the isolation is built in from the start.
//!
//! **All methods block** (they call into libgit2). The async engine must invoke
//! them via [`tokio::task::spawn_blocking`] rather than directly on the reactor.
//! [`WorktreeManager`] is `Clone` + `Send` + `Sync` (it holds only paths), so it
//! moves cheaply into a blocking closure. Do not cache a `git2::Repository` on
//! the struct — `Repository` is not `Send` and would break that contract.

use std::path::{Path, PathBuf};

use git2::{Delta, DiffFindOptions, DiffFormat, DiffOptions, Repository};
use serde::{Deserialize, Serialize};

/// Errors from worktree operations.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// `session_id` failed validation (must be `[A-Za-z0-9_-]{1,64}`). This is
    /// the boundary that prevents path traversal and git-ref injection.
    #[error("invalid session id: must be 1-64 chars of [A-Za-z0-9_-]")]
    InvalidSessionId,
    /// A libgit2 operation failed.
    #[error(transparent)]
    Git(#[from] git2::Error),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for worktree operations.
pub type Result<T> = std::result::Result<T, WorktreeError>;

/// Per-file change status in a [`WorktreeDiff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// File is new in the worktree.
    Added,
    /// Tracked file's contents changed.
    Modified,
    /// Tracked file was removed.
    Deleted,
    /// File was renamed (see [`FileDiff::old_path`]).
    Renamed,
    /// Any other delta (copied, typechange, …).
    Other,
}

impl From<Delta> for FileStatus {
    fn from(d: Delta) -> Self {
        match d {
            Delta::Added | Delta::Untracked => FileStatus::Added,
            Delta::Modified => FileStatus::Modified,
            Delta::Deleted => FileStatus::Deleted,
            Delta::Renamed => FileStatus::Renamed,
            _ => FileStatus::Other,
        }
    }
}

/// One file's change within a worktree diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Current path (or the deleted file's path).
    pub path: String,
    /// Previous path, set only when [`status`](FileDiff::status) is
    /// [`Renamed`](FileStatus::Renamed).
    pub old_path: Option<String>,
    /// What happened to the file.
    pub status: FileStatus,
    /// The unified patch text for just this file (with hunks). Lossy for
    /// non-UTF-8 content.
    pub patch: String,
}

/// A structured diff of a worktree against its `HEAD` commit.
///
/// Structured per-file (not a flat string) so the review UI (pillar 2) can show
/// a file list, per-file diffs, and — in M2 — cross-worktree comparison without
/// re-parsing a patch blob.
///
/// **Sensitive:** patches contain verbatim file contents and may include
/// secrets the agent wrote. Do not log or telemeter without redaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeDiff {
    /// Changed files, in libgit2 delta order.
    pub files: Vec<FileDiff>,
}

impl WorktreeDiff {
    /// Whether there are no changes.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Number of changed files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The concatenated unified patch across all files (for plain-text display).
    pub fn raw(&self) -> String {
        self.files.iter().map(|f| f.patch.as_str()).collect()
    }
}

/// Validate a session id used in filesystem paths and git ref names. Strict
/// allowlist: rejects `/`, `\`, `.`, NUL, whitespace, and anything that could
/// escape the worktree base or inject into a ref.
fn validate_session_id(session_id: &str) -> Result<()> {
    let ok = (1..=64).contains(&session_id.len())
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(WorktreeError::InvalidSessionId)
    }
}

/// Creates and inspects one git worktree per agent session.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    repo_path: PathBuf,
    worktree_base: PathBuf,
}

impl WorktreeManager {
    /// `repo_path` is the repository to branch from; each session's worktree is
    /// created under `worktree_base`.
    pub fn new(repo_path: impl Into<PathBuf>, worktree_base: impl Into<PathBuf>) -> Self {
        Self {
            repo_path: repo_path.into(),
            worktree_base: worktree_base.into(),
        }
    }

    fn branch_name(session_id: &str) -> String {
        format!("oryn-{session_id}")
    }

    /// Create a worktree (and a branch `oryn-<session_id>`) for the session,
    /// returning the worktree path. Rejects invalid `session_id`s.
    pub fn create(&self, session_id: &str) -> Result<PathBuf> {
        validate_session_id(session_id)?;
        std::fs::create_dir_all(&self.worktree_base)?;
        let repo = Repository::open(&self.repo_path)?;
        let path = self.worktree_base.join(session_id);
        repo.worktree(&Self::branch_name(session_id), &path, None)?;
        Ok(path)
    }

    /// Structured diff of all changes in the worktree relative to its `HEAD`
    /// commit — new (with content), modified, deleted, and renamed files.
    ///
    /// Computed `HEAD`-tree → workdir directly: the git index is never touched,
    /// so the agent's own staging state is irrelevant and preserved.
    pub fn diff(&self, worktree_path: &Path) -> Result<WorktreeDiff> {
        let repo = Repository::open(worktree_path)?;
        let head_tree = repo.head()?.peel_to_tree()?;

        let mut opts = DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let mut diff = repo.diff_tree_to_workdir(Some(&head_tree), Some(&mut opts))?;

        // Detect renames so status/old_path are accurate. `for_untracked` is
        // required because the rename *target* is an untracked new file here.
        let mut find = DiffFindOptions::new();
        find.renames(true).for_untracked(true);
        diff.find_similar(Some(&mut find))?;

        let mut files = Vec::with_capacity(diff.deltas().len());
        for (idx, delta) in diff.deltas().enumerate() {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let status = FileStatus::from(delta.status());
            let old_path = if status == FileStatus::Renamed {
                delta
                    .old_file()
                    .path()
                    .map(|p| p.to_string_lossy().into_owned())
            } else {
                None
            };
            let patch = match git2::Patch::from_diff(&diff, idx)? {
                Some(mut p) => {
                    let buf = p.to_buf()?;
                    String::from_utf8_lossy(&buf).into_owned()
                }
                None => String::new(),
            };
            files.push(FileDiff { path, old_path, status, patch });
        }
        Ok(WorktreeDiff { files })
    }

    /// Plain-text unified patch of the worktree (convenience over
    /// [`diff`](Self::diff) for callers that only want a string).
    pub fn diff_text(&self, worktree_path: &Path) -> Result<String> {
        let repo = Repository::open(worktree_path)?;
        let head_tree = repo.head()?.peel_to_tree()?;
        let mut opts = DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let diff = repo.diff_tree_to_workdir(Some(&head_tree), Some(&mut opts))?;

        let mut buf = String::new();
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let origin = line.origin();
            if matches!(origin, '+' | '-' | ' ') {
                buf.push(origin);
            }
            buf.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })?;
        Ok(buf)
    }

    /// Tear down the session's worktree: prune its git registration and remove
    /// its directory. Idempotent — removing a never-created session is `Ok`.
    pub fn remove(&self, session_id: &str) -> Result<()> {
        validate_session_id(session_id)?;
        let repo = Repository::open(&self.repo_path)?;
        if let Ok(worktree) = repo.find_worktree(&Self::branch_name(session_id)) {
            let mut opts = git2::WorktreePruneOptions::new();
            // `valid(true)` allows pruning a still-valid worktree; `working_tree`
            // also deletes its files.
            opts.valid(true).working_tree(true);
            worktree.prune(Some(&mut opts))?;
        }
        let path = self.worktree_base.join(session_id);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init_repo_with_commit(path: &Path) {
        let repo = Repository::init(path).unwrap();
        fs::write(path.join("README.md"), "seed\n").unwrap();
        fs::write(path.join(".gitignore"), "ignored.txt\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.add_path(Path::new(".gitignore")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[]).unwrap();
    }

    struct Fixture {
        _repo: tempfile::TempDir,
        _base: tempfile::TempDir,
        mgr: WorktreeManager,
    }

    fn fixture() -> Fixture {
        let repo = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        init_repo_with_commit(repo.path());
        let mgr = WorktreeManager::new(repo.path(), base.path());
        Fixture { _repo: repo, _base: base, mgr }
    }

    fn file<'a>(d: &'a WorktreeDiff, name: &str) -> &'a FileDiff {
        d.files
            .iter()
            .find(|f| f.path == name)
            .unwrap_or_else(|| panic!("no file {name} in diff: {d:?}"))
    }

    #[test]
    fn create_then_diff_sees_new_file_as_added_with_content() {
        let fx = fixture();
        let wt = fx.mgr.create("sess1").unwrap();
        assert!(wt.exists());
        fs::write(wt.join("new.txt"), "agent wrote this\n").unwrap();

        let diff = fx.mgr.diff(&wt).unwrap();
        let f = file(&diff, "new.txt");
        assert_eq!(f.status, FileStatus::Added);
        assert!(f.patch.contains("agent wrote this"));
        assert!(diff.raw().contains("new.txt"));
        assert_eq!(diff.file_count(), 1);
    }

    #[test]
    fn diff_sees_modification() {
        let fx = fixture();
        let wt = fx.mgr.create("sess2").unwrap();
        fs::write(wt.join("README.md"), "seed\nmore\n").unwrap();
        let diff = fx.mgr.diff(&wt).unwrap();
        let f = file(&diff, "README.md");
        assert_eq!(f.status, FileStatus::Modified);
        assert!(f.patch.contains("+more"));
    }

    #[test]
    fn diff_sees_deletion() {
        let fx = fixture();
        let wt = fx.mgr.create("sess3").unwrap();
        fs::remove_file(wt.join("README.md")).unwrap();
        let diff = fx.mgr.diff(&wt).unwrap();
        let f = file(&diff, "README.md");
        assert_eq!(f.status, FileStatus::Deleted);
    }

    #[test]
    fn diff_detects_rename() {
        let fx = fixture();
        let wt = fx.mgr.create("sess_ren").unwrap();
        fs::rename(wt.join("README.md"), wt.join("READYOU.md")).unwrap();
        let diff = fx.mgr.diff(&wt).unwrap();
        let f = file(&diff, "READYOU.md");
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("README.md"));
    }

    #[test]
    fn gitignored_files_are_excluded() {
        let fx = fixture();
        let wt = fx.mgr.create("sess_ign").unwrap();
        fs::write(wt.join("ignored.txt"), "secret\n").unwrap();
        let diff = fx.mgr.diff(&wt).unwrap();
        assert!(
            diff.files.iter().all(|f| f.path != "ignored.txt"),
            "ignored file must not appear: {diff:?}"
        );
    }

    #[test]
    fn diff_is_empty_with_no_changes() {
        let fx = fixture();
        let wt = fx.mgr.create("sess4").unwrap();
        let diff = fx.mgr.diff(&wt).unwrap();
        assert!(diff.is_empty());
        assert_eq!(diff.raw(), "");
    }

    #[test]
    fn diff_text_matches_structured_content() {
        let fx = fixture();
        let wt = fx.mgr.create("sess_txt").unwrap();
        fs::write(wt.join("new.txt"), "hello\n").unwrap();
        let text = fx.mgr.diff_text(&wt).unwrap();
        assert!(text.contains("new.txt"));
        assert!(text.contains("hello"));
    }

    #[test]
    fn diff_does_not_touch_agent_staging() {
        // The agent stages a file; diff() must leave the on-disk index intact.
        let fx = fixture();
        let wt = fx.mgr.create("sess_idx").unwrap();
        fs::write(wt.join("staged.txt"), "x\n").unwrap();
        let repo = Repository::open(&wt).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged.txt")).unwrap();
        index.write().unwrap();
        drop(index);

        let _ = fx.mgr.diff(&wt).unwrap();

        let repo = Repository::open(&wt).unwrap();
        let index = repo.index().unwrap();
        assert!(
            index.get_path(Path::new("staged.txt"), 0).is_some(),
            "agent's staged file must still be in the index"
        );
    }

    #[test]
    fn remove_is_idempotent_and_cleans_directory() {
        let fx = fixture();
        let wt = fx.mgr.create("sess5").unwrap();
        assert!(wt.exists());
        fx.mgr.remove("sess5").unwrap();
        assert!(!wt.exists());
        fx.mgr.remove("sess5").unwrap(); // again: no error
        fx.mgr.remove("never").unwrap(); // never created: no error
    }

    #[test]
    fn rejects_invalid_session_ids() {
        let fx = fixture();
        for bad in ["", "../escape", "a/b", "a.b", "with space", "nul\0byte"] {
            assert!(
                matches!(fx.mgr.create(bad), Err(WorktreeError::InvalidSessionId)),
                "create must reject {bad:?}"
            );
            assert!(
                matches!(fx.mgr.remove(bad), Err(WorktreeError::InvalidSessionId)),
                "remove must reject {bad:?}"
            );
        }
        // Absolute path and 65 chars also rejected.
        assert!(matches!(fx.mgr.create("/abs"), Err(WorktreeError::InvalidSessionId)));
        let too_long = "a".repeat(65);
        assert!(matches!(fx.mgr.create(&too_long), Err(WorktreeError::InvalidSessionId)));
    }

    #[test]
    fn accepts_valid_session_ids() {
        let fx = fixture();
        // Boundary: 64 chars, mixed allowed set.
        let id = format!("{}-_AZ09", "a".repeat(58));
        assert_eq!(id.len(), 64);
        assert!(fx.mgr.create(&id).is_ok());
    }

    #[test]
    fn create_on_non_repo_is_git_error() {
        let not_repo = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let mgr = WorktreeManager::new(not_repo.path(), base.path());
        assert!(matches!(mgr.create("x").unwrap_err(), WorktreeError::Git(_)));
    }

    #[test]
    fn diff_on_non_repo_is_git_error() {
        let not_repo = tempfile::tempdir().unwrap();
        let mgr = WorktreeManager::new(not_repo.path(), not_repo.path());
        assert!(matches!(mgr.diff(not_repo.path()).unwrap_err(), WorktreeError::Git(_)));
        assert!(matches!(mgr.diff_text(not_repo.path()).unwrap_err(), WorktreeError::Git(_)));
    }

    #[test]
    fn create_with_unusable_base_is_io_error() {
        let repo = tempfile::tempdir().unwrap();
        init_repo_with_commit(repo.path());
        let file = tempfile::NamedTempFile::new().unwrap();
        let bogus_base = file.path().join("cannot/exist");
        let mgr = WorktreeManager::new(repo.path(), bogus_base);
        assert!(matches!(mgr.create("x").unwrap_err(), WorktreeError::Io(_)));
    }

    #[test]
    fn errors_display_nonempty() {
        assert!(!WorktreeError::InvalidSessionId.to_string().is_empty());
        let not_repo = tempfile::tempdir().unwrap();
        let mgr = WorktreeManager::new(not_repo.path(), not_repo.path());
        assert!(!mgr.create("x").unwrap_err().to_string().is_empty());
    }
}
