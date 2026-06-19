//! Benchmark/training-data contamination scanner — classical, deterministic.
//!
//! Two complementary signals, both non-ML:
//!
//! * **Exact n-gram overlap** — the standard verbatim-leak check (GPT-3 used
//!   13-grams). For each eval item we measure the fraction of its n-grams that
//!   also occur anywhere in the reference corpus.
//! * **MinHash + LSH** — near-duplicate detection that survives paraphrase and
//!   reordering, the production standard for web-scale dedup (MinHashLSH).
//!
//! The scanner reports per-item contamination and emits a **clean held-out
//! split** (the eval items below threshold) so a contaminated benchmark can be
//! salvaged instead of discarded.

use crate::text::{ngram_fingerprints, Normalization};
use ahash::{AHashMap, AHashSet};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Mersenne prime 2^61 − 1, used for universal hashing in MinHash.
const MERSENNE_P: u64 = (1 << 61) - 1;

/// A single text record with a stable identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Caller-chosen identifier (kept verbatim in reports).
    pub id: String,
    /// Raw text.
    pub text: String,
}

impl Document {
    /// Convenience constructor.
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

/// Tunable scan parameters. Defaults follow common contamination-audit practice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    /// n-gram width in tokens.
    pub ngram_n: usize,
    /// Text normalization.
    pub normalization: Normalization,
    /// Fraction of an item's n-grams present in the corpus to flag it.
    pub ngram_threshold: f64,
    /// Estimated Jaccard similarity (MinHash) to flag near-duplication.
    pub jaccard_threshold: f64,
    /// Number of MinHash permutations (signature length).
    pub minhash_perms: usize,
    /// Number of LSH bands (must divide `minhash_perms`).
    pub lsh_bands: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            ngram_n: 13,
            normalization: Normalization::Standard,
            ngram_threshold: 0.5,
            jaccard_threshold: 0.8,
            minhash_perms: 128,
            lsh_bands: 32,
        }
    }
}

// `Normalization` lives in `text.rs`; it gets its serde impls here so
// `ScanConfig` can derive `Serialize`/`Deserialize`.
impl Serialize for Normalization {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Normalization::Raw => "raw",
            Normalization::Standard => "standard",
        })
    }
}
impl<'de> Deserialize<'de> for Normalization {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "raw" => Ok(Normalization::Raw),
            _ => Ok(Normalization::Standard),
        }
    }
}

/// Per-item contamination finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemContamination {
    /// Eval item id.
    pub id: String,
    /// Fraction of the item's n-grams found in the corpus (0..=1).
    pub ngram_overlap: f64,
    /// Best estimated Jaccard similarity to any corpus document (0..=1).
    pub estimated_jaccard: f64,
    /// Corpus document id of the closest match, if any candidate was found.
    pub closest_source: Option<String>,
    /// Number of the item's n-grams matched in the corpus.
    pub matched_ngrams: usize,
    /// Total n-grams in the item.
    pub total_ngrams: usize,
    /// True if either signal exceeded its threshold.
    pub contaminated: bool,
}

/// Aggregate scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContaminationReport {
    /// Config used.
    pub config: ScanConfig,
    /// Per-item findings (input order preserved).
    pub items: Vec<ItemContamination>,
    /// Number of eval items.
    pub total_items: usize,
    /// Number flagged as contaminated.
    pub contaminated_items: usize,
    /// `contaminated_items / total_items`.
    pub contamination_rate: f64,
    /// Mean n-gram overlap across items.
    pub mean_overlap: f64,
    /// Ids of items that passed (the clean held-out split).
    pub clean_holdout: Vec<String>,
}

/// MinHash signature (one min per permutation).
type MinHashSig = Vec<u64>;

/// An indexed reference corpus: exact n-gram set + MinHash/LSH structures.
pub struct CorpusIndex {
    config: ScanConfig,
    /// Every n-gram fingerprint present anywhere in the corpus.
    ngrams: AHashSet<u64>,
    /// Per-document MinHash signatures.
    signatures: Vec<MinHashSig>,
    /// Document ids, parallel to `signatures`.
    doc_ids: Vec<String>,
    /// LSH buckets: (band, band-hash) -> doc indices.
    buckets: AHashMap<(u32, u64), Vec<usize>>,
    /// Universal-hash coefficients (a, b), fixed-seeded for reproducibility.
    coeffs: Vec<(u64, u64)>,
    rows_per_band: usize,
}

impl CorpusIndex {
    /// Build an index over `corpus` using `config`.
    #[must_use]
    pub fn build(corpus: &[Document], config: ScanConfig) -> Self {
        let perms = config.minhash_perms.max(1);
        let bands = config.lsh_bands.clamp(1, perms);
        let rows_per_band = (perms / bands).max(1);
        let coeffs = minhash_coeffs(perms);

        let mut ngrams = AHashSet::new();
        let mut signatures = Vec::with_capacity(corpus.len());
        let mut doc_ids = Vec::with_capacity(corpus.len());
        let mut buckets: AHashMap<(u32, u64), Vec<usize>> = AHashMap::new();

        for (idx, doc) in corpus.iter().enumerate() {
            let fps = ngram_fingerprints(&doc.text, config.ngram_n, config.normalization);
            for &f in &fps {
                ngrams.insert(f);
            }
            let sig = minhash_signature(&fps, &coeffs);
            for (band, key) in band_keys(&sig, bands, rows_per_band) {
                buckets.entry((band, key)).or_default().push(idx);
            }
            signatures.push(sig);
            doc_ids.push(doc.id.clone());
        }

        Self {
            config,
            ngrams,
            signatures,
            doc_ids,
            buckets,
            coeffs,
            rows_per_band,
        }
    }

    /// Number of documents indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.doc_ids.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.doc_ids.is_empty()
    }

    /// Scan `eval` against the corpus, returning a full report.
    #[must_use]
    pub fn scan(&self, eval: &[Document]) -> ContaminationReport {
        let bands = self
            .config
            .lsh_bands
            .clamp(1, self.config.minhash_perms.max(1));
        let items: Vec<ItemContamination> = eval
            .par_iter()
            .map(|item| {
                let fps =
                    ngram_fingerprints(&item.text, self.config.ngram_n, self.config.normalization);
                let total = fps.len();
                let matched = fps.iter().filter(|f| self.ngrams.contains(f)).count();
                let overlap = if total == 0 {
                    0.0
                } else {
                    matched as f64 / total as f64
                };

                // MinHash near-duplicate via LSH candidate retrieval.
                let sig = minhash_signature(&fps, &self.coeffs);
                let mut candidates: AHashSet<usize> = AHashSet::new();
                for (band, key) in band_keys(&sig, bands, self.rows_per_band) {
                    if let Some(v) = self.buckets.get(&(band, key)) {
                        candidates.extend(v.iter().copied());
                    }
                }
                let mut best_jaccard = 0.0_f64;
                let mut best_src: Option<String> = None;
                // Deterministic iteration order: sort candidate indices.
                let mut cand: Vec<usize> = candidates.into_iter().collect();
                cand.sort_unstable();
                for c in cand {
                    let j = jaccard_estimate(&sig, &self.signatures[c]);
                    if j > best_jaccard {
                        best_jaccard = j;
                        best_src = Some(self.doc_ids[c].clone());
                    }
                }

                let contaminated = overlap >= self.config.ngram_threshold
                    || best_jaccard >= self.config.jaccard_threshold;

                ItemContamination {
                    id: item.id.clone(),
                    ngram_overlap: overlap,
                    estimated_jaccard: best_jaccard,
                    closest_source: best_src,
                    matched_ngrams: matched,
                    total_ngrams: total,
                    contaminated,
                }
            })
            .collect();

        let total_items = items.len();
        let contaminated_items = items.iter().filter(|i| i.contaminated).count();
        let mean_overlap = if total_items == 0 {
            0.0
        } else {
            items.iter().map(|i| i.ngram_overlap).sum::<f64>() / total_items as f64
        };
        let clean_holdout = items
            .iter()
            .filter(|i| !i.contaminated)
            .map(|i| i.id.clone())
            .collect();

        ContaminationReport {
            config: self.config.clone(),
            contamination_rate: if total_items == 0 {
                0.0
            } else {
                contaminated_items as f64 / total_items as f64
            },
            items,
            total_items,
            contaminated_items,
            mean_overlap,
            clean_holdout,
        }
    }
}

/// A near-duplicate pair found *within* a single document set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    /// First document id (lexicographically smaller).
    pub a: String,
    /// Second document id.
    pub b: String,
    /// Estimated Jaccard similarity.
    pub estimated_jaccard: f64,
}

/// Find near-duplicate pairs *inside* `docs` (intra-set dedup). Useful for
/// catching an eval benchmark that secretly repeats items, which silently
/// inflates the effective sample size and biases CIs.
///
/// Returns pairs with estimated Jaccard `>= cfg.jaccard_threshold`, sorted
/// deterministically by (similarity desc, ids).
#[must_use]
pub fn self_duplicates(docs: &[Document], cfg: &ScanConfig) -> Vec<DuplicatePair> {
    let perms = cfg.minhash_perms.max(1);
    let bands = cfg.lsh_bands.clamp(1, perms);
    let rows = (perms / bands).max(1);
    let coeffs = minhash_coeffs(perms);

    let sigs: Vec<MinHashSig> = docs
        .iter()
        .map(|d| {
            let fps = ngram_fingerprints(&d.text, cfg.ngram_n, cfg.normalization);
            minhash_signature(&fps, &coeffs)
        })
        .collect();

    // Bucket by band; any co-bucketed pair is a candidate.
    let mut buckets: AHashMap<(u32, u64), Vec<usize>> = AHashMap::new();
    for (i, sig) in sigs.iter().enumerate() {
        for (band, key) in band_keys(sig, bands, rows) {
            buckets.entry((band, key)).or_default().push(i);
        }
    }

    let mut seen: AHashSet<(usize, usize)> = AHashSet::new();
    let mut out = Vec::new();
    for members in buckets.values() {
        for (x, &i) in members.iter().enumerate() {
            for &j in &members[x + 1..] {
                let (lo, hi) = if i < j { (i, j) } else { (j, i) };
                if !seen.insert((lo, hi)) {
                    continue;
                }
                let jac = jaccard_estimate(&sigs[lo], &sigs[hi]);
                if jac >= cfg.jaccard_threshold {
                    let (mut a, mut b) = (docs[lo].id.clone(), docs[hi].id.clone());
                    if a > b {
                        std::mem::swap(&mut a, &mut b);
                    }
                    out.push(DuplicatePair {
                        a,
                        b,
                        estimated_jaccard: jac,
                    });
                }
            }
        }
    }
    out.sort_by(|p, q| {
        q.estimated_jaccard
            .partial_cmp(&p.estimated_jaccard)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(p.a.cmp(&q.a))
            .then(p.b.cmp(&q.b))
    });
    out
}

/// Generate deterministic (a, b) coefficients for `k` hash permutations.
fn minhash_coeffs(k: usize) -> Vec<(u64, u64)> {
    // SplitMix64 seeded deterministically; coefficients reduced mod p.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    (0..k)
        .map(|_| {
            let a = (next() % (MERSENNE_P - 1)) + 1; // a in [1, p-1]
            let b = next() % MERSENNE_P; // b in [0, p-1]
            (a, b)
        })
        .collect()
}

/// MinHash: for each (a,b), min over shingles of (a*x + b) mod p.
fn minhash_signature(fingerprints: &[u64], coeffs: &[(u64, u64)]) -> MinHashSig {
    if fingerprints.is_empty() {
        return vec![u64::MAX; coeffs.len()];
    }
    coeffs
        .iter()
        .map(|&(a, b)| {
            fingerprints
                .iter()
                .map(|&x| {
                    let xm = x % MERSENNE_P;
                    let prod = (a as u128 * xm as u128 + b as u128) % MERSENNE_P as u128;
                    prod as u64
                })
                .min()
                .unwrap_or(u64::MAX)
        })
        .collect()
}

/// Estimated Jaccard = fraction of signature positions that agree.
fn jaccard_estimate(a: &MinHashSig, b: &MinHashSig) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let eq = a.iter().zip(b).filter(|(x, y)| x == y).count();
    eq as f64 / a.len() as f64
}

/// Yield (band index, band hash) pairs for LSH bucketing.
fn band_keys(sig: &MinHashSig, bands: usize, rows: usize) -> Vec<(u32, u64)> {
    let mut out = Vec::with_capacity(bands);
    for band in 0..bands {
        let start = band * rows;
        let end = (start + rows).min(sig.len());
        if start >= end {
            break;
        }
        // FNV-1a over the band's mins -> stable bucket key.
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for &v in &sig[start..end] {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        out.push((band as u32, h));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_small() -> ScanConfig {
        ScanConfig {
            ngram_n: 3,
            ngram_threshold: 0.5,
            jaccard_threshold: 0.7,
            minhash_perms: 64,
            lsh_bands: 16,
            ..Default::default()
        }
    }

    #[test]
    fn exact_duplicate_is_flagged() {
        let corpus = vec![Document::new(
            "c1",
            "the capital of france is paris and the eiffel tower is there",
        )];
        let idx = CorpusIndex::build(&corpus, cfg_small());
        let eval = vec![Document::new(
            "e1",
            "the capital of france is paris and the eiffel tower is there",
        )];
        let r = idx.scan(&eval);
        assert_eq!(r.contaminated_items, 1);
        assert!(r.items[0].ngram_overlap > 0.99);
        assert!(r.items[0].estimated_jaccard > 0.99);
        assert_eq!(r.items[0].closest_source.as_deref(), Some("c1"));
        assert!(r.clean_holdout.is_empty());
    }

    #[test]
    fn unrelated_item_is_clean() {
        let corpus = vec![Document::new(
            "c1",
            "alpha beta gamma delta epsilon zeta eta",
        )];
        let idx = CorpusIndex::build(&corpus, cfg_small());
        let eval = vec![Document::new(
            "e1",
            "completely different words about quantum chromodynamics today",
        )];
        let r = idx.scan(&eval);
        assert_eq!(r.contaminated_items, 0);
        assert_eq!(r.clean_holdout, vec!["e1".to_string()]);
        assert!(r.items[0].ngram_overlap < 0.5);
    }

    #[test]
    fn partial_overlap_measured() {
        let corpus = vec![Document::new(
            "c1",
            "the quick brown fox jumps over the lazy dog every single day",
        )];
        let idx = CorpusIndex::build(&corpus, cfg_small());
        // First half overlaps, second half is novel.
        let eval = vec![Document::new(
            "e1",
            "the quick brown fox jumps then totally unrelated novel tokens appear",
        )];
        let r = idx.scan(&eval);
        let it = &r.items[0];
        assert!(it.ngram_overlap > 0.0 && it.ngram_overlap < 1.0);
    }

    #[test]
    fn scan_is_deterministic() {
        let corpus = vec![
            Document::new(
                "c1",
                "lorem ipsum dolor sit amet consectetur adipiscing elit",
            ),
            Document::new("c2", "sed do eiusmod tempor incididunt ut labore et dolore"),
        ];
        let idx = CorpusIndex::build(&corpus, cfg_small());
        let eval = vec![Document::new(
            "e1",
            "lorem ipsum dolor sit amet consectetur adipiscing",
        )];
        let a = idx.scan(&eval);
        let b = idx.scan(&eval);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn self_duplicates_found_within_set() {
        let docs = vec![
            Document::new("a", "one two three four five six seven eight nine ten"),
            Document::new("b", "one two three four five six seven eight nine ten"),
            Document::new(
                "c",
                "totally distinct content about marine biology and coral reefs",
            ),
        ];
        let cfg = ScanConfig {
            ngram_n: 2,
            jaccard_threshold: 0.7,
            ..cfg_small()
        };
        let dups = self_duplicates(&docs, &cfg);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].a, "a");
        assert_eq!(dups[0].b, "b");
        assert!(dups[0].estimated_jaccard >= 0.7);
    }

    #[test]
    fn no_self_duplicates_when_all_distinct() {
        let docs = vec![
            Document::new("a", "alpha beta gamma delta"),
            Document::new("b", "one two three four"),
            Document::new("c", "red green blue yellow"),
        ];
        assert!(self_duplicates(&docs, &cfg_small()).is_empty());
    }

    #[test]
    fn minhash_jaccard_tracks_real_overlap() {
        // Two near-identical docs should have high estimated Jaccard.
        let a_text = "one two three four five six seven eight nine ten eleven twelve";
        let b_text = "one two three four five six seven eight nine ten eleven thirteen";
        let coeffs = minhash_coeffs(256);
        let fa = ngram_fingerprints(a_text, 2, Normalization::Standard);
        let fb = ngram_fingerprints(b_text, 2, Normalization::Standard);
        let sa = minhash_signature(&fa, &coeffs);
        let sb = minhash_signature(&fb, &coeffs);
        let j = jaccard_estimate(&sa, &sb);
        assert!(j > 0.6, "expected high jaccard, got {j}");
    }
}
