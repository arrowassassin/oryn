//! Cache-stable prompt prefix — the byte-identical, append-only region that
//! every model provider's prompt cache must hit on every subtask completion.
//!
//! # Invariants
//!
//! 1. **Append-only, never edited mid-mission.** Once a [`CacheStablePrefix`]
//!    is built for a mission its three blocks (`system`, `repo_map`, `task`)
//!    are immutable. If the mission description changes, build a *new* prefix —
//!    the old one stays valid for any provider that already cached it.
//!
//! 2. **Byte-identical for identical inputs.** [`CacheStablePrefix::render`]
//!    joins blocks with the exact separator `"\n\n"` in the fixed order
//!    `system → repo_map → task`. [`repo_map_from`] sorts its input paths
//!    before joining so the result is independent of input order.
//!
//! 3. **Volatile text stays out.** Per-subtask instructions live in
//!    [`crate::orchestrator::provider::CompletionRequest::suffix`], never here.
//!    The cache-control breakpoint is conceptually at the end of this prefix.

use crate::ids::ArtifactId;

/// The fixed separator between prefix blocks.
const SEP: &str = "\n\n";

/// A byte-identical, append-only prompt prefix.
///
/// Built via [`CacheStablePrefixBuilder`] (returned by
/// [`CacheStablePrefix::builder`]). Once constructed the struct is immutable —
/// see the [module-level invariants](self).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheStablePrefix {
    system: String,
    repo_map: String,
    task: String,
}

impl CacheStablePrefix {
    /// Return a fresh builder.
    pub fn builder() -> CacheStablePrefixBuilder {
        CacheStablePrefixBuilder::default()
    }

    /// Render the full cache-stable region as a single `String`.
    ///
    /// The result is `system + SEP + repo_map + SEP + task` with `SEP = "\n\n"`.
    /// Identical inputs always produce byte-identical output (deterministic).
    pub fn render(&self) -> String {
        format!("{}{SEP}{}{SEP}{}", self.system, self.repo_map, self.task)
    }

    /// The content-address handle for this prefix (SHA-256 of `render()`).
    ///
    /// Two prefixes with the same content have the same handle; two with
    /// different content have different handles with overwhelming probability.
    pub fn handle(&self) -> ArtifactId {
        ArtifactId::from_content(self.render().as_bytes())
    }
}

/// Incremental builder for [`CacheStablePrefix`].
///
/// Each block may be set at most once; calling the same setter twice panics,
/// which surfaces bugs in calling code during development.
#[derive(Debug, Default)]
pub struct CacheStablePrefixBuilder {
    system: Option<String>,
    repo_map: Option<String>,
    task: Option<String>,
}

impl CacheStablePrefixBuilder {
    /// Set the system block (global instructions, persona, capabilities).
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same builder.
    pub fn system(mut self, system: impl Into<String>) -> Self {
        assert!(
            self.system.is_none(),
            "CacheStablePrefixBuilder: `system` already set"
        );
        self.system = Some(system.into());
        self
    }

    /// Set the repo-map block.
    ///
    /// Prefer [`repo_map_from`] to produce a byte-stable, sorted list of paths.
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same builder.
    pub fn repo_map(mut self, repo_map: impl Into<String>) -> Self {
        assert!(
            self.repo_map.is_none(),
            "CacheStablePrefixBuilder: `repo_map` already set"
        );
        self.repo_map = Some(repo_map.into());
        self
    }

    /// Set the task block (high-level mission description).
    ///
    /// # Panics
    ///
    /// Panics if called more than once on the same builder.
    pub fn task(mut self, task: impl Into<String>) -> Self {
        assert!(
            self.task.is_none(),
            "CacheStablePrefixBuilder: `task` already set"
        );
        self.task = Some(task.into());
        self
    }

    /// Consume the builder and return the finished prefix.
    ///
    /// Missing blocks default to an empty string so callers that genuinely
    /// have nothing to say for a block need not invent placeholder content.
    pub fn build(self) -> CacheStablePrefix {
        CacheStablePrefix {
            system: self.system.unwrap_or_default(),
            repo_map: self.repo_map.unwrap_or_default(),
            task: self.task.unwrap_or_default(),
        }
    }
}

/// Produce a byte-stable repo-map string from an unordered slice of paths.
///
/// Paths are sorted (byte order, i.e. [`str::cmp`]) before joining with `"\n"`
/// so that the output is independent of the order they were discovered —
/// directory traversal order varies across OSes and filesystems.
///
/// # Example
///
/// ```
/// # use oryn_core::orchestrator::prefix::repo_map_from;
/// let map = repo_map_from(&["src/b.rs".into(), "src/a.rs".into()]);
/// assert_eq!(map, "src/a.rs\nsrc/b.rs");
/// ```
pub fn repo_map_from(paths: &[String]) -> String {
    let mut sorted = paths.to_vec();
    sorted.sort();
    sorted.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helper ────────────────────────────────────────────────────────────────

    fn sample_prefix() -> CacheStablePrefix {
        CacheStablePrefix::builder()
            .system("You are a senior Rust engineer.")
            .repo_map(repo_map_from(&[
                "src/main.rs".into(),
                "src/lib.rs".into(),
            ]))
            .task("Refactor the event loop to use async/await.")
            .build()
    }

    // ── render determinism ────────────────────────────────────────────────────

    #[test]
    fn render_fixed_order_and_separator() {
        let prefix = CacheStablePrefix::builder()
            .system("SYS")
            .repo_map("MAP")
            .task("TSK")
            .build();
        assert_eq!(prefix.render(), "SYS\n\nMAP\n\nTSK");
    }

    #[test]
    fn identical_inputs_produce_identical_render() {
        let a = sample_prefix();
        let b = sample_prefix();
        assert_eq!(a.render(), b.render());
    }

    #[test]
    fn render_is_idempotent() {
        let prefix = sample_prefix();
        let first = prefix.render();
        let second = prefix.render();
        assert_eq!(first, second);
    }

    #[test]
    fn render_bytes_are_identical_for_identical_inputs() {
        let a = sample_prefix();
        let b = sample_prefix();
        assert_eq!(a.render().into_bytes(), b.render().into_bytes());
    }

    // ── handle ────────────────────────────────────────────────────────────────

    #[test]
    fn identical_inputs_produce_identical_handle() {
        let a = sample_prefix();
        let b = sample_prefix();
        assert_eq!(a.handle(), b.handle());
    }

    #[test]
    fn different_inputs_produce_different_handle() {
        let a = CacheStablePrefix::builder()
            .system("Alice")
            .repo_map("")
            .task("foo")
            .build();
        let b = CacheStablePrefix::builder()
            .system("Bob")
            .repo_map("")
            .task("foo")
            .build();
        assert_ne!(a.handle(), b.handle());
    }

    #[test]
    fn handle_is_artifact_id_of_rendered_bytes() {
        let prefix = sample_prefix();
        let expected = ArtifactId::from_content(prefix.render().as_bytes());
        assert_eq!(prefix.handle(), expected);
    }

    // ── repo_map_from ────────────────────────────────────────────────────────

    #[test]
    fn repo_map_from_sorts_paths() {
        let map = repo_map_from(&[
            "src/z.rs".into(),
            "src/a.rs".into(),
            "src/m.rs".into(),
        ]);
        assert_eq!(map, "src/a.rs\nsrc/m.rs\nsrc/z.rs");
    }

    #[test]
    fn repo_map_from_is_order_independent() {
        let ordered = repo_map_from(&["b".into(), "a".into(), "c".into()]);
        let reversed = repo_map_from(&["c".into(), "b".into(), "a".into()]);
        assert_eq!(ordered, reversed);
    }

    #[test]
    fn repo_map_from_byte_stable_against_shuffled_input() {
        let inputs: &[&[&str]] = &[
            &["src/main.rs", "src/lib.rs", "tests/mod.rs"],
            &["tests/mod.rs", "src/lib.rs", "src/main.rs"],
            &["src/lib.rs", "tests/mod.rs", "src/main.rs"],
        ];
        let maps: Vec<String> = inputs
            .iter()
            .map(|paths| {
                let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
                repo_map_from(&owned)
            })
            .collect();
        assert!(maps.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn repo_map_from_empty_slice_is_empty_string() {
        assert_eq!(repo_map_from(&[]), "");
    }

    #[test]
    fn repo_map_from_single_path_no_newline() {
        assert_eq!(repo_map_from(&["src/lib.rs".into()]), "src/lib.rs");
    }

    // ── builder guards ────────────────────────────────────────────────────────

    #[test]
    #[should_panic(expected = "`system` already set")]
    fn builder_panics_on_double_system() {
        let _ = CacheStablePrefix::builder()
            .system("first")
            .system("second");
    }

    #[test]
    #[should_panic(expected = "`repo_map` already set")]
    fn builder_panics_on_double_repo_map() {
        let _ = CacheStablePrefix::builder()
            .repo_map("first")
            .repo_map("second");
    }

    #[test]
    #[should_panic(expected = "`task` already set")]
    fn builder_panics_on_double_task() {
        let _ = CacheStablePrefix::builder()
            .task("first")
            .task("second");
    }

    #[test]
    fn builder_defaults_missing_blocks_to_empty() {
        let prefix = CacheStablePrefix::builder().build();
        assert_eq!(prefix.render(), "\n\n\n\n");
    }

    // ── trait impls ───────────────────────────────────────────────────────────

    #[test]
    fn prefix_clone_equals_original() {
        let a = sample_prefix();
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn prefix_debug_contains_field_names() {
        let prefix = sample_prefix();
        let debug = format!("{prefix:?}");
        assert!(debug.contains("system"));
        assert!(debug.contains("repo_map"));
        assert!(debug.contains("task"));
    }
}
