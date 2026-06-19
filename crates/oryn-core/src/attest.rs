//! Tamper-evident, signed attestations for reproducibility evidence.
//!
//! Every Oryn report can be sealed into an append-only, hash-chained log where
//! each entry is Ed25519-signed. Recomputing the chain detects any edit; the
//! signatures bind it to a key. This is the deterministic, classical-crypto
//! substrate auditors ask for (EU AI Act Art. 12/19 record-keeping) — no model
//! involved, just BLAKE3 + Ed25519.

use crate::{OrynError, Result};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};

/// A signing identity.
pub struct Signer {
    key: SigningKey,
}

impl Signer {
    /// Generate a fresh random identity from the OS CSPRNG.
    #[must_use]
    pub fn generate() -> Self {
        let mut csprng = rand::rngs::OsRng;
        Self {
            key: SigningKey::generate(&mut csprng),
        }
    }

    /// Construct from a 32-byte secret seed (deterministic identities, tests).
    #[must_use]
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// Construct from a hex-encoded 32-byte secret seed.
    ///
    /// # Errors
    /// Errors if the hex is malformed or not 32 bytes.
    pub fn from_seed_hex(hex_seed: &str) -> Result<Self> {
        let bytes = hex::decode(hex_seed.trim())?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| OrynError::InvalidParameter("seed must be 32 bytes".into()))?;
        Ok(Self::from_seed(&arr))
    }

    /// Hex-encoded public (verifying) key.
    #[must_use]
    pub fn public_hex(&self) -> String {
        hex::encode(self.key.verifying_key().to_bytes())
    }

    /// Hex-encoded secret seed. Handle with care.
    #[must_use]
    pub fn secret_hex(&self) -> String {
        hex::encode(self.key.to_bytes())
    }
}

/// One signed, chained attestation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attestation {
    /// Zero-based position in the chain.
    pub index: u64,
    /// Free-form label (e.g. "contamination-scan").
    pub label: String,
    /// BLAKE3 hex hash of the attested payload.
    pub payload_hash: String,
    /// Entry hash of the previous link, or empty for the genesis entry.
    pub prev_hash: String,
    /// BLAKE3 hex hash binding index + payload_hash + prev_hash.
    pub entry_hash: String,
    /// Hex Ed25519 signature over `entry_hash` bytes.
    pub signature: String,
    /// Hex Ed25519 public key of the signer.
    pub public_key: String,
}

/// Compute the entry hash from its bound fields.
fn compute_entry_hash(index: u64, payload_hash: &str, prev_hash: &str) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&index.to_le_bytes());
    hasher.update(&hex::decode(payload_hash)?);
    if !prev_hash.is_empty() {
        hasher.update(&hex::decode(prev_hash)?);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// An append-only chain of signed attestations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttestationChain {
    /// Entries in order.
    pub entries: Vec<Attestation>,
}

impl AttestationChain {
    /// New empty chain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an attestation over `payload`, signed by `signer`.
    ///
    /// # Errors
    /// Propagates hex/hash errors (practically infallible for valid input).
    pub fn append(&mut self, signer: &Signer, label: &str, payload: &[u8]) -> Result<&Attestation> {
        let index = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_default();
        let payload_hash = blake3::hash(payload).to_hex().to_string();
        let entry_hash = compute_entry_hash(index, &payload_hash, &prev_hash)?;
        let signature = signer.key.sign(entry_hash.as_bytes());
        self.entries.push(Attestation {
            index,
            label: label.to_string(),
            payload_hash,
            prev_hash,
            entry_hash,
            signature: hex::encode(signature.to_bytes()),
            public_key: signer.public_hex(),
        });
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Verify linkage, entry hashes, and signatures across the whole chain.
    ///
    /// # Errors
    /// Returns [`OrynError::Attestation`] on the first inconsistency found.
    pub fn verify(&self) -> Result<()> {
        let mut prev = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.index != i as u64 {
                return Err(OrynError::Attestation(format!(
                    "entry {i} has index {}",
                    e.index
                )));
            }
            if e.prev_hash != prev {
                return Err(OrynError::Attestation(format!(
                    "entry {i} prev_hash does not chain"
                )));
            }
            let recomputed = compute_entry_hash(e.index, &e.payload_hash, &e.prev_hash)?;
            if recomputed != e.entry_hash {
                return Err(OrynError::Attestation(format!(
                    "entry {i} hash mismatch (tampered payload or fields)"
                )));
            }
            verify_signature(&e.public_key, &e.signature, e.entry_hash.as_bytes())
                .map_err(|err| OrynError::Attestation(format!("entry {i}: {err}")))?;
            prev = e.entry_hash.clone();
        }
        Ok(())
    }

    /// Confirm a payload's hash matches the entry at `index` (proof of inclusion
    /// for a specific report).
    ///
    /// # Errors
    /// Errors if the index is out of range or the payload does not match.
    pub fn verify_payload(&self, index: usize, payload: &[u8]) -> Result<()> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| OrynError::Attestation(format!("no entry at {index}")))?;
        let h = blake3::hash(payload).to_hex().to_string();
        if h != e.payload_hash {
            return Err(OrynError::Attestation(format!(
                "payload does not match entry {index}"
            )));
        }
        Ok(())
    }
}

fn verify_signature(public_hex: &str, sig_hex: &str, msg: &[u8]) -> Result<()> {
    let pk_bytes: [u8; 32] = hex::decode(public_hex)?
        .try_into()
        .map_err(|_| OrynError::Signature("public key must be 32 bytes".into()))?;
    let vk =
        VerifyingKey::from_bytes(&pk_bytes).map_err(|e| OrynError::Signature(e.to_string()))?;
    let sig_bytes: [u8; 64] = hex::decode(sig_hex)?
        .try_into()
        .map_err(|_| OrynError::Signature("signature must be 64 bytes".into()))?;
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify(msg, &sig)
        .map_err(|e| OrynError::Signature(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> Signer {
        Signer::from_seed(&[7u8; 32])
    }

    #[test]
    fn single_entry_verifies() {
        let mut chain = AttestationChain::new();
        chain.append(&signer(), "report", b"hello world").unwrap();
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn multi_entry_chain_verifies() {
        let s = signer();
        let mut chain = AttestationChain::new();
        chain.append(&s, "scan", b"payload-1").unwrap();
        chain.append(&s, "eval", b"payload-2").unwrap();
        chain.append(&s, "determinism", b"payload-3").unwrap();
        assert!(chain.verify().is_ok());
        assert_eq!(chain.entries.len(), 3);
        assert_eq!(chain.entries[0].prev_hash, "");
        assert_eq!(chain.entries[1].prev_hash, chain.entries[0].entry_hash);
    }

    #[test]
    fn tampered_payload_hash_is_detected() {
        let mut chain = AttestationChain::new();
        chain.append(&signer(), "report", b"original").unwrap();
        chain.entries[0].payload_hash = blake3::hash(b"forged").to_hex().to_string();
        assert!(chain.verify().is_err());
    }

    #[test]
    fn reordering_breaks_chain() {
        let s = signer();
        let mut chain = AttestationChain::new();
        chain.append(&s, "a", b"1").unwrap();
        chain.append(&s, "b", b"2").unwrap();
        chain.entries.swap(0, 1);
        assert!(chain.verify().is_err());
    }

    #[test]
    fn payload_inclusion_proof() {
        let mut chain = AttestationChain::new();
        chain
            .append(&signer(), "report", b"the real payload")
            .unwrap();
        assert!(chain.verify_payload(0, b"the real payload").is_ok());
        assert!(chain.verify_payload(0, b"a different payload").is_err());
    }

    #[test]
    fn deterministic_signer_is_stable() {
        assert_eq!(signer().public_hex(), signer().public_hex());
    }
}
