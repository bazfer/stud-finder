//! A tiny dependency graph keyed by a file's "primary class name."
//!
//! Zeitwerk convention: a Ruby file at `app/models/order.rb` is expected
//! to define the constant `Order`. We encode that as a map
//! `class_name -> files that may define it`, derived here from each file's
//! basename (filename without extension, PascalCased).
//!
//! `fan_in(path)` then counts how many *other* files hold a reference
//! whose target class resolves to `path` via that map.
//!
//! Known first-slice limitations, to be lifted in follow-up slices:
//!
//! - Nested-namespace files (`app/models/order/line_item.rb` defining
//!   `Order::LineItem`) are represented only by their leaf class
//!   (`LineItem`). References to the full namespaced form miss.
//! - No Rails-root stripping. `lib/foo.rb` and `app/models/foo.rb` both
//!   register their basenames; there is no autoload-path concept yet.
//! - Multi-constant files only advertise their basename constant.
//! - No `require_relative` / `require` edge processing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A dependency graph built from a set of (file, outgoing references)
/// records. The graph owns both sides so callers can drop their source
/// after building.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    /// Map each primary class name → the files that may define it.
    defined_by: HashMap<String, Vec<PathBuf>>,
    /// Every file explicitly registered via `register_file`. Used by
    /// `fan_in` to refuse answering about unregistered paths even when
    /// their basename happens to match a registered file's primary class.
    registered: HashSet<PathBuf>,
    /// Map each file → the set of class names it references.
    outgoing: HashMap<PathBuf, HashSet<String>>,
    /// Map each file → the set of *registered file paths* it references
    /// explicitly, bypassing the Zeitwerk basename resolver. Used by
    /// path-based consumers (sf-cli's JS wiring) so distinct paths that
    /// share a basename (`a/index.js` vs `b/index.js`) do not collapse.
    path_outgoing: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file as defining its basename-derived primary class.
    /// Call this for every source file before adding references so that
    /// resolution has the full `defined_by` table in place.
    pub fn register_file(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        self.registered.insert(path.clone());
        if let Some(class) = primary_class_of(&path) {
            self.defined_by.entry(class).or_default().push(path);
        }
    }

    /// Record that `from` references `class_name`. Call once per
    /// reference; duplicates are deduped per-file by the internal
    /// `HashSet`, which matches the stud-finder semantics (we want the
    /// number of *files* that point at a target, not the raw mention
    /// count).
    pub fn add_reference(&mut self, from: impl Into<PathBuf>, class_name: impl Into<String>) {
        self.outgoing
            .entry(from.into())
            .or_default()
            .insert(class_name.into());
    }

    /// Record that `from` references the file at `to` by exact path,
    /// bypassing the Zeitwerk basename resolver. Used by consumers
    /// (sf-cli's JS wiring) where paths carry the identity rather than
    /// basenames. Duplicates are deduped per-file via the internal
    /// `HashSet`, matching the "number of files pointing at a target"
    /// semantics of `add_reference`. Self-references are stored but
    /// filtered out at `fan_in`/`fan_out` time, same as the class-name
    /// edges.
    pub fn add_path_edge(&mut self, from: impl Into<PathBuf>, to: impl Into<PathBuf>) {
        self.path_outgoing
            .entry(from.into())
            .or_default()
            .insert(to.into());
    }

    /// How many *other* files reference something the `path` defines.
    /// Returns 0 when `path` is unknown (not registered via
    /// `register_file`) or defines no class. An unregistered path that
    /// happens to share a basename with a registered file still returns
    /// 0, because it did not advertise itself as a definition site.
    pub fn fan_in(&self, path: &Path) -> usize {
        if !self.registered.contains(path) {
            return 0;
        }
        // Collect UNIQUE source paths across both edge types so a
        // file with BOTH a class-name edge AND a path-exact edge to
        // the same target counts exactly once. Fixes Ohm's
        // non-blocking double-count note on PR #81.
        let mut sources: HashSet<&Path> = HashSet::new();

        // Class-name edges (Ruby-style Zeitwerk resolution). The
        // two `if let`s are nested rather than a let-chain because
        // this workspace targets MSRV 1.85.
        #[allow(clippy::collapsible_if)]
        if let Some(class) = primary_class_of(path) {
            if let Some(defining) = self.defined_by.get(&class) {
                let defining_set: HashSet<&Path> = defining.iter().map(PathBuf::as_path).collect();
                for (from, refs) in &self.outgoing {
                    if !defining_set.contains(from.as_path()) && refs.contains(&class) {
                        sources.insert(from.as_path());
                    }
                }
            }
        }
        // Path-exact edges.
        for (from, targets) in &self.path_outgoing {
            if from.as_path() != path && targets.contains(path) {
                sources.insert(from.as_path());
            }
        }
        sources.len()
    }

    /// How many distinct outgoing class references `path` makes. Returns 0
    /// for an unregistered path, matching the symmetry contract with
    /// `fan_in` — the graph answers about explicitly registered files only.
    /// Duplicates are already deduped per-file in `add_reference`'s
    /// internal `HashSet`, so this is a count of *distinct* targets.
    pub fn fan_out(&self, path: &Path) -> usize {
        if !self.registered.contains(path) {
            return 0;
        }
        let class_edges = self.outgoing.get(path).map(|refs| refs.len()).unwrap_or(0);
        let path_edges = self
            .path_outgoing
            .get(path)
            .map(|targets| {
                // Don't count self-references toward fan_out either.
                targets.iter().filter(|t| t.as_path() != path).count()
            })
            .unwrap_or(0);
        class_edges + path_edges
    }

    /// Iterator over all registered files.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.registered.iter().map(PathBuf::as_path)
    }
}

/// Derive a primary class name from a file path using the Zeitwerk
/// filename convention: `foo_bar.rb` → `FooBar`.
fn primary_class_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }
    Some(pascalize(stem))
}

fn pascalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_to_class() {
        assert_eq!(
            primary_class_of(Path::new("app/models/order.rb")).as_deref(),
            Some("Order")
        );
        assert_eq!(
            primary_class_of(Path::new("app/services/payment_processor.rb")).as_deref(),
            Some("PaymentProcessor")
        );
    }

    #[test]
    fn fan_in_counts_referencing_files() {
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.register_file("app/models/user.rb");
        g.register_file("app/services/checkout.rb");

        g.add_reference("app/services/checkout.rb", "Order");
        g.add_reference("app/services/checkout.rb", "User");
        g.add_reference("app/models/user.rb", "Order");

        assert_eq!(g.fan_in(Path::new("app/models/order.rb")), 2);
        assert_eq!(g.fan_in(Path::new("app/models/user.rb")), 1);
        assert_eq!(g.fan_in(Path::new("app/services/checkout.rb")), 0);
    }

    #[test]
    fn fan_in_ignores_self_reference() {
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.add_reference("app/models/order.rb", "Order");
        assert_eq!(g.fan_in(Path::new("app/models/order.rb")), 0);
    }

    #[test]
    fn fan_in_deduplicates_per_file() {
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.register_file("app/services/checkout.rb");
        g.add_reference("app/services/checkout.rb", "Order");
        g.add_reference("app/services/checkout.rb", "Order");
        g.add_reference("app/services/checkout.rb", "Order");
        assert_eq!(g.fan_in(Path::new("app/models/order.rb")), 1);
    }

    #[test]
    fn fan_in_zero_for_unregistered_path_with_matching_basename() {
        // Ohm P3: an unregistered path that happens to share a basename
        // with a registered file must NOT inherit its fan_in. The graph
        // only answers about explicitly registered definition sites.
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.register_file("app/services/checkout.rb");
        g.add_reference("app/services/checkout.rb", "Order");
        assert_eq!(g.fan_in(Path::new("app/models/order.rb")), 1);
        assert_eq!(
            g.fan_in(Path::new("random/elsewhere/order.rb")),
            0,
            "unregistered path with matching basename must not get fan_in"
        );
    }

    #[test]
    fn fan_out_counts_distinct_outgoing_refs() {
        let mut g = DependencyGraph::new();
        g.register_file("app/services/checkout.rb");
        g.add_reference("app/services/checkout.rb", "Order");
        g.add_reference("app/services/checkout.rb", "User");
        g.add_reference("app/services/checkout.rb", "Order"); // duplicate
        assert_eq!(g.fan_out(Path::new("app/services/checkout.rb")), 2);
    }

    #[test]
    fn fan_out_zero_for_file_with_no_references() {
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        assert_eq!(g.fan_out(Path::new("app/models/order.rb")), 0);
    }

    #[test]
    fn fan_out_zero_for_unregistered_path() {
        let g = DependencyGraph::new();
        assert_eq!(g.fan_out(Path::new("some/random/path.rb")), 0);
    }

    #[test]
    fn path_edges_distinguish_same_basename() {
        // Two files share a basename. Path-exact edges must keep them
        // separate even though the Zeitwerk resolver would conflate
        // them under class "Index".
        let mut g = DependencyGraph::new();
        g.register_file("a/index.js");
        g.register_file("b/index.js");
        g.register_file("caller.js");

        g.add_path_edge("caller.js", "a/index.js");
        assert_eq!(g.fan_in(Path::new("a/index.js")), 1);
        assert_eq!(g.fan_in(Path::new("b/index.js")), 0);
        assert_eq!(g.fan_out(Path::new("caller.js")), 1);
    }

    #[test]
    fn path_edges_dedupe_per_file() {
        let mut g = DependencyGraph::new();
        g.register_file("target.js");
        g.register_file("caller.js");
        g.add_path_edge("caller.js", "target.js");
        g.add_path_edge("caller.js", "target.js");
        assert_eq!(g.fan_in(Path::new("target.js")), 1);
    }

    #[test]
    fn path_edge_self_reference_is_ignored() {
        let mut g = DependencyGraph::new();
        g.register_file("a.js");
        g.add_path_edge("a.js", "a.js");
        assert_eq!(g.fan_in(Path::new("a.js")), 0);
        assert_eq!(g.fan_out(Path::new("a.js")), 0);
    }

    #[test]
    fn class_and_path_edges_coexist_in_fan_in() {
        // A file can be targeted by BOTH class-name edges (Ruby) and
        // path-exact edges (JS). fan_in sums distinct contributors.
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.register_file("app/services/checkout.rb");
        g.register_file("scripts/batch.js");

        g.add_reference("app/services/checkout.rb", "Order");
        g.add_path_edge("scripts/batch.js", "app/models/order.rb");
        assert_eq!(g.fan_in(Path::new("app/models/order.rb")), 2);
    }

    #[test]
    fn files_iterates_all_registered() {
        let mut g = DependencyGraph::new();
        g.register_file("a.rb");
        g.register_file("b.rb");
        g.register_file("nested/c.rb");
        let mut files: Vec<_> = g.files().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();
        assert_eq!(files, vec!["a.rb", "b.rb", "nested/c.rb"]);
    }
}
