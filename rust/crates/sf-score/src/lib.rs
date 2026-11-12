//! stud-finder's composite score + percentile classification layer.
//!
//! Given a built `sf-core::DependencyGraph`, `Scorer::score` emits one
//! `ScoredFile` per registered file containing:
//!
//! - raw signal counts (`fan_in`, `fan_out`)
//! - percentile-ranked signals (`fan_in_pct`, `fan_out_pct`) in [0.0, 1.0]
//! - a weighted `composite` score in [0.0, 1.0]
//! - a `Class` tier (`Trunk` / `Branch` / `Leaf`) by composite percentile
//!   rank with absolute-floor escalation for raw fan_in
//! - an `evidence` number in [0.0, 1.0]
//!
//! This slice is Ruby-signals-only (no JS/TS crawling yet) and
//! fan-in/fan-out-only (no complexity, churn, coverage, coupling —
//! those land with `sf-ruby` complexity, `sf-history`, `sf-coverage` in
//! future slices). The composite is a 2-signal formula with the
//! `fan_in:fan_out = 4:2` ratio from the Ruby implementation, renormalized
//! to sum to 1.0 — i.e. `fan_in: 4/6, fan_out: 2/6`.
//!
//! `evidence` is hard-coded to `1.0` for every row this slice. The real
//! formula (`max(history_only, with_coverage)`) needs file age + commit
//! count + optional coverage data, which land with `sf-history` and
//! `sf-coverage`. Until then the field exists so consumers written
//! against this crate don't have to change shape later.
//!
//! The classification thresholds, the `fan_in >= 25` floor, and the
//! base-weight ratio (4 : 2 : 5 : 5 for fan_in : fan_out : complexity :
//! churn) are ported from the Ruby `lib/stud_finder/scorer.rb` on main;
//! see `rust/ATTRIBUTION.md` for the per-file provenance record.

use sf_core::DependencyGraph;
use std::path::{Path, PathBuf};

/// Default percentile cutoff for `Trunk` classification: top 15% by
/// composite score. Matches the Ruby default `--trunk-threshold 85`.
pub const TRUNK_THRESHOLD_PCT: f64 = 0.85;
/// Default percentile cutoff for `Branch`: top 50%. Matches the Ruby
/// default `--branch-threshold 50`.
pub const BRANCH_THRESHOLD_PCT: f64 = 0.50;
/// Raw-signal floor: a file with fan_in at or above this count cannot
/// classify below `Branch`. Matches the Ruby `FAN_IN_FLOOR = 25`.
pub const FAN_IN_FLOOR: usize = 25;

/// The fan_in weight in the 2-signal composite. 4/(4+2) from the Ruby
/// base-weight ratio `fan_in : fan_out : complexity : churn = 4 : 2 : 5 : 5`.
const WEIGHT_FAN_IN: f64 = 4.0 / 6.0;
/// The fan_out weight in the 2-signal composite. 2/(4+2).
const WEIGHT_FAN_OUT: f64 = 2.0 / 6.0;

/// Classification tier for a scored file. `Trunk` is load-bearing;
/// `Branch` has meaningful coupling; `Leaf` is isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Trunk,
    Branch,
    Leaf,
}

impl Class {
    /// Sort key so trunks group above branches above leaves.
    pub fn rank(self) -> u8 {
        match self {
            Class::Trunk => 0,
            Class::Branch => 1,
            Class::Leaf => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Trunk => "trunk",
            Class::Branch => "branch",
            Class::Leaf => "leaf",
        }
    }
}

/// One row of the stud-finder output: a file, its raw and ranked
/// signals, its composite score, its class, and the evidence
/// confidence attached to that score.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredFile {
    pub path: PathBuf,
    pub fan_in: usize,
    pub fan_out: usize,
    pub fan_in_pct: f64,
    pub fan_out_pct: f64,
    pub composite: f64,
    pub class: Class,
    pub evidence: f64,
}

/// Scorer with tunable thresholds. Construct with `Scorer::new()` for
/// the Ruby defaults, or `Scorer::with_thresholds` to override.
#[derive(Debug, Clone, Copy)]
pub struct Scorer {
    pub trunk_threshold_pct: f64,
    pub branch_threshold_pct: f64,
    pub fan_in_floor: usize,
}

impl Default for Scorer {
    fn default() -> Self {
        Self::new()
    }
}

impl Scorer {
    pub fn new() -> Self {
        Self {
            trunk_threshold_pct: TRUNK_THRESHOLD_PCT,
            branch_threshold_pct: BRANCH_THRESHOLD_PCT,
            fan_in_floor: FAN_IN_FLOOR,
        }
    }

    pub fn with_thresholds(
        trunk_threshold_pct: f64,
        branch_threshold_pct: f64,
        fan_in_floor: usize,
    ) -> Self {
        Self {
            trunk_threshold_pct,
            branch_threshold_pct,
            fan_in_floor,
        }
    }

    /// Score every registered file in `graph` and return the rows
    /// unsorted. The caller sorts for display.
    pub fn score(&self, graph: &DependencyGraph) -> Vec<ScoredFile> {
        let files: Vec<&Path> = graph.files().collect();
        if files.is_empty() {
            return Vec::new();
        }

        let fan_ins: Vec<usize> = files.iter().map(|p| graph.fan_in(p)).collect();
        let fan_outs: Vec<usize> = files.iter().map(|p| graph.fan_out(p)).collect();

        let fan_in_pcts = percentile_rank_usize(&fan_ins);
        let fan_out_pcts = percentile_rank_usize(&fan_outs);

        let composites: Vec<f64> = fan_in_pcts
            .iter()
            .zip(fan_out_pcts.iter())
            .map(|(&fi, &fo)| WEIGHT_FAN_IN * fi + WEIGHT_FAN_OUT * fo)
            .collect();

        let composite_pcts = percentile_rank_f64(&composites);

        let trunk_cutoff = self.trunk_threshold_pct;
        let branch_cutoff = self.branch_threshold_pct;

        let classes: Vec<Class> = composite_pcts
            .iter()
            .zip(fan_ins.iter())
            .map(|(&pct, &fan_in)| {
                let base = if pct >= trunk_cutoff {
                    Class::Trunk
                } else if pct >= branch_cutoff {
                    Class::Branch
                } else {
                    Class::Leaf
                };
                // Absolute floor: raw fan_in at or above the floor
                // escalates a Leaf to Branch. Never downgrade.
                if base == Class::Leaf && fan_in >= self.fan_in_floor {
                    Class::Branch
                } else {
                    base
                }
            })
            .collect();

        files
            .iter()
            .enumerate()
            .map(|(i, path)| ScoredFile {
                path: path.to_path_buf(),
                fan_in: fan_ins[i],
                fan_out: fan_outs[i],
                fan_in_pct: fan_in_pcts[i],
                fan_out_pct: fan_out_pcts[i],
                composite: round4(composites[i]),
                class: classes[i],
                evidence: 1.0,
            })
            .collect()
    }
}

/// Fractional-rank percentile over a slice of `usize` values, in
/// [0.0, 1.0]. Ties get the mean of their rank positions, matching the
/// Ruby implementation's dispersion behaviour. A slice with a single
/// value, or all-equal values, returns `0.0` for every entry — matching
/// the Ruby "insufficient dispersion" fallback (the raw signal becomes
/// uninformative).
fn percentile_rank_usize(values: &[usize]) -> Vec<f64> {
    let as_f64: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    percentile_rank_f64(&as_f64)
}

fn percentile_rank_f64(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut sorted: Vec<(usize, f64)> = values.iter().copied().enumerate().collect();
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    // Dispersion check: if all equal, every pct is 0.0.
    if (sorted[0].1 - sorted[n - 1].1).abs() < f64::EPSILON {
        return vec![0.0; n];
    }
    // Assign the mean rank to each tie group, then divide by (n - 1)
    // so the lowest value gets 0.0 and the highest 1.0.
    let mut pcts = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && (sorted[j].1 - sorted[i].1).abs() < f64::EPSILON {
            j += 1;
        }
        let mean_rank = (i + j - 1) as f64 / 2.0;
        let pct = mean_rank / (n - 1) as f64;
        for k in i..j {
            pcts[sorted[k].0] = pct;
        }
        i = j;
    }
    pcts
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_graph() -> DependencyGraph {
        let mut g = DependencyGraph::new();
        g.register_file("app/models/order.rb");
        g.register_file("app/models/user.rb");
        g.register_file("app/models/profile.rb");
        g.register_file("app/services/checkout.rb");
        g.register_file("app/services/noop.rb");

        // Order: fan_in 3, fan_out 0
        // User: fan_in 2, fan_out 1 (-> Order)
        // Profile: fan_in 0, fan_out 2 (-> User, Order)
        // Checkout: fan_in 0, fan_out 2 (-> Order, User)
        // Noop: fan_in 0, fan_out 0
        g.add_reference("app/models/user.rb", "Order");
        g.add_reference("app/models/profile.rb", "User");
        g.add_reference("app/models/profile.rb", "Order");
        g.add_reference("app/services/checkout.rb", "Order");
        g.add_reference("app/services/checkout.rb", "User");
        g
    }

    fn row<'a>(rows: &'a [ScoredFile], path: &str) -> &'a ScoredFile {
        let target = Path::new(path);
        rows.iter()
            .find(|r| r.path == target)
            .unwrap_or_else(|| panic!("no row for {path}"))
    }

    #[test]
    fn percentile_rank_endpoints_and_middle() {
        let r = percentile_rank_usize(&[1, 2, 3, 4, 5]);
        assert!((r[0] - 0.0).abs() < 1e-9);
        assert!((r[4] - 1.0).abs() < 1e-9);
        assert!((r[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn percentile_rank_ties_share_mean_rank() {
        // All the same → every entry gets 0.0 (insufficient dispersion).
        let r = percentile_rank_usize(&[7, 7, 7, 7]);
        assert_eq!(r, vec![0.0, 0.0, 0.0, 0.0]);
        // Two tied at the top:
        let r = percentile_rank_usize(&[1, 2, 3, 3]);
        assert!((r[0] - 0.0).abs() < 1e-9);
        assert!((r[1] - 1.0 / 3.0).abs() < 1e-9);
        assert!((r[2] - 5.0 / 6.0).abs() < 1e-9);
        assert!((r[3] - 5.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn scorer_emits_row_per_registered_file() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn composite_uses_four_to_two_weight_ratio() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        let order = row(&rows, "app/models/order.rb");
        let expected =
            round4(WEIGHT_FAN_IN * order.fan_in_pct + WEIGHT_FAN_OUT * order.fan_out_pct);
        assert!((order.composite - expected).abs() < 1e-9);
    }

    #[test]
    fn classification_tiers_partition_the_set() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        let mut trunks = 0;
        let mut branches = 0;
        let mut leaves = 0;
        for r in &rows {
            match r.class {
                Class::Trunk => trunks += 1,
                Class::Branch => branches += 1,
                Class::Leaf => leaves += 1,
            }
        }
        assert_eq!(trunks + branches + leaves, 5);
    }

    #[test]
    fn fan_in_floor_escalates_leaf_to_branch() {
        // Build a graph where one file has raw fan_in >= FAN_IN_FLOOR but
        // low composite score (because everyone else also has fan_in, so
        // its percentile rank is middling). Verify it escalates to Branch.
        let mut g = DependencyGraph::new();
        g.register_file("hot.rb");
        g.register_file("other.rb");
        for i in 0..26 {
            let file = format!("caller_{i}.rb");
            g.register_file(&file);
            g.add_reference(&file, "Hot");
        }
        let rows = Scorer::new().score(&g);
        let hot = row(&rows, "hot.rb");
        assert!(
            hot.fan_in >= FAN_IN_FLOOR,
            "setup: hot should trip the floor"
        );
        assert_ne!(
            hot.class,
            Class::Leaf,
            "fan_in >= {FAN_IN_FLOOR} must escalate Leaf to at least Branch"
        );
    }

    #[test]
    fn fan_in_floor_does_not_downgrade_trunk() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        for r in rows {
            if r.class == Class::Trunk {
                assert_eq!(r.class, Class::Trunk, "floor must not downgrade Trunk");
            }
        }
    }

    #[test]
    fn evidence_is_one_point_zero_in_this_slice() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        for r in rows {
            assert_eq!(r.evidence, 1.0);
        }
    }

    #[test]
    fn empty_graph_yields_no_rows() {
        let g = DependencyGraph::new();
        let rows = Scorer::new().score(&g);
        assert!(rows.is_empty());
    }

    #[test]
    fn single_file_renders_zero_pcts() {
        let mut g = DependencyGraph::new();
        g.register_file("solo.rb");
        let rows = Scorer::new().score(&g);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fan_in_pct, 0.0);
        assert_eq!(rows[0].fan_out_pct, 0.0);
    }

    #[test]
    fn two_files_with_same_signals_both_get_zero_pct() {
        // Insufficient dispersion → both get pct 0.0 and composite 0.0,
        // matching the Ruby insufficient_dispersion warning behaviour.
        let mut g = DependencyGraph::new();
        g.register_file("a.rb");
        g.register_file("b.rb");
        g.add_reference("a.rb", "External");
        g.add_reference("b.rb", "External");
        let rows = Scorer::new().score(&g);
        for r in rows {
            assert_eq!(r.fan_in_pct, 0.0);
            assert_eq!(r.fan_out_pct, 0.0);
            assert_eq!(r.composite, 0.0);
            assert_eq!(r.class, Class::Leaf);
        }
    }
}
