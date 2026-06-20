//! Hybrid impact analysis: dynamic coverage for function-body changes + a static
//! reference graph for non-execution dependencies (`const`/`static`/`type`).
//!
//! For each changed line:
//! * inside a **function** → that function's span is impacted (coverage handles
//!   per-test precision);
//! * inside a **`const`/`static`/`type`** → seed the reference graph; the
//!   functions that transitively reference it become impacted;
//! * inside anything else (struct/enum/trait/impl/macro/mod) or outside every
//!   item, or if the file won't parse → **whole-crate fallback** (safe).

use crate::difflines::Hunk;
use crate::fnselect::FileImpact;
use crate::refgraph::{ItemKind, RefGraph};
use std::collections::{BTreeMap, BTreeSet};

/// Result of hybrid analysis for one crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HybridImpact {
    /// A change we can't localize safely — rerun the whole crate.
    WholeCrate,
    /// Precise per-file impacted line sets.
    PerFile(BTreeMap<String, FileImpact>),
}

/// Analyze a crate's `hunks` against its base-revision `base_files`.
#[must_use]
pub fn analyze(
    base_files: &[(String, String)],
    hunks: &BTreeMap<String, Vec<Hunk>>,
) -> HybridImpact {
    if hunks.is_empty() {
        return HybridImpact::PerFile(BTreeMap::new());
    }
    let g = RefGraph::build(base_files);
    let mut impacted: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut seed: BTreeSet<usize> = BTreeSet::new();

    for (file, hs) in hunks {
        for h in hs {
            let (a, b) = h.touched_old_lines();
            for line in a..=b {
                if let Some((s, e)) = g.enclosing_fn(file, line) {
                    impacted.entry(file.clone()).or_default().extend(s..=e);
                } else if let Some(idx) = g.enclosing_nonfn(file, line) {
                    match g.kind(idx) {
                        ItemKind::Const | ItemKind::Static | ItemKind::TypeAlias => {
                            seed.insert(idx);
                        }
                        // Opaque (struct/enum/trait/impl/macro/...) — too broad.
                        ItemKind::Fn => unreachable!("fn handled above"),
                        ItemKind::Opaque => return HybridImpact::WholeCrate,
                    }
                } else {
                    // Change outside every item (or unparseable file).
                    return HybridImpact::WholeCrate;
                }
            }
        }
    }

    for (file, s, e) in g.reverse_reachable_functions(&seed) {
        impacted.entry(file).or_default().extend(s..=e);
    }

    HybridImpact::PerFile(
        impacted
            .into_iter()
            .map(|(f, l)| (f, FileImpact::Lines(l)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
const LIMIT: u32 = 10;\n\
fn uses_limit() -> u32 {\n    LIMIT * 2\n}\n\
fn unrelated() -> u32 {\n    42\n}\n\
struct S { x: u32 }\n";

    fn files() -> Vec<(String, String)> {
        vec![("src/lib.rs".to_string(), SRC.to_string())]
    }

    #[test]
    fn const_change_localizes_to_referencing_functions() {
        // change old line 1 (the const)
        let hunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![Hunk {
                old_start: 1,
                old_count: 1,
            }],
        )]);
        match analyze(&files(), &hunks) {
            HybridImpact::PerFile(impacts) => {
                let FileImpact::Lines(lines) = &impacts["src/lib.rs"] else {
                    panic!("expected lines");
                };
                assert!(lines.contains(&2) && lines.contains(&4)); // uses_limit span
                assert!(!lines.contains(&5)); // unrelated not impacted
            }
            HybridImpact::WholeCrate => panic!("const change should localize"),
        }
    }

    #[test]
    fn function_body_change_uses_coverage_path() {
        // change inside unrelated (old line 6)
        let hunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![Hunk {
                old_start: 6,
                old_count: 1,
            }],
        )]);
        match analyze(&files(), &hunks) {
            HybridImpact::PerFile(impacts) => {
                let FileImpact::Lines(lines) = &impacts["src/lib.rs"] else {
                    panic!();
                };
                assert!(lines.contains(&5) && lines.contains(&6)); // unrelated span
            }
            HybridImpact::WholeCrate => panic!(),
        }
    }

    #[test]
    fn struct_change_falls_back_to_whole_crate() {
        // change old line 8 (the struct)
        let hunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![Hunk {
                old_start: 8,
                old_count: 1,
            }],
        )]);
        assert_eq!(analyze(&files(), &hunks), HybridImpact::WholeCrate);
    }
}
