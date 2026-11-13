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
use std::collections::{HashMap, HashSet};
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
/// Raw per-signal weight values used by the composite before
/// renormalization. Matches Ruby `Scorer::DEFAULT_WEIGHTS` key set
/// so sf-cli's `--weights` / `--interaction-weight` /
/// `--coupling-weight` flags can override the whole hash or
/// individual fields.
///
/// Defaults are Ruby's `DEFAULT_WEIGHTS` verbatim: fan_in 0.19,
/// fan_out 0.095, complexity 0.2375, churn 0.2375, coverage 0.095,
/// interaction 0.095, coupling 0.05.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawWeights {
    pub fan_in: f64,
    pub fan_out: f64,
    pub complexity: f64,
    pub churn: f64,
    pub coverage: f64,
    pub interaction: f64,
    pub coupling: f64,
}

impl Default for RawWeights {
    fn default() -> Self {
        Self {
            fan_in: 0.19,
            fan_out: 0.095,
            complexity: 0.2375,
            churn: 0.2375,
            coverage: 0.095,
            interaction: 0.095,
            coupling: 0.05,
        }
    }
}

impl RawWeights {
    /// Sum of all seven raw weights (raw, not renormalized). Ruby
    /// `validate_weights!` requires this to equal 1.0 within 0.001
    /// when the full-vector override is active.
    pub fn sum(&self) -> f64 {
        self.fan_in
            + self.fan_out
            + self.complexity
            + self.churn
            + self.coverage
            + self.interaction
            + self.coupling
    }

    /// Match Ruby `validate_weights!` (`lib/stud_finder/cli.rb:416`)
    /// — gated by `--weights`/`custom_weights`. Requires:
    /// 1. Every weight finite.
    /// 2. Sum within 0.001 of 1.0. Error quotes the observed sum.
    /// 3. Zero coverage weight without a coverage report (separate
    ///    error from interaction).
    /// 4. Zero interaction weight without a coverage report.
    /// 5. Active-weight total strictly > 0 given the real signal set.
    ///
    /// Active-total-only check: returns the Ruby
    /// `active weights must be greater than 0.0.` error when the
    /// weight hash projected onto the given signal availability
    /// sums to zero or less. Separate from `validate_full_override`
    /// so per-section callers can enforce the normalization
    /// invariant without re-running the full-vector / sum /
    /// coverage-report CLI rules. Caught as Ohm's P2 on PR #90 r3.
    pub fn validate_active_total(
        &self,
        has_coverage: bool,
        has_coupling: bool,
    ) -> Result<(), String> {
        let mut active = self.fan_in + self.fan_out + self.complexity + self.churn;
        if has_coverage {
            active += self.coverage + self.interaction;
        }
        if has_coupling {
            active += self.coupling;
        }
        if active <= 0.0 {
            return Err("Error: active weights must be greater than 0.0.".to_string());
        }
        Ok(())
    }

    /// `has_coupling` lets the check include coupling in the active
    /// total when coupling data is available — a `coupling:1.0` vector
    /// with real history is valid. Caught as Ohm's P2 on PR #90 r2.
    pub fn validate_full_override(
        &self,
        has_coverage: bool,
        has_coupling: bool,
    ) -> Result<(), String> {
        if !self.fan_in.is_finite()
            || !self.fan_out.is_finite()
            || !self.complexity.is_finite()
            || !self.churn.is_finite()
            || !self.coverage.is_finite()
            || !self.interaction.is_finite()
            || !self.coupling.is_finite()
        {
            return Err("Error: weight values must be floats.".to_string());
        }
        let s = self.sum();
        if (s - 1.0).abs() > 0.001 {
            return Err(format!("Error: weights must sum to 1.0 (got {s}).",));
        }
        if !has_coverage && self.coverage > 0.0 {
            return Err("Error: coverage weight requires a coverage report.".to_string());
        }
        if !has_coverage && self.interaction > 0.0 {
            return Err("Error: interaction weight requires a coverage report.".to_string());
        }
        let mut active = self.fan_in + self.fan_out + self.complexity + self.churn;
        if has_coverage {
            active += self.coverage + self.interaction;
        }
        if has_coupling {
            active += self.coupling;
        }
        if active <= 0.0 {
            return Err("Error: active weights must be greater than 0.0.".to_string());
        }
        Ok(())
    }
}

/// Internal: compute (active_total, fan_in_w, fan_out_w, complexity_w,
/// churn_w, coupling_w, coverage_w, interaction_w) with a safety
/// fallback. If the input raw weights are non-finite OR the derived
/// active total is <= 0, substitute `RawWeights::default()` so the
/// composite math stays well-defined. Ruby raises before scoring; the
/// sf-cli layer validates on the `--weights` path, but library
/// consumers reaching Scorer / effective_weights_from directly must
/// not produce NaN/Infinity rows. Caught as Ohm's P2 on PR #90 r2.
fn safe_normalize(
    raw: &RawWeights,
    has_coupling: bool,
    has_coverage: bool,
) -> (f64, f64, f64, f64, f64, f64, f64) {
    let all_finite = raw.fan_in.is_finite()
        && raw.fan_out.is_finite()
        && raw.complexity.is_finite()
        && raw.churn.is_finite()
        && raw.coverage.is_finite()
        && raw.interaction.is_finite()
        && raw.coupling.is_finite();
    let mut candidate_total = raw.fan_in + raw.fan_out + raw.complexity + raw.churn;
    if has_coupling {
        candidate_total += raw.coupling;
    }
    if has_coverage {
        candidate_total += raw.coverage + raw.interaction;
    }
    let safe_raw = if all_finite && candidate_total > 0.0 {
        *raw
    } else {
        RawWeights::default()
    };
    let mut active_total =
        safe_raw.fan_in + safe_raw.fan_out + safe_raw.complexity + safe_raw.churn;
    if has_coupling {
        active_total += safe_raw.coupling;
    }
    if has_coverage {
        active_total += safe_raw.coverage + safe_raw.interaction;
    }
    (
        safe_raw.fan_in / active_total,
        safe_raw.fan_out / active_total,
        safe_raw.complexity / active_total,
        safe_raw.churn / active_total,
        if has_coupling {
            safe_raw.coupling / active_total
        } else {
            0.0
        },
        if has_coverage {
            safe_raw.coverage / active_total
        } else {
            0.0
        },
        if has_coverage {
            safe_raw.interaction / active_total
        } else {
            0.0
        },
    )
}

// Default base weights are kept as named constants so sf-score
// tests can assert against them symbolically. The production
// scoring path reads the equivalent fields on `RawWeights::default`
// (slice #10f); `#[allow(dead_code)]` keeps the symbols around for
// the test suite without warning on the lib side.
#[allow(dead_code)]
const BASE_WEIGHT_FAN_IN: f64 = 0.19;
#[allow(dead_code)]
const BASE_WEIGHT_FAN_OUT: f64 = 0.095;
#[allow(dead_code)]
const BASE_WEIGHT_COMPLEXITY: f64 = 0.2375;
#[allow(dead_code)]
const BASE_WEIGHT_CHURN: f64 = 0.2375;
/// Extra-signal weight for temporal coupling, from the Ruby
/// `DEFAULT_WEIGHTS[:coupling]`. Outside the base `4 : 2 : 5 : 5` ratio.
#[allow(dead_code)]
const EXTRA_WEIGHT_COUPLING: f64 = 0.05;
/// Coverage base-signal weight from Ruby `DEFAULT_WEIGHTS[:coverage]`.
/// Ruby lumps coverage into the active-keys sum next to the base
/// four, so a single Rust renormalization over "all active" handles
/// it identically. Coverage only lands together with interaction.
#[allow(dead_code)]
const BASE_WEIGHT_COVERAGE: f64 = 0.095;
/// Interaction cross-term weight from Ruby
/// `DEFAULT_WEIGHTS[:interaction]`. Ruby's interaction score is
/// `interaction_weight * fan_in_pct * coverage_pct` — not a
/// percentile-ranked signal. See `interaction_score` in scorer.rb.
#[allow(dead_code)]
const EXTRA_WEIGHT_INTERACTION: f64 = 0.095;

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
    /// Non-blank line count from `sf-cli`'s LOC pass. `0` when the
    /// file was unreadable or LOC was not measured. Matches Ruby
    /// `LocCounter#non_blank_lines`. Reported, not scored — loc
    /// is NOT a signal in the composite.
    pub loc: usize,
    /// Fan-out / (fan_in + fan_out), rounded to 4 decimals. `0.0`
    /// when both are zero. Matches Ruby `Scorer#instability`.
    pub instability: f64,
    /// Coupling partner with the strongest ratio for this file, if any.
    /// Empty path means no coupling partner (no pair cleared the Ruby
    /// min_co_changes + coupling_threshold filters).
    pub max_coupling_partner: PathBuf,
    pub max_coupling: f64,
    /// Count of qualifying coupling partners (ratio ≥ threshold AND
    /// co_changes ≥ min) for this file. `0` when no pair cleared the
    /// filters. Matches Ruby `aggregate_partners`'s `partners:
    /// partners.length`.
    pub coupling_partners: usize,
    /// Line coverage in `[0.0, 1.0]`. `0.0` when the coverage signal
    /// is unavailable OR when the file is missing from a provided
    /// coverage report.
    pub coverage: f64,
    pub fan_in_pct: f64,
    pub fan_out_pct: f64,
    pub complexity_pct: f64,
    pub churn_pct: f64,
    pub coupling_pct: f64,
    pub coverage_pct: f64,
    pub loc_pct: f64,
    pub instability_pct: f64,
    pub composite: f64,
    pub class: Class,
    /// Reason the composite classification was escalated beyond its
    /// percentile band, as a short tag (empty when not escalated).
    /// Current tags: `"complexity_floor"` (Leaf→Branch because raw
    /// complexity ≥ COMPLEXITY_FLOOR), `"fan_in_floor"` (Leaf→Branch
    /// because raw fan_in ≥ FAN_IN_FLOOR), `"trunk_adjacent"`
    /// (newness: new_file with a trunk dependency — promoted all
    /// the way to Trunk), `"recency_floor"` (newness: new_file
    /// that would have been Leaf or floor-escalated → promoted to
    /// Branch). Matches Ruby `Scorer#floored_classification` +
    /// `Newness.apply`.
    pub escalation: String,
    /// Evidence confidence in `[0.0, 1.0]`. Starts at `1.0` from
    /// `score_with_signals` (placeholder) and is replaced by
    /// `apply_newness` using the Ruby formula over `age_days`,
    /// `total_commits`, and whether the file has an explicit
    /// coverage entry. See `sf_newness::evidence_for`. When
    /// `evidence_available` is false this field is meaningless —
    /// readers must emit null/n/a instead.
    pub evidence: f64,
    /// `true` when `evidence` carries a real computed value.
    /// `false` when newness metadata was unavailable (disabled,
    /// shallow repo, failed) so Ruby's `Newness.evidence(...)`
    /// would have returned `nil`. The output layer gates
    /// JSON/CSV/markdown emission on this flag to avoid claiming
    /// confidence we don't have. Caught as Ohm's P2 on PR #88.
    pub evidence_available: bool,
    /// `true` when the file is within the recency window, below
    /// the min-commits threshold, or has unknown lineage. `false`
    /// when both thresholds are met OR when newness metadata is
    /// unavailable (shallow repo / disabled / failed). Only
    /// populated after `apply_newness`; defaults to `false`.
    pub new_file: bool,
    /// Days since the file's first commit by COMMITTER time. `0`
    /// when the file has no history (or metadata unavailable).
    pub age_days: u64,
    /// Non-merge commits touching this file after rename
    /// canonicalization and deletion boundaries. `0` when newness
    /// is unavailable.
    pub total_commits: usize,
    /// `true` when the newness run produced a real record for this
    /// file (even if that record has zero commits). `false` when
    /// the newness signal was disabled, the repo was shallow, or
    /// the run failed. Mirrors Ruby's `metadata_available`.
    pub newness_metadata_available: bool,
    /// `true` when the file had an explicit entry in the loaded
    /// coverage report. Distinguishes "coverage is 0.0 because the
    /// file genuinely has 0% coverage" from "coverage is 0.0
    /// because the file was missing from the report, Ruby would
    /// emit em-dash". `false` when the coverage signal isn't
    /// loaded at all OR when the file is missing from the report.
    /// Pair with a run-level `coverage_available` to tell the two
    /// apart at emit time.
    pub coverage_reported: bool,
    /// 1-based rank within this file's language section, assigned
    /// AFTER the primary sort but BEFORE any filters (`--top`,
    /// `--diff-base`). Zero when unassigned (callers that skip
    /// ranking). Preserving pre-filter ranks is the Ruby contract
    /// — a filtered output can land ranks 1, 7, 23, 98 rather
    /// than being renumbered.
    pub rank: usize,
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
    /// Count of all qualifying partners for this file (ratio ≥
    /// threshold AND co_changes ≥ min). Mirrors Ruby
    /// `aggregate_partners`'s `partners: partners.length`. Equal to
    /// the number of edges this file's node has in the coupling
    /// graph after the threshold cut — NOT just the strongest
    /// partner.
    pub partners: usize,
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
    /// Per-file line coverage in `[0.0, 1.0]`. `Some(_)` = coverage
    /// signal AVAILABLE — matches Ruby's `coverage_available? =
    /// !@coverage.nil?` gate; `Some(empty)` still activates the
    /// signal and every requested file gets risk 1.0 (fetch-with-0.0
    /// default). `None` = coverage not provided — both base-weight
    /// coverage AND the interaction cross-term drop out of the
    /// composite. Missing keys inside a `Some(_)` map default to
    /// 0.0 coverage (risk 1.0), matching Ruby
    /// `coverage_risk_values` using `.fetch(file, 0.0)`.
    ///
    /// The signal in the composite is `coverage_risk = 1.0 -
    /// coverage` (via Ruby `coverage_risk_values`): low coverage is
    /// high risk. Fixes Ohm's P1 on PR #80 — ranking raw coverage
    /// instead of risk had reversed the scoring direction.
    pub coverage: Option<HashMap<PathBuf, f64>>,
    /// Per-file non-blank LOC (from `sf-cli` LOC pass, Ruby
    /// `LocCounter#non_blank_lines`). Empty map = LOC signal not
    /// measured for this run — `ScoredFile.loc` defaults to 0 and
    /// `loc_pct` degenerates to 0.0. Missing keys inside a
    /// non-empty map default to 0. LOC is REPORTED, not SCORED:
    /// it carries a percentile for display but no composite
    /// weight — matches Ruby, which emits `loc`/`loc_pct` columns
    /// but doesn't fold them into the composite.
    pub loc: HashMap<PathBuf, usize>,
}

impl<'a> Signals<'a> {
    /// Signals bundle with just the graph — no complexity, no churn,
    /// no coupling, no coverage, no loc. Matches the sf-score first-slice shape.
    pub fn from_graph(graph: &'a DependencyGraph) -> Self {
        Self {
            graph,
            complexity: HashMap::new(),
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
            loc: HashMap::new(),
        }
    }
}

/// Renormalized weights for the composite, matching Ruby's
/// `Scorer#normalize_weights` output. Mirrors the shape of
/// `json_weights` in `lib/stud_finder/cli.rb`: `fan_in`, `fan_out`,
/// `complexity`, and `churn` are always numbers (0.0 when the signal
/// is inactive), while `coverage`, `interaction`, and `coupling` are
/// `None` when the corresponding signal is unavailable. Callers that
/// serialize to JSON should emit `None` as `null` to match Ruby's
/// `&.round(4)` pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveWeights {
    pub fan_in: f64,
    pub fan_out: f64,
    pub complexity: f64,
    pub churn: f64,
    pub coupling: Option<f64>,
    pub coverage: Option<f64>,
    pub interaction: Option<f64>,
}

/// Compute the active-weight renormalization for a given signal set,
/// matching Ruby `Scorer#normalize_weights`.
///
/// Ruby keeps all four `BASE_WEIGHT_KEYS` (fan_in, fan_out,
/// complexity, churn) active for the renormalization denominator
/// EVERY RUN, regardless of whether each signal is populated.
/// Absent signals contribute 0.0 to the per-file score but still
/// occupy their share of the weight ratio — so remaining signals
/// do NOT grow to fill the gap. `coverage` + `interaction` add to
/// active_total only when coverage is loaded; `coupling` only when
/// coupling data is available.
///
/// Prior revision dropped absent BASE signals from the denominator
/// (Ohm non-blocking on PR #86); the scorer and this helper swap
/// together to the Ruby behavior in slice #10d. For a Ruby-only
/// 4-factor run with all signals present, the numbers don't
/// change; divergence only surfaced on runs with an absent base
/// signal (e.g. empty complexity map).
pub fn effective_weights(
    _has_complexity: bool,
    _has_churn: bool,
    has_coupling: bool,
    has_coverage: bool,
) -> EffectiveWeights {
    effective_weights_from(&RawWeights::default(), has_coupling, has_coverage)
}

/// Like `effective_weights` but accepts an explicit raw-weight
/// override. sf-cli reaches for this when `--weights` /
/// `--interaction-weight` / `--coupling-weight` supply non-default
/// values; `meta.weights` reflects whatever the user passed.
pub fn effective_weights_from(
    raw: &RawWeights,
    has_coupling: bool,
    has_coverage: bool,
) -> EffectiveWeights {
    let (w_fi, w_fo, w_cx, w_ch, w_cp, w_cov, w_int) =
        safe_normalize(raw, has_coupling, has_coverage);
    EffectiveWeights {
        fan_in: w_fi,
        fan_out: w_fo,
        complexity: w_cx,
        churn: w_ch,
        coupling: has_coupling.then_some(w_cp),
        coverage: has_coverage.then_some(w_cov),
        interaction: has_coverage.then_some(w_int),
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
    /// Optional custom raw weights. `None` = Ruby `DEFAULT_WEIGHTS`
    /// (the inline constants). `Some` overrides every signal weight
    /// before renormalization, matching Ruby's `--weights`,
    /// `--interaction-weight`, `--coupling-weight`.
    pub weights: Option<RawWeights>,
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
            weights: None,
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
            weights: None,
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
            weights: None,
        }
    }

    /// Override the composite weights for this scorer. Pass the
    /// RawWeights returned by `sf-cli` after parsing
    /// `--weights`/`--interaction-weight`/`--coupling-weight`.
    pub fn with_weights(mut self, weights: RawWeights) -> Self {
        self.weights = Some(weights);
        self
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
        let coupling_strongest: Vec<(PathBuf, f64)> = files
            .iter()
            .map(|p| {
                signals
                    .coupling
                    .get(*p)
                    .map(|c| (c.partner.clone(), c.ratio))
                    .unwrap_or_default()
            })
            .collect();
        let coupling_partner_counts: Vec<usize> = files
            .iter()
            .map(|p| signals.coupling.get(*p).map(|c| c.partners).unwrap_or(0))
            .collect();

        // LOC is reported, not scored. Percentile is emitted for
        // display parity with Ruby but never enters the composite.
        let loc_values: Vec<usize> = files
            .iter()
            .map(|p| signals.loc.get(*p).copied().unwrap_or(0))
            .collect();
        let loc_pcts = if !signals.loc.is_empty() {
            percentile_rank_usize(&loc_values)
        } else {
            vec![0.0; files.len()]
        };

        // Instability = fan_out / (fan_in + fan_out), rounded to 4
        // decimals — matches Ruby `Scorer#instability`. 0.0 when both
        // sides are zero (an isolated file). Always available because
        // fan_in/fan_out are always available.
        let instability_values: Vec<f64> = (0..files.len())
            .map(|i| {
                let total = fan_ins[i] + fan_outs[i];
                if total == 0 {
                    0.0
                } else {
                    round4((fan_outs[i] as f64) / (total as f64))
                }
            })
            .collect();
        let instability_pcts = percentile_rank_f64(&instability_values);

        // Coverage + interaction activate together via the explicit
        // `Option` gate — matches Ruby `coverage_available? =
        // !@coverage.nil?`. A caller that successfully loaded an
        // empty report still activates the signal; every requested
        // file defaults to 0.0 coverage (risk 1.0). Fixes Ohm's P2
        // on PR #80 (empty-provided coverage was incorrectly
        // disabling the weights).
        //
        // The ranked signal is `coverage_risk = 1.0 - coverage` per
        // Ruby `coverage_risk_values`. Raw coverage still lands in
        // `ScoredFile.coverage` for display. Fixes Ohm's P1 on PR
        // #80 (percentile-ranking raw coverage reversed the scoring
        // direction).
        let has_coverage = signals.coverage.is_some();
        let coverage_values: Vec<f64> = files
            .iter()
            .map(|p| {
                signals
                    .coverage
                    .as_ref()
                    .and_then(|m| m.get(*p).copied())
                    .unwrap_or(0.0)
            })
            .collect();
        let coverage_risk_values: Vec<f64> = coverage_values.iter().map(|c| 1.0 - c).collect();

        let fan_in_pcts = percentile_rank_usize(&fan_ins);
        let fan_out_pcts = percentile_rank_usize(&fan_outs);
        let complexity_pcts = if has_complexity {
            percentile_rank_usize(&complexities)
        } else {
            vec![0.0; files.len()]
        };

        // Composite churn: percentile of avg(count_pct, line_pct) per
        // file — matches Ruby `composite_churn_pct`. The averaged raw
        // value is rounded to 10 decimal places BEFORE being re-ranked
        // so near-tied averages (e.g. `(0/5 + 3/5)/2 = 0.3` vs
        // `(1/5 + 2/5)/2 = 0.30000000000000004`) collapse to the same
        // rank, matching Ruby's lower-precision `Rational`/`round(10)`
        // semantics at the `composite_churn_pct` boundary. Fixes Ohm's
        // P1 on PR #79.
        //
        // When only one side of the churn signal is present (one map
        // empty), the empty-side percentile is 0.0 for every file and
        // the composite degrades gracefully to half of the active
        // side's percentile. Not ideal but deterministic and documented.
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
                .map(|i| round10((count_pcts[i] + line_pcts[i]) / 2.0))
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
        // Rank the RISK (= 1 - coverage), not the coverage itself —
        // Ruby `coverage_risk_pct = Normalizer.percentile_rank(
        // coverage_risk_values, @files)`. A 0%-covered file gets
        // risk_pct ≈ 1.0 so it adds to the composite; a 100%-covered
        // file gets risk_pct ≈ 0.0.
        let coverage_pcts = if has_coverage {
            percentile_rank_f64(&coverage_risk_values)
        } else {
            vec![0.0; files.len()]
        };

        // Ruby `Scorer#normalize_weights` keeps ALL FOUR
        // `BASE_WEIGHT_KEYS` active for the denominator every run,
        // regardless of whether each signal is populated. An absent
        // signal contributes 0 to the per-file score but its weight
        // still occupies the ratio share — so remaining signals
        // don't balloon to fill the gap. coverage/interaction add
        // only when coverage is loaded; coupling only when coupling
        // data is available.
        //
        // Uses `self.weights` when sf-cli supplied a custom override
        // via `--weights`/`--interaction-weight`/`--coupling-weight`,
        // else Ruby's `DEFAULT_WEIGHTS`. Slice #10f adds the custom-
        // weights path; the default path matches the slice #10d
        // Ruby-parity fix.
        // Route the inline math through `safe_normalize` so NaN
        // and zero-active-total raw inputs fall back to defaults
        // before any division. The sf-cli validator catches these
        // on the --weights path, but library callers using
        // `Scorer::with_weights(...)` directly also must not
        // produce NaN rows. Caught as Ohm's P2 on PR #90 r2.
        let raw = self.weights.unwrap_or_default();
        let (w_fi, w_fo, w_cx, w_ch, w_cp, w_cov, w_int) =
            safe_normalize(&raw, has_coupling, has_coverage);

        // Composite includes the interaction cross-term `w_int *
        // fan_in_pct * coverage_pct` — Ruby `interaction_score` uses
        // coverage_pct (percentile) not raw coverage. The cross-term
        // lives alongside the base weighted sum (which also gets its
        // own coverage term via w_cov * coverage_pct).
        let composites: Vec<f64> = (0..files.len())
            .map(|i| {
                w_fi * fan_in_pcts[i]
                    + w_fo * fan_out_pcts[i]
                    + w_cx * complexity_pcts[i]
                    + w_ch * churn_pcts[i]
                    + w_cp * coupling_pcts[i]
                    + w_cov * coverage_pcts[i]
                    + w_int * fan_in_pcts[i] * coverage_pcts[i]
            })
            .collect();

        let composite_pcts = percentile_rank_f64(&composites);

        let trunk_cutoff = self.trunk_threshold_pct;
        let branch_cutoff = self.branch_threshold_pct;
        let fan_in_floor = self.fan_in_floor;
        let complexity_floor = self.complexity_floor;

        // Walk class + escalation together so the reason string stays
        // in lock-step with the class decision. Matches Ruby
        // `Scorer#floored_classification` returning `[class, reason]`.
        let class_decisions: Vec<(Class, String)> = (0..files.len())
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
                // Trunk and Branch are never downgraded. Reason tag
                // matches Ruby: complexity_floor wins the tie because
                // Ruby checks it first.
                if base == Class::Leaf {
                    if has_complexity && complexities[i] >= complexity_floor {
                        (Class::Branch, "complexity_floor".to_string())
                    } else if fan_ins[i] >= fan_in_floor {
                        (Class::Branch, "fan_in_floor".to_string())
                    } else {
                        (base, String::new())
                    }
                } else {
                    (base, String::new())
                }
            })
            .collect();

        files
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let (partner_path, _partner_ratio) = coupling_strongest[i].clone();
                let (class, escalation) = class_decisions[i].clone();
                ScoredFile {
                    path: path.to_path_buf(),
                    fan_in: fan_ins[i],
                    fan_out: fan_outs[i],
                    complexity: complexities[i],
                    churn_commits: churn_commits[i],
                    churn_lines: churn_lines[i],
                    loc: loc_values[i],
                    instability: instability_values[i],
                    max_coupling_partner: partner_path,
                    max_coupling: coupling_values[i],
                    coupling_partners: coupling_partner_counts[i],
                    coverage: coverage_values[i],
                    fan_in_pct: fan_in_pcts[i],
                    fan_out_pct: fan_out_pcts[i],
                    complexity_pct: complexity_pcts[i],
                    churn_pct: churn_pcts[i],
                    coupling_pct: coupling_pcts[i],
                    coverage_pct: coverage_pcts[i],
                    loc_pct: loc_pcts[i],
                    instability_pct: instability_pcts[i],
                    composite: round4(composites[i]),
                    class,
                    escalation,
                    evidence: 1.0,
                    // Before apply_newness runs the evidence field
                    // carries the scorer placeholder 1.0. The
                    // placeholder is never emitted — apply_newness
                    // always runs on the sf-cli hot path, either
                    // replacing it with a real value or flipping
                    // availability off. Downstream readers must gate
                    // emission on this flag; it stays false until
                    // apply_newness decides.
                    evidence_available: false,
                    new_file: false,
                    age_days: 0,
                    total_commits: 0,
                    newness_metadata_available: false,
                    coverage_reported: signals
                        .coverage
                        .as_ref()
                        .map(|m| m.contains_key(*path))
                        .unwrap_or(false),
                    rank: 0,
                }
            })
            .collect()
    }
}

/// Newness-aware reclassification + evidence replacement, matching
/// Ruby `Newness.apply` on `lib/stud_finder/newness.rb`.
///
/// Two-pass walk:
///
/// 1. Merge newness metadata into each row and compute `evidence`
///    via `sf_newness::evidence_for`. `escalation` is preserved
///    from the scorer (complexity_floor / fan_in_floor) — newness
///    rules in pass 2 may override.
/// 2. For each row with `new_file: true`:
///    - If any dependency is a Trunk → class = Trunk,
///      escalation = `"trunk_adjacent"`.
///    - Else if class == Leaf OR escalation is a floor tag →
///      class = Branch, escalation = `"recency_floor"`.
///    - Else: preserved.
///
/// `has_coverage_for(path)` names whether the file has an
/// explicit coverage entry (Ruby `coverage&.key?(row[:path])`).
/// Pass `|_| false` when no coverage was loaded.
pub fn apply_newness<F>(
    rows: &mut [ScoredFile],
    newness: &HashMap<PathBuf, sf_newness::FileNewness>,
    graph: &DependencyGraph,
    has_coverage_for: F,
) where
    F: Fn(&Path) -> bool,
{
    // Pass 1: merge newness + recompute evidence.
    for row in rows.iter_mut() {
        let placeholder = sf_newness::FileNewness::disabled();
        let meta = newness.get(&row.path).unwrap_or(&placeholder);
        row.new_file = meta.new_file;
        row.age_days = meta.age_days;
        row.total_commits = meta.total_commits;
        row.newness_metadata_available = meta.metadata_available;
        // Match Ruby `Newness.apply` which ALWAYS sets `row[:evidence]
        // = Newness.evidence(...)`. When the result is `nil` (because
        // `metadata_available` is false) Ruby stores nil; we flip
        // `evidence_available` to false so output layers emit null
        // instead of the scorer placeholder. Caught as Ohm's P2 on
        // PR #88.
        match sf_newness::evidence_for(meta, has_coverage_for(row.path.as_path())) {
            Some(e) => {
                row.evidence = e;
                row.evidence_available = true;
            }
            None => {
                row.evidence_available = false;
            }
        }
        // escalation preserved from the scorer — pass 2 overrides.
    }

    // Snapshot of which paths are Trunk BEFORE pass 2 so trunk
    // promotions in pass 2 don't cascade — matches Ruby, which
    // computes `trunk_paths` once from the pre-pass rows.
    let trunk_paths: HashSet<PathBuf> = rows
        .iter()
        .filter(|r| r.class == Class::Trunk)
        .map(|r| r.path.clone())
        .collect();

    for row in rows.iter_mut() {
        if !row.new_file {
            continue;
        }
        let deps = graph.dependencies_of(row.path.as_path());
        if deps.iter().any(|d| trunk_paths.contains(d)) {
            row.class = Class::Trunk;
            row.escalation = "trunk_adjacent".to_string();
        } else if row.class == Class::Leaf
            || row.escalation == "complexity_floor"
            || row.escalation == "fan_in_floor"
        {
            row.class = Class::Branch;
            row.escalation = "recency_floor".to_string();
        }
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

/// Round `x` to 10 decimal places, matching the precision Ruby applies
/// at the `composite_churn_pct` boundary so IEEE-754 noise at the
/// 1e-17 scale doesn't split what Ruby treats as tied averages.
fn round10(x: f64) -> f64 {
    (x * 1e10).round() / 1e10
}

#[cfg(test)]
mod tests;
