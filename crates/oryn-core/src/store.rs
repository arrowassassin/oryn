//! Persistent per-project store: green-result cache + per-test history.
//!
//! Lives under `target/oryn/` by default. Two things:
//!
//! * **Green cache** — for each crate, the [`fingerprint`](crate::fingerprint)
//!   at which its test suite last passed cleanly. If a crate's current
//!   fingerprint matches, its tests are known-green and can be skipped (sound
//!   for deterministic tests; flaky tests are tracked separately and excluded).
//! * **Test history** — per-test pass/fail counts, recent outcomes, and last
//!   duration, powering statistical flaky scoring and fail-fast prioritization.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// How many recent outcomes to retain per test (ring buffer).
const RECENT_CAP: usize = 50;

/// A crate's last known-green fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateGreen {
    /// Merkle fingerprint of the crate's dependency closure at green time.
    pub fingerprint: String,
    /// Unix seconds when recorded.
    pub at: u64,
}

/// Recorded history for a single test.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestRecord {
    /// Total passing runs ever seen.
    pub passes: u64,
    /// Total failing runs ever seen.
    pub fails: u64,
    /// Unix seconds of the most recent failure, if any.
    pub last_fail: Option<u64>,
    /// Most recent observed wall-clock duration, milliseconds.
    pub last_duration_ms: Option<u64>,
    /// Recent outcomes (true = pass), newest last, capped at `RECENT_CAP`.
    pub recent: Vec<bool>,
}

impl TestRecord {
    /// Record one outcome.
    pub fn observe(&mut self, passed: bool, now: u64, duration_ms: Option<u64>) {
        if passed {
            self.passes += 1;
        } else {
            self.fails += 1;
            self.last_fail = Some(now);
        }
        if let Some(d) = duration_ms {
            self.last_duration_ms = Some(d);
        }
        self.recent.push(passed);
        if self.recent.len() > RECENT_CAP {
            let excess = self.recent.len() - RECENT_CAP;
            self.recent.drain(0..excess);
        }
    }

    /// Did this test fail within its recent window?
    #[must_use]
    pub fn recently_failed(&self) -> bool {
        self.recent.iter().any(|&p| !p)
    }
}

/// The on-disk store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    /// Crate name -> last green fingerprint.
    #[serde(default)]
    pub greens: BTreeMap<String, CrateGreen>,
    /// Test id -> history.
    #[serde(default)]
    pub tests: BTreeMap<String, TestRecord>,
}

impl Store {
    /// Default store directory for a workspace: `<root>/target/oryn`.
    #[must_use]
    pub fn dir_for(workspace_root: &Path) -> PathBuf {
        workspace_root.join("target").join("oryn")
    }

    fn file(dir: &Path) -> PathBuf {
        dir.join("store.json")
    }

    /// Load the store from `dir`, returning an empty store if none exists.
    ///
    /// # Errors
    /// Propagates I/O or JSON errors (a missing file is *not* an error).
    pub fn load(dir: &Path) -> io::Result<Self> {
        let path = Self::file(dir);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Persist the store to `dir` (creating it if needed), atomically.
    ///
    /// # Errors
    /// Propagates I/O or serialization errors.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = Self::file(dir);
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Is `crate_name` known-green at `fingerprint`?
    #[must_use]
    pub fn is_green(&self, crate_name: &str, fingerprint: &str) -> bool {
        self.greens
            .get(crate_name)
            .is_some_and(|g| g.fingerprint == fingerprint)
    }

    /// Record that `crate_name` passed cleanly at `fingerprint`.
    pub fn record_green(&mut self, crate_name: &str, fingerprint: &str, now: u64) {
        self.greens.insert(
            crate_name.to_string(),
            CrateGreen {
                fingerprint: fingerprint.to_string(),
                at: now,
            },
        );
    }

    /// Forget a crate's green status (e.g. after a failure).
    pub fn clear_green(&mut self, crate_name: &str) {
        self.greens.remove(crate_name);
    }

    /// Record a single test outcome into history.
    pub fn observe_test(&mut self, id: &str, passed: bool, now: u64, duration_ms: Option<u64>) {
        self.tests
            .entry(id.to_string())
            .or_default()
            .observe(passed, now, duration_ms);
    }
}

/// Current Unix time in seconds (0 if the clock is before the epoch).
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_roundtrip_and_match() {
        let mut s = Store::default();
        assert!(!s.is_green("core", "abc"));
        s.record_green("core", "abc", 100);
        assert!(s.is_green("core", "abc"));
        assert!(!s.is_green("core", "def")); // fingerprint changed -> not green
        s.clear_green("core");
        assert!(!s.is_green("core", "abc"));
    }

    #[test]
    fn test_history_tracks_outcomes_and_recent_failure() {
        let mut s = Store::default();
        s.observe_test("t::a", true, 1, Some(10));
        s.observe_test("t::a", false, 2, Some(12));
        let r = &s.tests["t::a"];
        assert_eq!(r.passes, 1);
        assert_eq!(r.fails, 1);
        assert_eq!(r.last_fail, Some(2));
        assert_eq!(r.last_duration_ms, Some(12));
        assert!(r.recently_failed());
    }

    #[test]
    fn recent_ring_is_capped() {
        let mut r = TestRecord::default();
        for i in 0..(RECENT_CAP + 20) {
            r.observe(true, i as u64, None);
        }
        assert_eq!(r.recent.len(), RECENT_CAP);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::default();
        s.record_green("core", "fp1", 5);
        s.observe_test("t::x", false, 7, Some(3));
        s.save(dir.path()).unwrap();
        let loaded = Store::load(dir.path()).unwrap();
        assert!(loaded.is_green("core", "fp1"));
        assert_eq!(loaded.tests["t::x"].fails, 1);
    }

    #[test]
    fn load_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::load(dir.path()).unwrap();
        assert!(s.greens.is_empty() && s.tests.is_empty());
    }
}
