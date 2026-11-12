//! stud-finder's composite score + percentile classification layer.
//!
//! Given a `Signals` bundle (a `sf-core::DependencyGraph` plus optional
//! per-file auxiliary maps), `Scorer::score_with_signals` emits one
//! `ScoredFile` per registered file containing:
//!
//! - raw signal counts (`fan_in`, `fan_out`, `complexity`, `churn_commits`,
//!   `churn_lines`, `max_coupling`, `max_coupling_partner`)
//! - percentile-ranked signals (`fan_in_pct`, `fan_out_pct`, `complexity_pct`,
//!   `churn_pct`, `coupling_pct`) in [0.0, 1.0]
//! - a weighted `composite` score in [0.0, 1.0]
//! - a `Class` tier (`Trunk` / `Branch` / `Leaf`) by composite percentile
//!   rank with absolute-floor escalation for raw fan_in and complexity
//! - an `evidence` number in [0.0, 1.0]
//!
//! Weights follow the Ruby `DEFAULT_WEIGHTS` + `BASE_WEIGHT_KEYS` split:
//! fan_in, fan_out, complexity, churn are the base signals (ratio
//! `4 : 2 : 5 : 5` — Ruby's 0.19 : 0.095 : 0.2375 : 0.2375). Coupling is
//! an extra signal outside the base ratio at weight 0.05. Active weights
//! are renormalized to sum to 1.0, matching `Scorer#normalize_weights` on
//! `lib/stud_finder/scorer.rb`. Signals that are absent from the bundle
//! (empty complexity map, empty churn map, empty coupling map) simply
//! drop out of the renormalization.
//!
//! `evidence` is hard-coded to `1.0` for every row until sf-coverage
//! lands. The real formula is `max(history_only, with_coverage)` on
//! file age + commit count + optional coverage.
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

// Base weights from the Ruby `DEFAULT_WEIGHTS`. The ratio of these four
// is 4 : 2 : 5 : 5. When all four signals are active the active weight
// sum is 0.76, so each renormalized weight is `raw / 0.76` and the
// ratio is preserved. When a signal is missing the sum shrinks and the
// remaining weights renormalize automatically.
const BASE_WEIGHT_FAN_IN: f64 = 0.19;
const BASE_WEIGHT_FAN_OUT: f64 = 0.095;
const BASE_WEIGHT_COMPLEXITY: f64 = 0.2375;
const BASE_WEIGHT_CHURN: f64 = 0.2375;
/// Extra-signal weight for temporal coupling, from the Ruby
/// `DEFAULT_WEIGHTS[:coupling]`. Outside the base `4 : 2 : 5 : 5` ratio.
const EXTRA_WEIGHT_COUPLING: f64 = 0.05;

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
    pub churn_commits: usize,
    pub churn_lines: usize,
    /// Coupling partner with the strongest ratio for this file, if any.
    /// Empty path means no coupling partner (no pair cleared the Ruby
    /// min_co_changes + coupling_threshold filters).
    pub max_coupling_partner: PathBuf,
    pub max_coupling: f64,
    pub fan_in_pct: f64,
    pub fan_out_pct: f64,
    pub complexity_pct: f64,
    pub churn_pct: f64,
    pub coupling_pct: f64,
    pub composite: f64,
    pub class: Class,
    pub evidence: f64,
}

/// Per-file coupling info the scorer takes in — `partner` is the
/// strongest coupling partner's path, `ratio` is the Ruby
/// `co_changes / min(own_changes)` fraction, `co_changes` is the raw
/// count. Matches `sf_history::CouplingPartner` in shape so sf-cli can
/// forward it verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct CouplingSignal {
    pub partner: PathBuf,
    pub ratio: f64,
    pub co_changes: usize,
}

/// Bundle of per-file signal data the scorer consumes. `graph` is
/// mandatory; other maps are optional and only fill out the composite
/// when non-empty. Signals extend this struct as new slices land —
/// sf-history added churn + coupling, sf-coverage will add coverage +
/// interaction.
pub struct Signals<'a> {
    pub graph: &'a DependencyGraph,
    /// Max cyclomatic complexity per file (from `sf-ruby::max_complexity`).
    /// Empty = complexity signal is unavailable and drops out of the
    /// composite. Missing keys inside a non-empty map default to 0.
    pub complexity: HashMap<PathBuf, usize>,
    /// Non-merge commits in the churn window touching each file
    /// (from `sf-history::analyze`). Empty = churn signal unavailable.
    /// Missing keys inside a non-empty map default to 0.
    pub churn_commits: HashMap<PathBuf, usize>,
    /// Sum of added+deleted lines in the churn window per file. Empty
    /// = churn signal unavailable. Missing keys default to 0. Churn
    /// composite is the percentile of `avg(count_pct, line_pct)` per
    /// Ruby `composite_churn_pct` — a count-only or lines-only churn
    /// signal still works but degrades to that single-side percentile.
    pub churn_lines: HashMap<PathBuf, usize>,
    /// Per-file strongest coupling partner. Empty = coupling signal
    /// unavailable. Only files whose pair cleared Ruby's
    /// min_co_changes / coupling_threshold filters appear here.
    pub coupling: HashMap<PathBuf, CouplingSignal>,
}

impl<'a> Signals<'a> {
    /// Signals bundle with just the graph — no complexity, no churn,
    /// no coupling. Matches the sf-score first-slice shape.
    pub fn from_graph(graph: &'a DependencyGraph) -> Self {
        Self {
            graph,
            complexity: HashMap::new(),
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
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
    /// or churn data.
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

        // Churn is available if EITHER map is non-empty. Both arrive
        // from the same sf-history call, but a caller could supply only
        // one; the composite handles single-side degradation.
        let has_churn = !signals.churn_commits.is_empty() || !signals.churn_lines.is_empty();
        let churn_commits: Vec<usize> = files
            .iter()
            .map(|p| signals.churn_commits.get(*p).copied().unwrap_or(0))
            .collect();
        let churn_lines: Vec<usize> = files
            .iter()
            .map(|p| signals.churn_lines.get(*p).copied().unwrap_or(0))
            .collect();

        let has_coupling = !signals.coupling.is_empty();
        // Coupling signal raw: per-file max_coupling ratio (0.0 when no
        // partner cleared the filters). Matches Ruby `coupling_values`.
        let coupling_values: Vec<f64> = files
            .iter()
            .map(|p| signals.coupling.get(*p).map(|c| c.ratio).unwrap_or(0.0))
            .collect();
        let coupling_partners: Vec<(PathBuf, f64)> = files
            .iter()
            .map(|p| {
                signals
                    .coupling
                    .get(*p)
                    .map(|c| (c.partner.clone(), c.ratio))
                    .unwrap_or_default()
            })
            .collect();

        let fan_in_pcts = percentile_rank_usize(&fan_ins);
        let fan_out_pcts = percentile_rank_usize(&fan_outs);
        let complexity_pcts = if has_complexity {
            percentile_rank_usize(&complexities)
        } else {
            vec![0.0; files.len()]
        };

        // Composite churn: percentile of avg(count_pct, line_pct) per
        // file — matches Ruby `composite_churn_pct`. When only one side
        // of the churn signal is present (one map empty), the empty-side
        // percentile is 0.0 for every file and the composite degrades
        // gracefully to half of the active side's percentile. Not ideal
        // but deterministic and documented.
        let churn_pcts = if has_churn {
            let count_pcts = if !signals.churn_commits.is_empty() {
                percentile_rank_usize(&churn_commits)
            } else {
                vec![0.0; files.len()]
            };
            let line_pcts = if !signals.churn_lines.is_empty() {
                percentile_rank_usize(&churn_lines)
            } else {
                vec![0.0; files.len()]
            };
            let composite_churn_raw: Vec<f64> = (0..files.len())
                .map(|i| (count_pcts[i] + line_pcts[i]) / 2.0)
                .collect();
            percentile_rank_f64(&composite_churn_raw)
        } else {
            vec![0.0; files.len()]
        };

        let coupling_pcts = if has_coupling {
            percentile_rank_f64(&coupling_values)
        } else {
            vec![0.0; files.len()]
        };

        // Base active weight sum + optional coupling. Renormalize over
        // whatever the caller supplied; matches Ruby normalize_weights.
        let mut active_total = BASE_WEIGHT_FAN_IN + BASE_WEIGHT_FAN_OUT;
        if has_complexity {
            active_total += BASE_WEIGHT_COMPLEXITY;
        }
        if has_churn {
            active_total += BASE_WEIGHT_CHURN;
        }
        if has_coupling {
            active_total += EXTRA_WEIGHT_COUPLING;
        }
        let w_fi = BASE_WEIGHT_FAN_IN / active_total;
        let w_fo = BASE_WEIGHT_FAN_OUT / active_total;
        let w_cx = if has_complexity {
            BASE_WEIGHT_COMPLEXITY / active_total
        } else {
            0.0
        };
        let w_ch = if has_churn {
            BASE_WEIGHT_CHURN / active_total
        } else {
            0.0
        };
        let w_cp = if has_coupling {
            EXTRA_WEIGHT_COUPLING / active_total
        } else {
            0.0
        };

        let composites: Vec<f64> = (0..files.len())
            .map(|i| {
                w_fi * fan_in_pcts[i]
                    + w_fo * fan_out_pcts[i]
                    + w_cx * complexity_pcts[i]
                    + w_ch * churn_pcts[i]
                    + w_cp * coupling_pcts[i]
            })
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
            .map(|(i, path)| {
                let (partner_path, _partner_ratio) = coupling_partners[i].clone();
                ScoredFile {
                    path: path.to_path_buf(),
                    fan_in: fan_ins[i],
                    fan_out: fan_outs[i],
                    complexity: complexities[i],
                    churn_commits: churn_commits[i],
                    churn_lines: churn_lines[i],
                    max_coupling_partner: partner_path,
                    max_coupling: coupling_values[i],
                    fan_in_pct: fan_in_pcts[i],
                    fan_out_pct: fan_out_pcts[i],
                    complexity_pct: complexity_pcts[i],
                    churn_pct: churn_pcts[i],
                    coupling_pct: coupling_pcts[i],
                    composite: round4(composites[i]),
                    class: classes[i],
                    evidence: 1.0,
                }
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
mod tests;
