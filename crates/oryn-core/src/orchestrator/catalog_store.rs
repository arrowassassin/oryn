//! Persisted, pinned, interval-refreshed model catalog — the "keep it parked"
//! mechanism behind locked design decision #2.
//!
//! A [`CatalogBundle`] pairs the capability snapshot with the live pricing
//! snapshot. It is **parked on disk** via a [`Store`] (filesystem impl in the app),
//! **pinned per run** (a mission snapshots the bundle at start, never mid-run), and
//! **refreshed on an interval** from real sources — keeping whatever it already has
//! when a source is offline ([`refresh_or_keep`]). First run / total failure falls
//! back to the bundled seed.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::orchestrator::catalog::{CapabilityCatalog, CapabilitySource, DimensionWeights};
use crate::orchestrator::pricing::{PricingSource, PricingTable};

/// The full parked snapshot: capability profiles + live pricing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogBundle {
    /// Per-model capability profiles (with provenance).
    pub capability: CapabilityCatalog,
    /// Per-model pricing (with provenance).
    pub pricing: PricingTable,
}

impl CatalogBundle {
    /// The bundled-seed bundle: offline, deterministic, always available.
    pub fn seed() -> Self {
        Self {
            capability: CapabilityCatalog::seed(),
            pricing: PricingTable::seed(),
        }
    }

    /// Serialize to pretty JSON for parking on disk.
    ///
    /// # Errors
    ///
    /// [`StoreError::Serialize`] if serialization fails (should not happen for this
    /// type, but surfaced rather than panicked).
    pub fn to_json(&self) -> Result<String, StoreError> {
        serde_json::to_string_pretty(self).map_err(|e| StoreError::Serialize(e.to_string()))
    }

    /// Parse a parked bundle from JSON.
    ///
    /// # Errors
    ///
    /// [`StoreError::Deserialize`] if the JSON is not a valid bundle.
    pub fn from_json(s: &str) -> Result<Self, StoreError> {
        serde_json::from_str(s).map_err(|e| StoreError::Deserialize(e.to_string()))
    }

    /// The age basis: the **oldest** of the two snapshots' fetch timestamps, so a
    /// stale half forces a refresh.
    pub fn fetched_at(&self) -> u64 {
        self.capability
            .provenance
            .fetched_at_unix
            .min(self.pricing.provenance.fetched_at_unix)
    }

    /// Whether the bundle is older than `interval_secs` as of `now_unix` (and thus
    /// due for a background refresh). Seed (`fetched_at == 0`) is always stale.
    pub fn is_stale(&self, now_unix: u64, interval_secs: u64) -> bool {
        now_unix.saturating_sub(self.fetched_at()) >= interval_secs
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Errors from parking / loading a bundle.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Could not write to the backing store.
    #[error("store write failed: {0}")]
    Write(String),
    /// Serialization failed.
    #[error("serialize failed: {0}")]
    Serialize(String),
    /// Deserialization failed.
    #[error("deserialize failed: {0}")]
    Deserialize(String),
}

/// Where the parked bundle lives. Object-safe; the app implements a filesystem
/// store, tests use an in-memory one.
pub trait Store: Send + Sync {
    /// Read the parked bundle JSON, or `None` if nothing is parked yet.
    fn read(&self) -> Option<String>;

    /// Park `json`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Write`] on I/O failure.
    fn write(&self, json: &str) -> Result<(), StoreError>;
}

/// Load the parked bundle, falling back to the seed when nothing is parked or the
/// parked data is corrupt — never fails.
pub fn load_or_seed(store: &dyn Store) -> CatalogBundle {
    match store.read() {
        Some(json) => CatalogBundle::from_json(&json).unwrap_or_else(|_| CatalogBundle::seed()),
        None => CatalogBundle::seed(),
    }
}

/// Park `bundle`.
///
/// # Errors
///
/// Propagates [`StoreError`] from serialization or the store write.
pub fn save(store: &dyn Store, bundle: &CatalogBundle) -> Result<(), StoreError> {
    store.write(&bundle.to_json()?)
}

/// Refresh policy: how long a parked bundle stays fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    /// Seconds before a parked bundle is considered stale.
    pub interval_secs: u64,
}

impl RefreshPolicy {
    /// A policy with the given interval.
    pub fn new(interval_secs: u64) -> Self {
        Self { interval_secs }
    }

    /// Whether `bundle` is due for refresh as of `now_unix`.
    pub fn due(&self, bundle: &CatalogBundle, now_unix: u64) -> bool {
        bundle.is_stale(now_unix, self.interval_secs)
    }
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        // Refresh roughly once a day.
        Self {
            interval_secs: 24 * 60 * 60,
        }
    }
}

/// Refresh from live sources, **keeping the current snapshot for any source that
/// fails** — so one offline benchmark/pricing endpoint never wipes parked data.
///
/// `now_unix` stamps provenance on whatever is successfully fetched.
pub fn refresh_or_keep(
    current: &CatalogBundle,
    capability_source: &dyn CapabilitySource,
    weights: &DimensionWeights,
    pricing_source: &dyn PricingSource,
    now_unix: u64,
) -> CatalogBundle {
    let capability = CapabilityCatalog::refreshed(capability_source, weights, now_unix)
        .unwrap_or_else(|_| current.capability.clone());
    let pricing = pricing_source
        .fetch(now_unix)
        .unwrap_or_else(|_| current.pricing.clone());
    CatalogBundle {
        capability,
        pricing,
    }
}

/// Load the parked bundle and, if it is due for refresh, fetch + re-park a fresh
/// one. Returns the bundle to pin for the session. Offline-safe throughout.
pub fn load_and_maybe_refresh(
    store: &dyn Store,
    policy: RefreshPolicy,
    capability_source: &dyn CapabilitySource,
    weights: &DimensionWeights,
    pricing_source: &dyn PricingSource,
    now_unix: u64,
) -> CatalogBundle {
    let current = load_or_seed(store);
    if !policy.due(&current, now_unix) {
        return current;
    }
    let refreshed = refresh_or_keep(
        &current,
        capability_source,
        weights,
        pricing_source,
        now_unix,
    );
    // Park the refreshed bundle; ignore write failures (we still return it).
    let _ = save(store, &refreshed);
    refreshed
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::catalog::{
        CatalogProvenance, RawBenchmarks, SourceError, default_weights,
    };
    use crate::orchestrator::pricing::PricingTable;
    use crate::orchestrator::provider::{ModelId, Pricing};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// In-memory store.
    #[derive(Default)]
    struct MemStore {
        slot: Mutex<Option<String>>,
    }
    impl Store for MemStore {
        fn read(&self) -> Option<String> {
            self.slot.lock().unwrap().clone()
        }
        fn write(&self, json: &str) -> Result<(), StoreError> {
            *self.slot.lock().unwrap() = Some(json.to_string());
            Ok(())
        }
    }

    struct FakeCap(Result<RawBenchmarks, ()>);
    impl CapabilitySource for FakeCap {
        fn id(&self) -> &str {
            "fake-bench"
        }
        fn fetch(&self) -> Result<RawBenchmarks, SourceError> {
            self.0.clone().map_err(|()| SourceError::Unavailable)
        }
    }

    struct FakePrice(Result<(), ()>);
    impl PricingSource for FakePrice {
        fn id(&self) -> &str {
            "fake-price"
        }
        fn fetch(&self, now: u64) -> Result<PricingTable, SourceError> {
            self.0.map_err(|()| SourceError::Unavailable)?;
            let mut prices = BTreeMap::new();
            prices.insert(
                ModelId::new("opus"),
                Pricing {
                    input: 9.0,
                    output: 9.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
            );
            Ok(PricingTable {
                prices,
                provenance: CatalogProvenance {
                    source: "fake-price".into(),
                    fetched_at_unix: now,
                    version: "v1".into(),
                },
            })
        }
    }

    fn bench() -> RawBenchmarks {
        let mut m = BTreeMap::new();
        m.insert(
            ModelId::new("opus"),
            BTreeMap::from([("aider-polyglot".to_string(), 0.9)]),
        );
        RawBenchmarks { metrics: m }
    }

    #[test]
    fn load_or_seed_returns_seed_when_empty() {
        let store = MemStore::default();
        assert_eq!(load_or_seed(&store), CatalogBundle::seed());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let store = MemStore::default();
        let bundle = CatalogBundle::seed();
        save(&store, &bundle).unwrap();
        assert_eq!(load_or_seed(&store), bundle);
    }

    #[test]
    fn corrupt_parked_data_falls_back_to_seed() {
        let store = MemStore::default();
        store.write("{ not a bundle }").unwrap();
        assert_eq!(load_or_seed(&store), CatalogBundle::seed());
    }

    #[test]
    fn seed_is_always_stale() {
        assert!(CatalogBundle::seed().is_stale(10_000, 3600));
        assert!(RefreshPolicy::new(3600).due(&CatalogBundle::seed(), 10_000));
    }

    #[test]
    fn fresh_bundle_is_not_stale_within_interval() {
        let fresh = refresh_or_keep(
            &CatalogBundle::seed(),
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Ok(())),
            1_000_000,
        );
        assert!(!fresh.is_stale(1_000_000 + 10, 3600));
        assert!(fresh.is_stale(1_000_000 + 3600, 3600));
    }

    #[test]
    fn refresh_pulls_both_sources() {
        let fresh = refresh_or_keep(
            &CatalogBundle::seed(),
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Ok(())),
            42,
        );
        assert_eq!(fresh.capability.provenance.fetched_at_unix, 42);
        assert_eq!(fresh.pricing.provenance.fetched_at_unix, 42);
        assert_eq!(
            fresh.pricing.price(&ModelId::new("opus")).unwrap().input,
            9.0
        );
    }

    #[test]
    fn refresh_keeps_current_when_pricing_source_down() {
        let current = CatalogBundle::seed();
        let kept = refresh_or_keep(
            &current,
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Err(())),
            42,
        );
        // pricing kept from seed, capability refreshed
        assert_eq!(kept.pricing, current.pricing);
        assert_eq!(kept.capability.provenance.fetched_at_unix, 42);
    }

    #[test]
    fn refresh_keeps_current_when_benchmark_source_down() {
        let current = CatalogBundle::seed();
        let kept = refresh_or_keep(
            &current,
            &FakeCap(Err(())),
            &default_weights(),
            &FakePrice(Ok(())),
            42,
        );
        assert_eq!(kept.capability, current.capability);
        assert_eq!(kept.pricing.provenance.fetched_at_unix, 42);
    }

    #[test]
    fn load_and_maybe_refresh_parks_when_stale() {
        let store = MemStore::default();
        // Empty store → seed (stale) → refresh + park.
        let bundle = load_and_maybe_refresh(
            &store,
            RefreshPolicy::new(3600),
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Ok(())),
            5_000,
        );
        assert_eq!(bundle.pricing.provenance.fetched_at_unix, 5_000);
        // It was parked: reloading yields the same fresh bundle.
        assert_eq!(load_or_seed(&store), bundle);
    }

    #[test]
    fn load_and_maybe_refresh_skips_when_fresh() {
        let store = MemStore::default();
        let fresh = refresh_or_keep(
            &CatalogBundle::seed(),
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Ok(())),
            1_000,
        );
        save(&store, &fresh).unwrap();
        // now only 100s later, interval 3600 → not due → returns parked unchanged,
        // even though the sources would return now=1_500.
        let got = load_and_maybe_refresh(
            &store,
            RefreshPolicy::new(3600),
            &FakeCap(Ok(bench())),
            &default_weights(),
            &FakePrice(Ok(())),
            1_100,
        );
        assert_eq!(got, fresh);
    }
}
