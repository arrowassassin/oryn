//! Function spans (line ranges) of a source file, and the per-file *impact* of a
//! set of diff hunks — at function granularity.
//!
//! Why function granularity: raw line matching is unsound under insertions — a
//! few lines inserted into a covered function flag no *old* line as changed. By
//! mapping every touched old line to its enclosing function's full span, an edit
//! anywhere inside a function correctly impacts every test that executed any part
//! of it. Changes *outside* every function (a `struct`, `const`, `use`, a macro
//! at item position) are conservatively treated as impacting the whole file.

use crate::difflines::Hunk;
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

/// Inclusive 1-based line range of a function body item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnSpan {
    /// First line.
    pub start: usize,
    /// Last line.
    pub end: usize,
}

#[derive(Default)]
struct Collector {
    spans: Vec<FnSpan>,
}

impl Collector {
    fn push<S: Spanned>(&mut self, node: &S) {
        let span = node.span();
        let (s, e) = (span.start().line, span.end().line);
        if s >= 1 && e >= s {
            self.spans.push(FnSpan { start: s, end: e });
        }
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_fn(&mut self, n: &'ast syn::ItemFn) {
        self.push(n);
        visit::visit_item_fn(self, n);
    }
    fn visit_impl_item_fn(&mut self, n: &'ast syn::ImplItemFn) {
        self.push(n);
        visit::visit_impl_item_fn(self, n);
    }
    fn visit_trait_item_fn(&mut self, n: &'ast syn::TraitItemFn) {
        if n.default.is_some() {
            self.push(n);
        }
        visit::visit_trait_item_fn(self, n);
    }
}

/// All function-body spans in `src` (free fns, impl methods, trait default
/// methods). Returns an empty vec if the source cannot be parsed — callers must
/// treat "no spans" conservatively.
#[must_use]
pub fn function_spans(src: &str) -> Vec<FnSpan> {
    let Ok(file) = syn::parse_file(src) else {
        return Vec::new();
    };
    let mut c = Collector::default();
    c.visit_file(&file);
    c.spans.sort_by_key(|s| (s.start, s.end));
    c.spans
}

/// The impact of a file's hunks: either the whole file (a change outside any
/// function, or an unparseable file) or a precise set of impacted old lines
/// (each touched line expanded to its enclosing function's full span).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileImpact {
    /// Conservatively rerun every test that executed any line of this file.
    Whole,
    /// Rerun a test only if it executed one of these old lines.
    Lines(BTreeSet<usize>),
}

/// Compute the impact of `hunks` against the *base* source `src_old`.
#[must_use]
pub fn impact(src_old: &str, hunks: &[Hunk]) -> FileImpact {
    if hunks.is_empty() {
        return FileImpact::Lines(BTreeSet::new());
    }
    let spans = function_spans(src_old);
    let mut lines = BTreeSet::new();
    for h in hunks {
        let (a, b) = h.touched_old_lines();
        for line in a..=b {
            let mut enclosed = false;
            for s in &spans {
                if line >= s.start && line <= s.end {
                    lines.extend(s.start..=s.end);
                    enclosed = true;
                }
            }
            if !enclosed {
                // Change outside any function body — can't localize safely.
                return FileImpact::Whole;
            }
        }
    }
    FileImpact::Lines(lines)
}

/// Does a test that executed `covered` old lines need to rerun under `impact`?
#[must_use]
pub fn intersects(impact: &FileImpact, covered: &BTreeSet<usize>) -> bool {
    match impact {
        FileImpact::Whole => true,
        FileImpact::Lines(lines) => lines.intersection(covered).next().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "fn a() {\n    let x = 1;\n    let _ = x;\n}\n\
fn b() {\n    foo();\n    bar();\n}\n\
const C: u32 = 1;\n";

    #[test]
    fn finds_function_spans() {
        let spans = function_spans(SRC);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0], FnSpan { start: 1, end: 4 }); // fn a
        assert_eq!(spans[1], FnSpan { start: 5, end: 8 }); // fn b
    }

    #[test]
    fn modification_inside_function_impacts_whole_function() {
        // change old line 6 (inside b) -> impacts all of b (5..=8)
        let imp = impact(
            SRC,
            &[Hunk {
                old_start: 6,
                old_count: 1,
            }],
        );
        match imp {
            FileImpact::Lines(l) => {
                assert!(l.contains(&5) && l.contains(&8));
                assert!(!l.contains(&2)); // fn a untouched
            }
            FileImpact::Whole => panic!("should be localized"),
        }
    }

    #[test]
    fn insertion_inside_function_is_caught() {
        // pure insertion after old line 6 -> touches lines 6,7 (both in b)
        let imp = impact(
            SRC,
            &[Hunk {
                old_start: 6,
                old_count: 0,
            }],
        );
        match imp {
            FileImpact::Lines(l) => assert!(l.contains(&5) && l.contains(&8)),
            FileImpact::Whole => panic!("insertion inside fn should be localized to the fn"),
        }
    }

    #[test]
    fn change_outside_any_function_is_whole_file() {
        // old line 9 is the const, not in any fn
        assert_eq!(
            impact(
                SRC,
                &[Hunk {
                    old_start: 9,
                    old_count: 1
                }]
            ),
            FileImpact::Whole
        );
    }

    #[test]
    fn unparseable_source_is_whole_file() {
        assert_eq!(
            impact(
                "fn broken( {",
                &[Hunk {
                    old_start: 1,
                    old_count: 1
                }]
            ),
            FileImpact::Whole
        );
    }

    #[test]
    fn intersection_logic() {
        let imp = FileImpact::Lines(BTreeSet::from([5, 6, 7, 8]));
        assert!(intersects(&imp, &BTreeSet::from([6])));
        assert!(!intersects(&imp, &BTreeSet::from([2, 3])));
        assert!(intersects(&FileImpact::Whole, &BTreeSet::new()));
    }
}
