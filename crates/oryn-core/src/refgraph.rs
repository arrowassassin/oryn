//! A crate-wide, over-approximate **item reference graph** (static analysis) for
//! the hybrid selector.
//!
//! Coverage (dynamic) can't see *non-execution* dependencies: a test that runs
//! `f` where `f` reads `const C` never "executes" `C`, so a change to `C` is
//! invisible to coverage. This graph fills that gap. We parse the base revision
//! with `syn` and record, for every item, the set of identifiers its tokens
//! reference. An edge `X → Y` exists when `X`'s text names `Y` — an
//! over-approximation (it may add spurious edges, but never drops a real one),
//! which is the safe direction for test selection.
//!
//! It is used only for the item kinds where name references are reliable
//! (`const`/`static`/`type`): you cannot use a const without naming it, and the
//! operator/trait sugar that hides names only affects *function* dispatch, which
//! the coverage path already handles. Structural changes to types/traits/impls
//! fall back to the whole crate elsewhere.

use proc_macro2::TokenTree;
use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use syn::spanned::Spanned;

/// Kind of a top-level (or nested) item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A function or method body.
    Fn,
    /// A `const`.
    Const,
    /// A `static`.
    Static,
    /// A `type` alias.
    TypeAlias,
    /// Anything else whose change we don't analyze finely (struct/enum/trait/
    /// impl header/macro/mod/use/…).
    Opaque,
}

/// A defined item with its source span and the names it references.
#[derive(Debug, Clone)]
pub struct Item {
    /// Defined name (method/impl items use the bare name).
    pub name: String,
    /// Kind.
    pub kind: ItemKind,
    /// Source file (repo-relative).
    pub file: String,
    /// First line (1-based, inclusive).
    pub start: usize,
    /// Last line.
    pub end: usize,
    /// Identifiers this item's tokens reference.
    pub refs: BTreeSet<String>,
}

/// The reference graph over a crate's items.
pub struct RefGraph {
    items: Vec<Item>,
    /// `referenced_by[j]` = items that reference item `j`.
    referenced_by: Vec<Vec<usize>>,
}

fn collect_idents(tokens: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
    for t in tokens {
        match t {
            TokenTree::Ident(i) => {
                out.insert(i.to_string());
            }
            TokenTree::Group(g) => collect_idents(g.stream(), out),
            _ => {}
        }
    }
}

fn refs_of<T: ToTokens>(node: &T) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    collect_idents(node.to_token_stream(), &mut s);
    s
}

fn push<T: ToTokens + Spanned>(
    out: &mut Vec<Item>,
    name: String,
    kind: ItemKind,
    file: &str,
    node: &T,
) {
    let span = node.span();
    let (start, end) = (span.start().line, span.end().line);
    if start >= 1 && end >= start {
        out.push(Item {
            name,
            kind,
            file: file.to_string(),
            start,
            end,
            refs: refs_of(node),
        });
    }
}

fn walk_items(items: &[syn::Item], file: &str, out: &mut Vec<Item>) {
    for it in items {
        match it {
            syn::Item::Fn(f) => push(out, f.sig.ident.to_string(), ItemKind::Fn, file, f),
            syn::Item::Const(c) => push(out, c.ident.to_string(), ItemKind::Const, file, c),
            syn::Item::Static(s) => push(out, s.ident.to_string(), ItemKind::Static, file, s),
            syn::Item::Type(t) => push(out, t.ident.to_string(), ItemKind::TypeAlias, file, t),
            syn::Item::Trait(t) => {
                push(out, t.ident.to_string(), ItemKind::Opaque, file, t);
                for ti in &t.items {
                    if let syn::TraitItem::Fn(m) = ti {
                        if m.default.is_some() {
                            push(out, m.sig.ident.to_string(), ItemKind::Fn, file, m);
                        }
                    }
                }
            }
            syn::Item::Impl(i) => {
                push(out, impl_name(i), ItemKind::Opaque, file, i);
                for ii in &i.items {
                    match ii {
                        syn::ImplItem::Fn(m) => {
                            push(out, m.sig.ident.to_string(), ItemKind::Fn, file, m)
                        }
                        syn::ImplItem::Const(c) => {
                            push(out, c.ident.to_string(), ItemKind::Const, file, c)
                        }
                        syn::ImplItem::Type(t) => {
                            push(out, t.ident.to_string(), ItemKind::TypeAlias, file, t)
                        }
                        _ => {}
                    }
                }
            }
            syn::Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    walk_items(items, file, out);
                }
            }
            syn::Item::Struct(s) => push(out, s.ident.to_string(), ItemKind::Opaque, file, s),
            syn::Item::Enum(e) => push(out, e.ident.to_string(), ItemKind::Opaque, file, e),
            syn::Item::Union(u) => push(out, u.ident.to_string(), ItemKind::Opaque, file, u),
            other => push(out, String::new(), ItemKind::Opaque, file, other),
        }
    }
}

fn impl_name(i: &syn::ItemImpl) -> String {
    if let syn::Type::Path(p) = &*i.self_ty {
        if let Some(seg) = p.path.segments.last() {
            return seg.ident.to_string();
        }
    }
    String::new()
}

impl RefGraph {
    /// Parse the items of a single file (used in tests and by `build`).
    #[must_use]
    pub fn parse_items(file: &str, src: &str) -> Vec<Item> {
        match syn::parse_file(src) {
            Ok(f) => {
                let mut out = Vec::new();
                walk_items(&f.items, file, &mut out);
                out
            }
            Err(_) => Vec::new(),
        }
    }

    /// Build the graph from `(file, source)` pairs of the base revision.
    #[must_use]
    pub fn build(files: &[(String, String)]) -> Self {
        let mut items = Vec::new();
        for (file, src) in files {
            items.extend(Self::parse_items(file, src));
        }
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, it) in items.iter().enumerate() {
            if !it.name.is_empty() {
                by_name.entry(it.name.clone()).or_default().push(i);
            }
        }
        let mut referenced_by = vec![Vec::new(); items.len()];
        for (i, it) in items.iter().enumerate() {
            for name in &it.refs {
                if let Some(targets) = by_name.get(name) {
                    for &j in targets {
                        if j != i {
                            referenced_by[j].push(i);
                        }
                    }
                }
            }
        }
        Self {
            items,
            referenced_by,
        }
    }

    /// All items.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Smallest `Fn` item containing `line` in `file`, if any.
    #[must_use]
    pub fn enclosing_fn(&self, file: &str, line: usize) -> Option<(usize, usize)> {
        self.items
            .iter()
            .filter(|it| {
                it.kind == ItemKind::Fn && it.file == file && it.start <= line && line <= it.end
            })
            .min_by_key(|it| it.end - it.start)
            .map(|it| (it.start, it.end))
    }

    /// Smallest non-`Fn` item containing `line` in `file` (index), if any.
    #[must_use]
    pub fn enclosing_nonfn(&self, file: &str, line: usize) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.kind != ItemKind::Fn && it.file == file && it.start <= line && line <= it.end
            })
            .min_by_key(|(_, it)| it.end - it.start)
            .map(|(i, _)| i)
    }

    /// Kind of item at index `i`.
    #[must_use]
    pub fn kind(&self, i: usize) -> ItemKind {
        self.items[i].kind
    }

    /// Functions that transitively reference any seed item — i.e. whose behavior
    /// may depend on a change to those items. Returns `(file, start, end)`.
    #[must_use]
    pub fn reverse_reachable_functions(
        &self,
        seed: &BTreeSet<usize>,
    ) -> Vec<(String, usize, usize)> {
        let mut seen: BTreeSet<usize> = seed.clone();
        let mut queue: VecDeque<usize> = seed.iter().copied().collect();
        let mut out = Vec::new();
        while let Some(n) = queue.pop_front() {
            for &dep in &self.referenced_by[n] {
                if seen.insert(dep) {
                    queue.push_back(dep);
                }
            }
        }
        for &i in &seen {
            let it = &self.items[i];
            if it.kind == ItemKind::Fn {
                out.push((it.file.clone(), it.start, it.end));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
const LIMIT: u32 = 10;\n\
const DERIVED: u32 = LIMIT + 1;\n\
fn uses_limit() -> u32 {\n    LIMIT * 2\n}\n\
fn uses_derived() -> u32 {\n    DERIVED\n}\n\
fn unrelated() -> u32 {\n    42\n}\n\
struct S { x: u32 }\n";

    fn graph() -> RefGraph {
        RefGraph::build(&[("src/lib.rs".to_string(), SRC.to_string())])
    }

    #[test]
    fn const_change_reaches_direct_and_transitive_functions() {
        let g = graph();
        let limit = g.items().iter().position(|i| i.name == "LIMIT").unwrap();
        let fns = g.reverse_reachable_functions(&BTreeSet::from([limit]));
        let names: Vec<_> = fns.iter().map(|(f, s, _)| (f.as_str(), *s)).collect();
        // uses_limit (direct) and uses_derived (via DERIVED) depend on LIMIT;
        // unrelated does not.
        let lines: Vec<usize> = names.iter().map(|(_, s)| *s).collect();
        // uses_limit starts line 3, uses_derived line 6.
        assert!(lines.contains(&3));
        assert!(lines.contains(&6));
        assert!(!lines.contains(&9)); // unrelated at line 9
    }

    #[test]
    fn classifies_item_kinds_and_enclosure() {
        let g = graph();
        // line 1 is the LIMIT const
        let c = g.enclosing_nonfn("src/lib.rs", 1).unwrap();
        assert_eq!(g.kind(c), ItemKind::Const);
        // line 4 is inside uses_limit
        assert_eq!(g.enclosing_fn("src/lib.rs", 4), Some((3, 5)));
        // the struct is opaque
        let s = g.enclosing_nonfn("src/lib.rs", 12).unwrap();
        assert_eq!(g.kind(s), ItemKind::Opaque);
    }

    #[test]
    fn method_in_impl_is_a_function() {
        let src =
            "struct T;\nimpl T {\n    fn m(&self) -> u32 { CONST_X }\n}\nconst CONST_X: u32 = 1;\n";
        let g = RefGraph::build(&[("a.rs".to_string(), src.to_string())]);
        let cx = g.items().iter().position(|i| i.name == "CONST_X").unwrap();
        let fns = g.reverse_reachable_functions(&BTreeSet::from([cx]));
        // method m references CONST_X
        assert!(fns.iter().any(|(_, s, _)| *s == 3));
    }
}
