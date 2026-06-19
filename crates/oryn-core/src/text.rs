//! Deterministic text normalization, tokenization and n-gram shingling.
//!
//! Every function here is pure and order-stable: the same input always yields
//! the same tokens and the same hashes, on any platform. This is the bedrock of
//! the contamination scanner — contamination detection that is itself
//! nondeterministic would be worthless.
//!
//! Hashing uses a fixed-seed [`ahash`] state so n-gram fingerprints are stable
//! across processes (unlike the default `RandomState`, which seeds from the RNG).

use ahash::AHasher;
use std::hash::{BuildHasher, Hash, Hasher};

/// Fixed seeds so fingerprints are reproducible across runs and machines.
const SEED0: u64 = 0x243f_6a88_85a3_08d3;
const SEED1: u64 = 0x1319_8a2e_0370_7344;
const SEED2: u64 = 0xa409_3822_299f_31d0;
const SEED3: u64 = 0x082e_fa98_ec4e_6c89;

/// How input text is normalized before tokenization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Normalization {
    /// No change — exact bytes (split on ASCII whitespace only).
    Raw,
    /// Lowercase, collapse runs of non-alphanumeric characters into single
    /// spaces. This is the standard for benchmark-contamination n-gram matching
    /// (it defeats trivial punctuation/casing edits).
    #[default]
    Standard,
}

/// Tokenize `text` into a vector of word tokens under `norm`.
#[must_use]
pub fn tokenize(text: &str, norm: Normalization) -> Vec<String> {
    match norm {
        Normalization::Raw => text.split_whitespace().map(str::to_string).collect(),
        Normalization::Standard => {
            let lower = text.to_lowercase();
            let mut out = Vec::new();
            let mut cur = String::new();
            for ch in lower.chars() {
                if ch.is_alphanumeric() {
                    cur.push(ch);
                } else if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            if !cur.is_empty() {
                out.push(cur);
            }
            out
        }
    }
}

/// Hash a single token slice with a fixed-seed hasher.
fn hash_tokens(tokens: &[String]) -> u64 {
    let mut h = AHasher::default_with_seeds();
    tokens.len().hash(&mut h);
    for t in tokens {
        t.hash(&mut h);
    }
    h.finish()
}

trait SeededHasher {
    fn default_with_seeds() -> Self;
}

impl SeededHasher for AHasher {
    fn default_with_seeds() -> Self {
        ahash::RandomState::with_seeds(SEED0, SEED1, SEED2, SEED3).build_hasher()
    }
}

/// Compute the set of distinct n-gram fingerprints (as `u64`) for `text`.
///
/// `n` is the n-gram width in tokens (e.g. 13 for GPT-3-style contamination
/// checks). Returns deduplicated, sorted fingerprints for stable downstream use.
#[must_use]
pub fn ngram_fingerprints(text: &str, n: usize, norm: Normalization) -> Vec<u64> {
    let tokens = tokenize(text, norm);
    let mut out = ngram_fingerprints_from_tokens(&tokens, n);
    out.sort_unstable();
    out.dedup();
    out
}

/// Same as [`ngram_fingerprints`] but from pre-tokenized input (no sort/dedup).
#[must_use]
pub fn ngram_fingerprints_from_tokens(tokens: &[String], n: usize) -> Vec<u64> {
    let n = n.max(1);
    if tokens.len() < n {
        // Document shorter than the window: fingerprint the whole thing once so
        // short items are still comparable instead of silently invisible.
        if tokens.is_empty() {
            return Vec::new();
        }
        return vec![hash_tokens(tokens)];
    }
    let mut out = Vec::with_capacity(tokens.len() - n + 1);
    for window in tokens.windows(n) {
        out.push(hash_tokens(window));
    }
    out
}

/// Strong content hash of arbitrary bytes (BLAKE3, hex-encoded).
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_normalization_is_case_and_punct_insensitive() {
        let a = tokenize("Hello, World!", Normalization::Standard);
        let b = tokenize("hello world", Normalization::Standard);
        assert_eq!(a, b);
        assert_eq!(a, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn raw_normalization_preserves_case() {
        let a = tokenize("Hello World", Normalization::Raw);
        assert_eq!(a, vec!["Hello".to_string(), "World".to_string()]);
    }

    #[test]
    fn fingerprints_are_deterministic_across_calls() {
        let t = "the quick brown fox jumps over the lazy dog";
        let a = ngram_fingerprints(t, 3, Normalization::Standard);
        let b = ngram_fingerprints(t, 3, Normalization::Standard);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn identical_text_shares_all_ngrams() {
        let t = "alpha beta gamma delta epsilon zeta";
        let a = ngram_fingerprints(t, 3, Normalization::Standard);
        let b = ngram_fingerprints(t, 3, Normalization::Standard);
        assert_eq!(a, b);
    }

    #[test]
    fn disjoint_text_shares_no_ngrams() {
        let a = ngram_fingerprints("alpha beta gamma delta", 2, Normalization::Standard);
        let b = ngram_fingerprints("one two three four", 2, Normalization::Standard);
        assert!(a.iter().all(|x| !b.contains(x)));
    }

    #[test]
    fn short_doc_yields_single_fingerprint() {
        let fp = ngram_fingerprints("two words", 13, Normalization::Standard);
        assert_eq!(fp.len(), 1);
    }
}
