//! stud-finder's composite score + percentile classification layer.
//!
//! Given a `Signals` bundle (a `sf-core::DependencyGraph` plus optional
//! per-file auxiliary maps), `Scorer::score_with_signals` emits one
//! `ScoredFile` per registered file containing:
//!
//! - raw signal counts (`fan_in`, `fan_out`, `complexity`)
//! - percentile-ranked signals (`fan_in_pct`, `fan_out_pct`, `complexity_pct`)
//!   in [0.0, 1.0]
//! - a weighted `composite` score in [0.0, 1.0]
//! - a `Class` tier (`Trunk` / `Branch` / `Leaf`) by composite percentile
//!   rank with absolute-floor escalation for raw fan_in and complexity
//! - an `evidence` number in [0.0, 1.0]
//!
//! Weights follow the Ruby base-weight ratio
//! `fan_in : fan_out : complexity : churn = 4 : 2 : 5 : 5`, renormalized
//! over the signals that are available in the `Signals` bundle (so weights
//! always sum to 1.0). fan_in and fan_out are always available from the
//! graph. `complexity` is available when the complexity map is non-empty.
//! churn, coverage, and coupling land with sf-history / sf-coverage in
//! later slices and extend the same renormalization.
//!
//! `evidence` is hard-coded to `1.0` for every row until sf-history +
//! sf-coverage land. The real formula is
//! `max(history_only, with_coverage)` on file age + commit count +
//! optional coverage.
//!
//! The classification thresholds, the `fan_in >= 25` and `complexity >= 15`
//! floors, and the base-weight ratio are ported from the Ruby
//! `lib/stud_finder/scorer.rb` on main. See `rust/ATTRIBUTION.md` for the
//! per-file provenance record.

use sf_core::DependencyGraph;
use std::collections::HashMap;
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

/// Raw-signal floor: a file whose highest method complexity is at or
/// above this value cannot classify below `Branch`. Matches the Ruby
/// `COMPLEXITY_FLOOR = 15`.
pub const COMPLEXITY_FLOOR: usize = 15;

/// Base-weight ratios from the Ruby implementation:
/// fan_in : fan_out : complexity : churn = 4 : 2 : 5 : 5. Only the
/// available signals contribute; weights are renormalized over them so
/// the active weights always sum to 1.0.
const BASE_WEIGHT_FAN_IN: f64 = 4.0;
const BASE_WEIGHT_FAN_OUT: f64 = 2.0;
const BASE_WEIGHT_COMPLEXITY: f64 = 5.0;

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
    pub complexity: usize,
    pub fan_in_pct: f64,
    pub fan_out_pct: f64,
    pub complexity_pct: f64,
    pub composite: f64,
    pub class: Class,
    pub evidence: f64,
}

/// Bundle of per-file signal data the scorer consumes. `graph` is
/// mandatory; other maps are optional and only fill out the composite
/// when non-empty. Signals extend this struct as new slices land
/// (sf-history adds churn + coupling, sf-coverage adds coverage +
/// interaction).
pub struct Signals<'a> {
    pub graph: &'a DependencyGraph,
    /// Max cyclomatic complexity per file (from `sf-ruby::max_complexity`).
    /// Empty = complexity signal is unavailable and drops out of the
    /// composite. Missing keys inside a non-empty map default to 0.
    pub complexity: HashMap<PathBuf, usize>,
}

impl<'a> Signals<'a> {
    /// Signals bundle with just the graph — no complexity, no churn,
    /// no coverage. Matches the sf-score first-slice shape.
    pub fn from_graph(graph: &'a DependencyGraph) -> Self {
        Self {
            graph,
            complexity: HashMap::new(),
        }
    }
}

/// Scorer with tunable thresholds. Construct with `Scorer::new()` for
/// the Ruby defaults, or `Scorer::with_thresholds` to override.
#[derive(Debug, Clone, Copy)]
pub struct Scorer {
    pub trunk_threshold_pct: f64,
    pub branch_threshold_pct: f64,
    pub fan_in_floor: usize,
    pub complexity_floor: usize,
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
            complexity_floor: COMPLEXITY_FLOOR,
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
            complexity_floor: COMPLEXITY_FLOOR,
        }
    }

    pub fn with_floors(
        trunk_threshold_pct: f64,
        branch_threshold_pct: f64,
        fan_in_floor: usize,
        complexity_floor: usize,
    ) -> Self {
        Self {
            trunk_threshold_pct,
            branch_threshold_pct,
            fan_in_floor,
            complexity_floor,
        }
    }

    /// Score every registered file in `graph` using fan_in and fan_out
    /// only. Equivalent to `score_with_signals(Signals::from_graph(graph))`.
    /// Kept as a convenience for consumers that do not have complexity
    /// data.
    pub fn score(&self, graph: &DependencyGraph) -> Vec<ScoredFile> {
        self.score_with_signals(Signals::from_graph(graph))
    }

    /// Score every registered file in `signals.graph` with every
    /// available signal. Returns rows unsorted — the caller sorts for
    /// display.
    pub fn score_with_signals(&self, signals: Signals<'_>) -> Vec<ScoredFile> {
        let files: Vec<&Path> = signals.graph.files().collect();
        if files.is_empty() {
            return Vec::new();
        }

        let fan_ins: Vec<usize> = files.iter().map(|p| signals.graph.fan_in(p)).collect();
        let fan_outs: Vec<usize> = files.iter().map(|p| signals.graph.fan_out(p)).collect();

        let has_complexity = !signals.complexity.is_empty();
        let complexities: Vec<usize> = files
            .iter()
            .map(|p| signals.complexity.get(*p).copied().unwrap_or(0))
            .collect();

        let fan_in_pcts = percentile_rank_usize(&fan_ins);
        let fan_out_pcts = percentile_rank_usize(&fan_outs);
        let complexity_pcts = if has_complexity {
            percentile_rank_usize(&complexities)
        } else {
            vec![0.0; files.len()]
        };

        // Base weights for the signals that are active.
        let mut weight_sum = BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT;
        if has_complexity {
            weight_sum += BASE_WEIGHT_COMPLEXITY;
        }
        let w_fi = BASE_WEIGHT_FAN_IN / weight_sum;
        let w_fo = BASE_WEIGHT_FAN_OUT / weight_sum;
        let w_cx = if has_complexity {
            BASE_WEIGHT_COMPLEXITY / weight_sum
        } else {
            0.0
        };

        let composites: Vec<f64> = (0..files.len())
            .map(|i| w_fi * fan_in_pcts[i] + w_fo * fan_out_pcts[i] + w_cx * complexity_pcts[i])
            .collect();

        let composite_pcts = percentile_rank_f64(&composites);

        let trunk_cutoff = self.trunk_threshold_pct;
        let branch_cutoff = self.branch_threshold_pct;
        let fan_in_floor = self.fan_in_floor;
        let complexity_floor = self.complexity_floor;

        let classes: Vec<Class> = (0..files.len())
            .map(|i| {
                let pct = composite_pcts[i];
                let base = if pct >= trunk_cutoff {
                    Class::Trunk
                } else if pct >= branch_cutoff {
                    Class::Branch
                } else {
                    Class::Leaf
                };
                // Absolute floors: a file whose raw fan_in or
                // complexity meets or exceeds the respective threshold
                // cannot classify below Branch. Floors only escalate —
                // Trunk and Branch are never downgraded.
                if base == Class::Leaf
                    && (fan_ins[i] >= fan_in_floor
                        || (has_complexity && complexities[i] >= complexity_floor))
                {
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
                complexity: complexities[i],
                fan_in_pct: fan_in_pcts[i],
                fan_out_pct: fan_out_pcts[i],
                complexity_pct: complexity_pcts[i],
                composite: round4(composites[i]),
                class: classes[i],
                evidence: 1.0,
            })
            .collect()
    }
}

/// Lower-bound fractional-rank percentile over a slice of `usize`
/// values, in [0.0, 1.0]. For each value, the rank is the number of
/// *strictly smaller* values divided by `(n - 1)` — so ties all share
/// the lower end of their rank band, matching the Ruby implementation's
/// `Normalizer.percentile_rank` + `lower_bound` on `lib/stud_finder/normalizer.rb`
/// on main (cross-checked when sf-score was ported). A slice with a
/// single value, or all-equal values, returns `0.0` for every entry —
/// matching the Ruby "insufficient dispersion" fallback (the raw
/// signal becomes uninformative).
fn percentile_rank_usize(values: &[usize]) -> Vec<f64> {
    let as_f64: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    percentile_rank_f64(&as_f64)
}

fn percentile_rank_f64(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Dispersion check: if all equal, every pct is 0.0.
    if (sorted[0] - sorted[n - 1]).abs() < f64::EPSILON {
        return vec![0.0; n];
    }
    let denom = (n - 1) as f64;
    values
        .iter()
        .map(|&v| lower_bound(&sorted, v) as f64 / denom)
        .collect()
}

/// First index in `sorted` whose value is NOT strictly less than
/// `target`. Equivalent to Ruby's `Normalizer.lower_bound` — counts
/// strictly-smaller entries, giving every tie group the same rank at
/// the low end of their band.
fn lower_bound(sorted: &[f64], target: f64) -> usize {
    let mut low = 0;
    let mut high = sorted.len();
    while low < high {
        let mid = (low + high) / 2;
        if sorted[mid] < target {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
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
    fn percentile_rank_ties_use_lower_bound_matching_ruby() {
        // All the same → every entry gets 0.0 (insufficient dispersion).
        let r = percentile_rank_usize(&[7, 7, 7, 7]);
        assert_eq!(r, vec![0.0, 0.0, 0.0, 0.0]);
        // Ohm's P1 example: [0, 0, 1] → [0.0, 0.0, 1.0] (lower bound),
        // NOT [0.25, 0.25, 1.0] (mean rank).
        let r = percentile_rank_usize(&[0, 0, 1]);
        assert_eq!(r, vec![0.0, 0.0, 1.0]);
        // Two tied at the top: `[1, 2, 3, 3]` with lower_bound:
        //   1 → 0 strictly smaller → 0/3
        //   2 → 1 strictly smaller → 1/3
        //   3 → 2 strictly smaller → 2/3 (both ties)
        let r = percentile_rank_usize(&[1, 2, 3, 3]);
        assert!((r[0] - 0.0).abs() < 1e-9);
        assert!((r[1] - 1.0 / 3.0).abs() < 1e-9);
        assert!((r[2] - 2.0 / 3.0).abs() < 1e-9);
        assert!((r[3] - 2.0 / 3.0).abs() < 1e-9);
        // Ties at the bottom: `[1, 1, 2, 3]`:
        //   1 → 0 strictly smaller → 0/3 (both ties)
        //   2 → 2 strictly smaller → 2/3
        //   3 → 3 strictly smaller → 1.0
        let r = percentile_rank_usize(&[1, 1, 2, 3]);
        assert!((r[0] - 0.0).abs() < 1e-9);
        assert!((r[1] - 0.0).abs() < 1e-9);
        assert!((r[2] - 2.0 / 3.0).abs() < 1e-9);
        assert!((r[3] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn scorer_emits_row_per_registered_file() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn composite_uses_four_to_two_weight_ratio_in_two_signal_mode() {
        let g = tiny_graph();
        let rows = Scorer::new().score(&g);
        let order = row(&rows, "app/models/order.rb");
        let w_fi = BASE_WEIGHT_FAN_IN / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT);
        let w_fo = BASE_WEIGHT_FAN_OUT / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT);
        let expected = round4(w_fi * order.fan_in_pct + w_fo * order.fan_out_pct);
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

    /// Build a graph where `hot.rb` has raw fan_in >= `FAN_IN_FLOOR`
    /// AND a composite percentile low enough to classify as `Leaf`
    /// without the floor. That's the only configuration the floor rule
    /// is for — a visibly dangerous file flattened by percentile-rank
    /// compression needs the absolute threshold to escape `Leaf`.
    ///
    /// Construction: 100 peer files each with fan_in = fan_out = 50
    /// (via a mutual-reference ring). `hot.rb` has fan_in = 30 (over
    /// the floor) but fan_out = 0. That drops hot's fan_in and fan_out
    /// percentiles below the peer majority, so its composite lands in
    /// the bottom 50% → `Leaf` without the floor, `Branch` with.
    fn floor_test_graph() -> DependencyGraph {
        let mut g = DependencyGraph::new();
        let peer_count = 100usize;
        for i in 0..peer_count {
            g.register_file(format!("peer_{i}.rb"));
        }
        for i in 0..peer_count {
            for offset in 1..=50 {
                let target_class = format!("Peer{}", (i + offset) % peer_count);
                g.add_reference(format!("peer_{i}.rb"), target_class);
            }
        }
        g.register_file("hot.rb");
        for i in 0..30 {
            g.add_reference(format!("peer_{i}.rb"), "Hot");
        }
        g
    }

    #[test]
    fn fan_in_floor_escalates_leaf_to_branch_when_composite_is_low() {
        let g = floor_test_graph();

        // Baseline: same graph scored with the floor effectively
        // disabled (`fan_in_floor = usize::MAX`). Hot should classify
        // as Leaf by composite percentile — without that baseline, the
        // "escalates" assertion could pass vacuously on a file that was
        // already above Leaf.
        let no_floor =
            Scorer::with_thresholds(TRUNK_THRESHOLD_PCT, BRANCH_THRESHOLD_PCT, usize::MAX);
        let rows_no_floor = no_floor.score(&g);
        let hot_no_floor = row(&rows_no_floor, "hot.rb");
        assert!(
            hot_no_floor.fan_in >= FAN_IN_FLOOR,
            "setup: hot should meet the floor count ({} >= {})",
            hot_no_floor.fan_in,
            FAN_IN_FLOOR
        );
        assert_eq!(
            hot_no_floor.class,
            Class::Leaf,
            "setup: without floor, hot should classify as Leaf by composite percentile"
        );

        // With the default floor, the same graph escalates hot to Branch.
        let default = Scorer::new();
        let rows_default = default.score(&g);
        let hot_default = row(&rows_default, "hot.rb");
        assert_eq!(
            hot_default.class,
            Class::Branch,
            "fan_in >= {FAN_IN_FLOOR} must escalate Leaf to Branch"
        );
    }

    #[test]
    fn fan_in_floor_never_downgrades_a_trunk_that_meets_the_count() {
        // A file that is Trunk by composite AND meets the floor count
        // must stay Trunk — the floor only ever escalates Leaf, never
        // touches Branch or Trunk.
        //
        // Construction: `top.rb` has the maximum fan_in (30, over the
        // floor) AND the maximum fan_out (30 outgoing refs to Dep1..Dep30).
        // 30 "caller" files each reference Top and nothing else so they
        // bring fan_in up without matching top's fan_out. This puts top
        // at (fan_in=30, fan_out=30) — both percentile maxes — so
        // composite is at the top → Trunk unambiguously.
        let mut g = DependencyGraph::new();
        g.register_file("top.rb");
        for i in 0..30 {
            g.add_reference("top.rb", format!("Dep{i}"));
        }
        for i in 0..30 {
            let f = format!("caller_{i}.rb");
            g.register_file(&f);
            g.add_reference(&f, "Top");
        }

        // Baseline with floor disabled: top is Trunk by composite alone.
        let no_floor =
            Scorer::with_thresholds(TRUNK_THRESHOLD_PCT, BRANCH_THRESHOLD_PCT, usize::MAX);
        let top_no_floor = row(&no_floor.score(&g), "top.rb").clone();
        assert!(
            top_no_floor.fan_in >= FAN_IN_FLOOR,
            "setup: top should meet the floor count ({} >= {})",
            top_no_floor.fan_in,
            FAN_IN_FLOOR
        );
        assert_eq!(
            top_no_floor.class,
            Class::Trunk,
            "setup: top should be Trunk by composite, independent of the floor"
        );

        // Default scorer runs the floor rule; top still Trunk.
        let top_default = row(&Scorer::new().score(&g), "top.rb").clone();
        assert_eq!(
            top_default.class,
            Class::Trunk,
            "floor must not downgrade a Trunk even when it meets the floor count"
        );
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

    // --- 3-signal mode with complexity ---

    fn signals_with_complexity<'a>(
        graph: &'a DependencyGraph,
        complexities: &[(&str, usize)],
    ) -> Signals<'a> {
        let mut cx = HashMap::new();
        for (p, c) in complexities {
            cx.insert(PathBuf::from(*p), *c);
        }
        Signals {
            graph,
            complexity: cx,
        }
    }

    #[test]
    fn three_signal_renormalization_weights_sum_to_one() {
        // With fan_in, fan_out, AND complexity active, the base ratios
        // 4 : 2 : 5 renormalize to {4/11, 2/11, 5/11}. Verify by scoring
        // a graph whose fan_in/fan_out/complexity percentiles are all
        // 1.0 for one file: its composite must be exactly 1.0.
        let mut g = DependencyGraph::new();
        g.register_file("top.rb");
        for i in 0..5 {
            g.add_reference("top.rb", format!("Dep{i}"));
        }
        for i in 0..5 {
            let f = format!("caller_{i}.rb");
            g.register_file(&f);
            g.add_reference(&f, "Top");
        }
        // Complexity values: top has 20 (max), others have 1.
        let mut cx_entries = vec![("top.rb", 20usize)];
        for i in 0..5 {
            // We leak the &'static str from Box so the slice can borrow.
            let name = Box::leak(format!("caller_{i}.rb").into_boxed_str()) as &'static str;
            cx_entries.push((name, 1));
        }
        let signals = signals_with_complexity(&g, &cx_entries);
        let rows = Scorer::new().score_with_signals(signals);
        let top = row(&rows, "top.rb");
        assert!(
            (top.composite - 1.0).abs() < 1e-9,
            "three-signal composite should hit 1.0 when every signal's pct is 1.0, got {}",
            top.composite
        );
        // And weights sum to 1.0: compute the expected composite manually.
        let w_fi = BASE_WEIGHT_FAN_IN
            / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
        let w_fo = BASE_WEIGHT_FAN_OUT
            / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
        let w_cx = BASE_WEIGHT_COMPLEXITY
            / (BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT + BASE_WEIGHT_COMPLEXITY);
        assert!((w_fi + w_fo + w_cx - 1.0).abs() < 1e-12);
    }

    #[test]
    fn empty_complexity_map_degenerates_to_two_signal_mode() {
        // Scoring with an empty complexity map must produce the SAME
        // rows as calling `score(&graph)`. Any drift means the
        // availability-mode logic broke the back-compat shape.
        let g = tiny_graph();
        let rows_two_sig = Scorer::new().score(&g);
        let rows_signals = Scorer::new().score_with_signals(Signals::from_graph(&g));

        assert_eq!(rows_two_sig.len(), rows_signals.len());
        for a in &rows_two_sig {
            let b = rows_signals
                .iter()
                .find(|r| r.path == a.path)
                .unwrap_or_else(|| panic!("missing row for {}", a.path.display()));
            assert!((a.composite - b.composite).abs() < 1e-9);
            assert_eq!(a.class, b.class);
            assert!((a.fan_in_pct - b.fan_in_pct).abs() < 1e-9);
            assert!((a.fan_out_pct - b.fan_out_pct).abs() < 1e-9);
            assert_eq!(b.complexity, 0);
            assert_eq!(b.complexity_pct, 0.0);
        }
    }

    /// Build a graph + complexity map where `spiky.rb` meets the
    /// `COMPLEXITY_FLOOR` on raw complexity but has a composite
    /// percentile that lands in the `Leaf` tier. Complexity has the
    /// highest base weight (5/11), so making a high-complexity file
    /// rank Leaf requires peers that are even higher on every signal.
    ///
    /// Construction: 100 peer files at fan_in = fan_out = 10 and
    /// complexity = 50; spiky.rb has fan_in = fan_out = 0 and
    /// complexity = `COMPLEXITY_FLOOR` (exactly the threshold).
    /// Spiky's percentile is 0 on every signal (every peer is higher)
    /// so its composite is 0 and it ranks Leaf. With the floor, the
    /// absolute complexity = 15 triggers the Leaf → Branch escalation.
    fn complexity_floor_test_graph() -> (DependencyGraph, HashMap<PathBuf, usize>) {
        let mut g = DependencyGraph::new();
        let peer_count = 100usize;
        for i in 0..peer_count {
            g.register_file(format!("peer_{i}.rb"));
        }
        for i in 0..peer_count {
            for offset in 1..=10 {
                let target = format!("Peer{}", (i + offset) % peer_count);
                g.add_reference(format!("peer_{i}.rb"), target);
            }
        }
        g.register_file("spiky.rb");

        let mut complexity = HashMap::new();
        for i in 0..peer_count {
            complexity.insert(PathBuf::from(format!("peer_{i}.rb")), 50);
        }
        complexity.insert(PathBuf::from("spiky.rb"), COMPLEXITY_FLOOR);
        (g, complexity)
    }

    #[test]
    fn complexity_floor_escalates_leaf_to_branch_when_composite_is_low() {
        let (g, cx) = complexity_floor_test_graph();

        // Baseline: default scorer but with complexity_floor pushed
        // unreachably high so we see what the composite alone decided.
        let no_cx_floor = Scorer::with_floors(
            TRUNK_THRESHOLD_PCT,
            BRANCH_THRESHOLD_PCT,
            FAN_IN_FLOOR,
            usize::MAX,
        );
        let signals = Signals {
            graph: &g,
            complexity: cx.clone(),
        };
        let rows_no_floor = no_cx_floor.score_with_signals(signals);
        let spiky_no_floor = row(&rows_no_floor, "spiky.rb");
        assert!(
            spiky_no_floor.complexity >= COMPLEXITY_FLOOR,
            "setup: spiky should meet the complexity floor ({} >= {})",
            spiky_no_floor.complexity,
            COMPLEXITY_FLOOR
        );
        assert_eq!(
            spiky_no_floor.class,
            Class::Leaf,
            "setup: without the complexity floor, spiky should classify as Leaf by composite"
        );

        // With the default floor (15), spiky escalates to Branch.
        let default_signals = Signals {
            graph: &g,
            complexity: cx,
        };
        let rows_default = Scorer::new().score_with_signals(default_signals);
        let spiky_default = row(&rows_default, "spiky.rb");
        assert_eq!(
            spiky_default.class,
            Class::Branch,
            "complexity >= {COMPLEXITY_FLOOR} must escalate Leaf to Branch"
        );
    }

    #[test]
    fn complexity_floor_never_downgrades_trunk_or_branch() {
        // A file that is Trunk by composite AND meets the complexity
        // floor must stay Trunk — the floor only escalates Leaf, never
        // touches Branch or Trunk.
        let mut g = DependencyGraph::new();
        g.register_file("top.rb");
        for i in 0..30 {
            g.add_reference("top.rb", format!("Dep{i}"));
        }
        for i in 0..30 {
            let f = format!("caller_{i}.rb");
            g.register_file(&f);
            g.add_reference(&f, "Top");
        }
        let mut cx = HashMap::new();
        cx.insert(PathBuf::from("top.rb"), 20); // meets complexity floor
        for i in 0..30 {
            cx.insert(PathBuf::from(format!("caller_{i}.rb")), 1);
        }

        let signals = Signals {
            graph: &g,
            complexity: cx.clone(),
        };
        let rows = Scorer::new().score_with_signals(signals);
        let top = row(&rows, "top.rb");
        assert!(
            top.complexity >= COMPLEXITY_FLOOR,
            "setup: top should meet the complexity floor"
        );
        assert_eq!(
            top.class,
            Class::Trunk,
            "floor must not downgrade a Trunk even when it meets the floor count"
        );
    }
}
