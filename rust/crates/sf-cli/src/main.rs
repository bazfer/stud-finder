//! stud-finder CLI (Rust port, work in progress).
//!
//! Current behaviour: walks a path, parses every Ruby file with `sf-ruby`,
//! builds a `sf-core::DependencyGraph`, pulls churn + coupling signals
//! from `sf-history` over the same repo, scores it with `sf-score`, and
//! prints the
//! `class | score | fan_in | fan_out | complexity | churn | coupling | file`
//! table sorted by (class rank, score desc, path asc). JS/TS files are
//! still unprocessed this slice — they land with `sf-js` in a later PR.

// Tests sit mid-file so the exclusion regression lives right next to
// the `analyze_ruby_source` function it exercises; print helpers follow.
#![allow(clippy::items_after_test_module)]

mod output;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use clap::Parser;
use sf_core::{DependencyGraph, FileRef, Language};
use sf_history::{DEFAULT_WINDOW_DAYS, HistoryWarning};
use sf_score::{CouplingSignal, ScoredFile, Scorer, Signals};
use walkdir::WalkDir;

use output::{OutputFormat, Sections, WarningPayload};

/// Stud-finder version, shared with the Ruby gem. `--version`
/// prints this and exits 0. Keep in sync with
/// `lib/stud_finder/version.rb`.
pub const VERSION: &str = "0.7.0";

#[derive(Parser, Debug)]
#[command(
    name = "stud-finder",
    version = VERSION,
    about = "Rank files by structural risk (Rust port, WIP)."
)]
struct Cli {
    /// Repository path to scan.
    path: Option<PathBuf>,
    /// Path to a Ruby coverage report (SimpleCov `resultset.json`,
    /// LCOV `.info`, or Cobertura XML). Enables the coverage signal
    /// + the interaction cross-term on the Ruby section.
    #[arg(long = "ruby-coverage")]
    ruby_coverage: Option<PathBuf>,
    /// Deprecated alias for `--ruby-coverage`. Emits a
    /// `coverage_flag_deprecated` warning to stderr + `meta.warnings`
    /// and treats the value as if it were passed to `--ruby-coverage`.
    #[arg(long = "coverage")]
    coverage: Option<PathBuf>,
    /// Output format for the final rows. `table` keeps the human
    /// output (default), `json` emits a `{meta, warnings, ruby,
    /// javascript}` root, `csv` and `markdown` emit Ruby-contract
    /// shape.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    /// Keep only the top `N` rows per section (after sorting).
    #[arg(long, value_parser = parse_positive_top)]
    top: Option<usize>,
    /// Only emit rows for files changed between `<REF>` and HEAD.
    #[arg(long)]
    diff_base: Option<String>,
    /// Commit lookback window in days for churn + coupling signals
    /// (Ruby default: 180). Rejects zero (`--churn-days must be
    /// positive`) matching Ruby `validate_threshold!`.
    #[arg(long, default_value_t = DEFAULT_WINDOW_DAYS, value_parser = parse_positive_churn_days)]
    churn_days: u64,
    /// Glob exclude pattern, repeatable. Applied AFTER the
    /// built-in default-rule globs (vendor/, node_modules/, etc.).
    /// Paths matching any pattern are dropped from the scored
    /// population.
    #[arg(long = "exclude", value_name = "PATTERN")]
    excludes: Vec<String>,
    /// Emit only these comma-separated repo-relative paths (still
    /// scored against the full repo). Matches Ruby `--only`: the
    /// full repo participates in percentile scoring, but only the
    /// matching rows are emitted.
    #[arg(long)]
    only: Option<String>,
    /// Print suppressed per-file warnings to stderr. In Ruby this
    /// toggles additional stderr detail; in Rust most warnings are
    /// already emitted — the flag is accepted for parity and will
    /// expand verbosity in follow-ups.
    #[arg(long)]
    verbose: bool,
    /// Percentile cutoff for the `trunk` class (default 85). A file
    /// whose composite percentile is >= this value classifies as
    /// trunk. Must be greater than `--branch-threshold`.
    #[arg(long, default_value_t = 85, value_parser = parse_percentile_threshold)]
    trunk_threshold: usize,
    /// Percentile cutoff for the `branch` class (default 50). A
    /// file whose composite percentile is >= this value and
    /// < trunk-threshold classifies as branch. Below → leaf.
    #[arg(long, default_value_t = 50, value_parser = parse_percentile_threshold)]
    branch_threshold: usize,
    /// Days-since-first-commit window for the newness signal.
    /// Files with first-commit age ≤ this value are candidates for
    /// new-file reclassification. Ruby accepts zero for this field
    /// (disables the age half of the recency check). Default: 30.
    /// Caught as Ohm's P2 on PR #88 — prior revision rejected zero.
    #[arg(long, default_value_t = sf_newness::DEFAULT_DAYS)]
    new_file_days: u64,
    /// Minimum total commits for a file to leave the "new" bucket.
    /// A file with fewer commits (and within `--new-file-days`)
    /// still counts as new. Ruby accepts zero (disables the commit
    /// half of the recency check). Default: 3. Caught as Ohm's P2
    /// on PR #88 — prior revision rejected zero.
    #[arg(long, default_value_t = sf_newness::DEFAULT_MIN_COMMITS)]
    new_file_min_commits: usize,
    /// Disable the newness signal entirely. Skips the sf-newness
    /// analyze call; every row reports `new_file: false`, no
    /// reclassification, evidence confidence falls back to the
    /// scorer's placeholder.
    #[arg(long = "no-newness", action = clap::ArgAction::SetTrue)]
    no_newness: bool,
    /// Advisory minimum file count. When the scored population is
    /// smaller than this value a `small_repo` warning is emitted
    /// (percentile ranks are unreliable at small scales).
    /// Ruby default: 20.
    #[arg(long, default_value_t = 20, value_parser = parse_positive_usize)]
    min_files: usize,
    /// Coupling ratio threshold. Pairs with ratio below this are
    /// dropped from the coupling signal. Ruby default: 0.30. Must
    /// fall in `[0.0, 1.0]`.
    #[arg(long, default_value_t = sf_history::COUPLING_THRESHOLD, value_parser = parse_unit_float)]
    coupling_threshold: f64,
    /// Minimum co-change count for a pair to be considered. Pairs
    /// with fewer co-changes than this are dropped. Ruby default: 5.
    #[arg(long, default_value_t = sf_history::MIN_CO_CHANGES, value_parser = parse_positive_usize)]
    coupling_min_commits: usize,
    /// Bulk-commit cutoff: commits touching strictly more files
    /// than this (within the scored set) are skipped from coupling.
    /// They still contribute to churn. Ruby default: 50. **Zero
    /// disables the cap** — every commit participates in coupling
    /// regardless of size. Caught as Ohm's P2 on PR #89: prior
    /// revision rejected zero.
    #[arg(long, default_value_t = sf_history::BULK_COMMIT_THRESHOLD)]
    coupling_max_commit_files: usize,
    /// Disable Rails association/string fan-in inference. Accepted
    /// for parity with Ruby — the Rust port does not yet perform
    /// Rails-specific inference, so this flag is a no-op today. It
    /// is wired here so automation targeting Ruby's option set
    /// passes through without failing.
    #[arg(long = "no-rails-inference", action = clap::ArgAction::SetTrue)]
    no_rails_inference: bool,
    /// dependency-cruiser timeout in seconds. Accepted for parity
    /// with Ruby — the Rust port uses oxc_parser directly rather
    /// than shelling out to dependency-cruiser, so this flag is a
    /// no-op today. Kept so Ruby-matching automation does not break.
    #[arg(long, default_value_t = 60, value_parser = parse_positive_u64)]
    js_timeout: u64,
    /// Full weight override: comma-separated `key:float` pairs. Required
    /// keys: `fan_in`, `fan_out`, `complexity`, `churn`, `coverage`.
    /// Optional: `interaction` (defaults to 0.0), `coupling` (defaults
    /// to 0.05). All values in `[0.0, 1.0]`. Matches Ruby `parse_weights`.
    /// When supplied together with `--interaction-weight` or
    /// `--coupling-weight` the per-key flags override the matching
    /// entry from `--weights`.
    #[arg(long)]
    weights: Option<String>,
    /// `fan_in × coverage_risk` interaction weight. Requires coverage;
    /// if `--weights` is also set this overrides its `interaction:` entry.
    #[arg(long, value_parser = parse_unit_float)]
    interaction_weight: Option<f64>,
    /// Temporal-coupling weight. If `--weights` is set this overrides
    /// its `coupling:` entry.
    #[arg(long, value_parser = parse_unit_float)]
    coupling_weight: Option<f64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));

    // Collected warning payloads flow to stderr AND Sections.warnings.
    let mut warnings: Vec<WarningPayload> = Vec::new();

    // Coverage flag resolution:
    //  - `--coverage` used: ALWAYS emit the `coverage_flag_deprecated`
    //    warning (Ruby emits it each time the option is parsed,
    //    regardless of whether a later `--ruby-coverage` wins).
    //  - `--coverage` AND `--ruby-coverage` together: reject hard.
    //    Ruby takes last-option-wins; without ARGV order tracking
    //    we match Ruby's SPIRIT (don't silently pick the wrong
    //    path) by refusing the ambiguity.
    // Caught as Ohm's P2 on PR #87.
    if cli.coverage.is_some() {
        eprintln!(
            "Warning: coverage_flag_deprecated: --coverage is deprecated; use --ruby-coverage."
        );
        warnings.push(WarningPayload {
            code: "coverage_flag_deprecated".into(),
            message: "--coverage is deprecated; use --ruby-coverage instead".into(),
        });
    }
    let coverage_path: Option<PathBuf> = match (cli.ruby_coverage.as_ref(), cli.coverage.as_ref()) {
        (Some(_), Some(_)) => {
            eprintln!(
                "Error: --ruby-coverage and --coverage are mutually exclusive; pass only one."
            );
            std::process::exit(2);
        }
        (Some(p), None) | (None, Some(p)) => Some(p.clone()),
        (None, None) => None,
    };

    let collect_result = collect(&path, &cli.excludes);
    let files = collect_result.files;
    let default_excluded_count = collect_result.default_excluded;

    // Ruby `validate_filter_options!` rejects --diff-base combined
    // with --only (cli.rb:395-396). Match the same hard error so
    // automation that passes both by mistake fails loudly rather
    // than getting a logical-AND surprise. Caught as Ohm's P2 on
    // PR #87.
    if cli.diff_base.is_some() && cli.only.is_some() {
        eprintln!("Error: --diff-base and --only are mutually exclusive.");
        std::process::exit(2);
    }

    // Ruby `Scorer#validate!` requires branch < trunk; raise with
    // the exact Ruby error message so automation parsers can
    // string-match.
    if cli.branch_threshold >= cli.trunk_threshold {
        eprintln!("Error: branch-threshold must be strictly less than trunk-threshold.");
        std::process::exit(2);
    }

    // Parse `--only` into a list once. Ruby `value.split(',').map(&:strip)
    // .reject(&:empty?)` returns an empty array when the user passes
    // `--only ""` or `--only ","`; we match that (empty filter → emit
    // nothing + diff_filter_empty warning path downstream) rather
    // than rejecting at parse time.
    let only_paths_raw: Option<Vec<String>> = cli.only.as_deref().map(parse_only_paths);
    // Rebase repo-root-relative --only paths to the analysis root
    // when the scan root is a subdirectory, matching Ruby
    // `resolve_filter_set` → `rebase_to_analysis_root`. A scan of
    // REPO/lib with `--only lib/foo.rb` must compare against
    // `foo.rb` (the scan-root-relative row key), not `lib/foo.rb`.
    // Caught as Ohm's P2 on PR #87.
    let only_paths: Option<Vec<String>> = only_paths_raw
        .as_ref()
        .map(|paths| rebase_filter_paths(paths, &path));
    // Score the Ruby graph on its own — JS/TS files are unprocessed in
    // this slice and must NOT participate in the percentile calculation
    // (they would otherwise pin a fan_in=0/fan_out=0 row into the
    // distribution and depress Ruby percentiles). sf-js builds its own
    // graph in a later slice; each language gets its own ranking
    // section, matching the Ruby implementation.
    let (graph, complexity, ruby_skipped) = build_signals(&path, &files);

    // Pull churn + coupling from git history over the same registered
    // Ruby files. A non-git-repo or missing `git` degrades to zero
    // signals with a stderr warning — the run still completes.
    let ruby_paths: Vec<PathBuf> = graph.files().map(Path::to_path_buf).collect();
    let ruby_refs: Vec<&Path> = ruby_paths.iter().map(PathBuf::as_path).collect();
    // Honor --coupling-threshold / --coupling-min-commits /
    // --coupling-max-commit-files by building a HistoryConfig once.
    // Defaults match Ruby's.
    let history_cfg = sf_history::HistoryConfig {
        window_days: cli.churn_days,
        max_commit_files: cli.coupling_max_commit_files,
        min_co_changes: cli.coupling_min_commits,
        coupling_threshold: cli.coupling_threshold,
    };
    let history = sf_history::analyze_with_config(&path, &ruby_refs, history_cfg);
    for warning in &history.warnings {
        match warning {
            HistoryWarning::BulkCommitsSkipped {
                count,
                max_commit_files,
            } => {
                eprintln!(
                    "stud-finder: {count} bulk commits (> {max_commit_files} files) skipped from coupling"
                );
                warnings.push(WarningPayload {
                    code: "temporal_coupling_bulk_commits_skipped".into(),
                    message: format!(
                        "one or more bulk commits were skipped while computing temporal coupling \
                         ({count} skipped; max_commit_files={max_commit_files})"
                    ),
                });
            }
            HistoryWarning::NotAGitRepo => {
                eprintln!(
                    "stud-finder: {} is not a git repository; churn + coupling signals are skipped",
                    path.display()
                );
                warnings.push(WarningPayload {
                    code: "git_error".into(),
                    message: "git command failed while computing temporal coupling; \
                              coupling is unavailable"
                        .into(),
                });
            }
            HistoryWarning::GitNotFound => {
                eprintln!(
                    "stud-finder: `git` not found in PATH; churn + coupling signals are skipped"
                );
                warnings.push(WarningPayload {
                    code: "git_not_found".into(),
                    message: "git was not found in PATH while computing temporal coupling; \
                              coupling is unavailable"
                        .into(),
                });
            }
            HistoryWarning::GitFailed(msg) => {
                eprintln!("stud-finder: git log failed: {msg}");
                warnings.push(WarningPayload {
                    code: "git_error".into(),
                    message: "git command failed while computing temporal coupling; \
                              coupling is unavailable"
                        .into(),
                });
            }
        }
    }
    let coupling: HashMap<PathBuf, CouplingSignal> = history
        .coupling
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                CouplingSignal {
                    partner: v.partner,
                    ratio: v.ratio,
                    co_changes: v.co_changes,
                    partners: v.partners,
                },
            )
        })
        .collect();

    // Coverage (optional, from --coverage flag). `None` means the
    // flag wasn't provided — coverage + interaction stay out of the
    // composite. `Some(_)` activates both signals even if the loaded
    // report ended up empty (Ruby `coverage_available? = !@coverage.nil?`
    // semantics).
    let coverage = load_coverage(coverage_path.as_deref(), &path, &ruby_refs);

    // LOC pass — non-blank line count per Ruby file. Ruby
    // `LocCounter#non_blank_lines`. Unreadable files map to 0 and
    // land in the ScoredFile row anyway so a rename-at-scan-time
    // doesn't drop the row from the output.
    let loc = count_loc(&path, &ruby_refs);

    // Cache the active-signal booleans BEFORE Signals is moved into
    // the scorer — the composite's active-weight renormalization and
    // the `meta.formula` label both read these flags. Keeping them
    // alongside `coverage_available` removes the earlier post-hoc
    // row inspection.
    let has_ruby_complexity = !complexity.is_empty();
    let has_ruby_churn = !history.churn_commits.is_empty() || !history.churn_lines.is_empty();
    let has_ruby_coupling = !coupling.is_empty();
    let signals = Signals {
        graph: &graph,
        complexity,
        churn_commits: history.churn_commits,
        churn_lines: history.churn_lines,
        coupling,
        coverage,
        loc,
    };
    let coverage_for_newness = signals.coverage.clone();
    let coverage_available = signals.coverage.is_some();
    // Honor `--trunk-threshold` + `--branch-threshold` by converting
    // from Ruby's 0-100 percentile integers to the sf-score 0.0-1.0
    // floats the composite classification uses. Custom weights (if
    // any) overlay on top of either `--weights` or the Ruby defaults.
    // Walk raw ARGV so weight flags apply in the user's declared
    // order. Ruby `OptionParser` mutates `@options[:weights]` as it
    // encounters each flag; a later `--weights` REPLACES whatever
    // earlier `--coupling-weight` / `--interaction-weight` set, and
    // vice versa. clap's derive collapses to final values, losing
    // the order needed for parity. Caught as Ohm's P2 on PR #90.
    let (effective_raw, use_custom_weights) = apply_weight_overrides_in_argv_order()
        .unwrap_or_else(|err| {
            eprintln!("{err}");
            std::process::exit(2);
        });
    // Ruby `validate_weights!` fires only when `--weights` provided
    // a full-vector replacement — not for the per-key overlay flags
    // alone. The full-vector check enforces sum-to-1.0 within 0.001
    // AND rejects positive coverage/interaction without coverage.
    // Gate on `--weights` presence, not on `use_custom_weights`.
    // Caught as Ohm's P2 on PR #90.
    let full_weights_override = cli.weights.is_some();
    if full_weights_override {
        let has_coverage_for_validate = cli.ruby_coverage.is_some() || cli.coverage.is_some();
        if let Err(msg) = effective_raw.validate_full_override(has_coverage_for_validate) {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    }
    let _ = cli.interaction_weight;
    let _ = cli.coupling_weight;
    let scorer = {
        let mut s = Scorer::with_floors(
            cli.trunk_threshold as f64 / 100.0,
            cli.branch_threshold as f64 / 100.0,
            sf_score::FAN_IN_FLOOR,
            sf_score::COMPLEXITY_FLOOR,
        );
        if use_custom_weights {
            s = s.with_weights(effective_raw);
        }
        s
    };
    let scored = scorer.score_with_signals(signals);
    let mut ruby_rows = scored;

    // Newness config: honor `--new-file-days` + `--new-file-min-commits`,
    // skip the sf-newness analyze call entirely when `--no-newness`
    // is set (matches Ruby `@options[:newness] = false` which short-
    // circuits `Newness::Analyzer#analyze` and leaves rows with
    // `new_file: false`).
    let newness_cfg = sf_newness::NewnessConfig {
        days: cli.new_file_days,
        min_commits: cli.new_file_min_commits,
        now_epoch: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let newness_report = if cli.no_newness {
        // Empty report with no metadata — apply_newness becomes a
        // no-op pass on the base classification.
        sf_newness::NewnessReport::default()
    } else {
        sf_newness::analyze(&path, &ruby_refs, newness_cfg)
    };
    for warning in &newness_report.warnings {
        match warning {
            sf_newness::NewnessWarning::ShallowClone => {
                eprintln!(
                    "stud-finder: shallow git clone detected; newness rules disabled (use fetch-depth: 0 in CI)"
                );
                warnings.push(WarningPayload {
                    code: "shallow_clone_newness_disabled".into(),
                    message: "shallow git clone detected; newness rules disabled \
                              (use fetch-depth: 0)"
                        .into(),
                });
            }
            sf_newness::NewnessWarning::NotAGitRepo => {
                // No Ruby warning code — Ruby raises on non-repo here. Stderr only.
                eprintln!("stud-finder: not a git repository; newness signals skipped");
            }
            sf_newness::NewnessWarning::GitNotFound => {
                eprintln!("stud-finder: `git` not found in PATH; newness signals skipped");
            }
            sf_newness::NewnessWarning::GitFailed(msg) => {
                eprintln!("stud-finder: git log failed during newness: {msg}");
            }
        }
    }
    sf_score::apply_newness(&mut ruby_rows, &newness_report.metadata, &graph, |p| {
        coverage_for_newness
            .as_ref()
            .map(|m| m.contains_key(p))
            .unwrap_or(false)
    });
    sort_rows(&mut ruby_rows);
    // Assign ranks AFTER the primary sort but BEFORE any filters
    // (`--top`, `--diff-base`). Preserving pre-filter ranks is the
    // Ruby contract: a `--diff-base`-filtered output can legitimately
    // land ranks like 1, 7, 23 rather than being renumbered. Caught
    // as Ohm's P1 on PR #85.
    assign_ranks(&mut ruby_rows);
    let ruby_total_analyzed = ruby_rows.len();

    // Build the JavaScript section on its own graph + complexity so
    // JS files don't pool into the Ruby percentile distribution.
    let (js_graph, js_complexity, js_skipped) = build_js_signals(&path, &files);
    // Per-section signal flags, captured BEFORE Signals is moved
    // into the JS scorer. Needed so a JS-only repository reports JS
    // weights (nonzero complexity) rather than falling back to the
    // Ruby-section flags (all zero). Matches Ruby's
    // `analysis.ruby.weights || analysis.javascript.weights`.
    let has_js_complexity = !js_complexity.is_empty();
    let mut js_rows: Vec<ScoredFile> = if js_graph.files().count() > 0 {
        let js_paths: Vec<PathBuf> = js_graph.files().map(Path::to_path_buf).collect();
        let js_refs: Vec<&Path> = js_paths.iter().map(PathBuf::as_path).collect();
        let js_loc = count_loc(&path, &js_refs);
        let js_signals = Signals {
            graph: &js_graph,
            complexity: js_complexity,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
            loc: js_loc,
        };
        let mut rows = scorer.score_with_signals(js_signals);
        // Reuse the Ruby-section newness report's git run (same repo,
        // supersets fine) rather than re-invoking git for JS. Only
        // JS paths appear in the metadata lookup. Honor
        // --no-newness identically to the Ruby pass.
        let js_newness = if cli.no_newness {
            sf_newness::NewnessReport::default()
        } else {
            sf_newness::analyze(&path, &js_refs, newness_cfg)
        };
        sf_score::apply_newness(&mut rows, &js_newness.metadata, &js_graph, |_| false);
        sort_rows(&mut rows);
        assign_ranks(&mut rows);
        rows
    } else {
        Vec::new()
    };
    let js_total_analyzed = js_rows.len();

    // --- `--only` filter. Ruby and this port both reject combining
    //     with --diff-base, so --only is always applied standalone
    //     (gated by the mutual-exclusion check above). Paths were
    //     already rebased from repo-root to scan-root earlier.
    if let Some(only) = only_paths.as_ref() {
        use std::collections::HashSet;
        // Empty --only list (user passed `--only ""` or `","`) is a
        // valid filter meaning "match nothing" — Ruby emits zero rows
        // with a `diff_filter_empty` warning. Emit the warning and
        // clear both section row vectors.
        if only.is_empty() {
            eprintln!("Note: --only was empty; nothing to filter on.");
            warnings.push(WarningPayload {
                code: "diff_filter_empty".into(),
                message: "diff filter matched no changed files; output rows are empty".into(),
            });
            ruby_rows.clear();
            js_rows.clear();
        } else {
            let keep: HashSet<PathBuf> = only.iter().map(PathBuf::from).collect();
            let before_ruby = ruby_rows.len();
            let before_js = js_rows.len();
            ruby_rows.retain(|r| keep.contains(&r.path));
            js_rows.retain(|r| keep.contains(&r.path));
            if ruby_rows.is_empty() && js_rows.is_empty() && (before_ruby + before_js) > 0 {
                eprintln!(
                    "Note: --only matched no scored files. The listed paths may be unscorable (docs, config, migrations) or outside this scan root."
                );
                warnings.push(WarningPayload {
                    code: "diff_no_scored_files".into(),
                    message: "diff matched no scored files; the PR may only touch unscorable files"
                        .into(),
                });
            }
        }
    }

    // --- Diff-base filter (applied BEFORE --top) ---
    // An explicitly-requested --diff-base that CAN'T be satisfied
    // (missing git, non-repo, bad ref) is a hard failure: exit
    // non-zero rather than silently emitting the whole repo. Matches
    // Ruby `Diff` which raises on the same conditions. An empty-but-
    // successful diff is a valid filter — the result is the empty
    // set, not "no filter".
    if let Some(base) = cli.diff_base.as_deref() {
        match diff_names(&path, base) {
            Ok(changed) => {
                ruby_rows.retain(|r| changed.contains(&r.path));
                js_rows.retain(|r| changed.contains(&r.path));
            }
            Err(err) => {
                eprintln!("stud-finder: --diff-base failed: {err}");
                std::process::exit(2);
            }
        }
    }

    // --- Top-N filter (applied per section, AFTER diff filter) ---
    if let Some(n) = cli.top {
        ruby_rows.truncate(n);
        js_rows.truncate(n);
    }

    // --- Emit ---
    let analyzed_at = current_iso8601_utc();
    let analyzed_date = analyzed_at.get(0..10).unwrap_or(&analyzed_at).to_string();
    // Active-signal booleans drive the `meta.weights` renormalization
    // and the `meta.formula` label. Ruby `report_coverage_available?`
    // is the union of per-section availability — a Ruby-only coverage
    // run gets the 5-factor formula even though JS rows carry null
    // coverage.
    let has_coverage = output::report_coverage_available(coverage_available, false);
    // Ruby `analysis.ruby.weights || analysis.javascript.weights` —
    // pick the Ruby section's effective weights when a Ruby analysis
    // ran (ruby_rows non-empty), otherwise fall back to the JS
    // section's. A JS-only repository must report the JS-section
    // weights (nonzero complexity) rather than the Ruby-section
    // weights (all zero). Caught as Ohm's P2 on PR #86.
    let ruby_section_present = ruby_total_analyzed > 0;
    let (has_complexity, has_churn, has_coupling) = if ruby_section_present {
        (has_ruby_complexity, has_ruby_churn, has_ruby_coupling)
    } else {
        // JS section's signal state: complexity derived from the
        // non-empty js_complexity map; churn + coupling never
        // populated for JS in this slice.
        (has_js_complexity, false, false)
    };
    let _ = (has_complexity, has_churn);
    let weights = if use_custom_weights {
        sf_score::effective_weights_from(&effective_raw, has_coupling, has_coverage)
    } else {
        sf_score::effective_weights(true, true, has_coupling, has_coverage)
    };
    // Formula label gates `+ coupling` on BOTH the signal being
    // present AND the effective coupling weight being positive —
    // Ruby `json_formula` reads `normalized_weights[:coupling] > 0`.
    // A `--coupling-weight 0` run with coupling data still labels
    // as `4-factor` because coupling contributes zero. Caught as
    // Ohm's P2 on PR #90.
    let coupling_contributes = weights.coupling.map(|c| c > 0.0).unwrap_or(false);
    let formula = output::formula_label(has_coverage, coupling_contributes);
    // Ruby `meta.filtered` is set ONLY when `filter_set` is
    // populated — i.e. `--diff-base` or `--only`. `--top` is a
    // truncation knob, not a filter. Caught as Ohm's P2 on PR #86.
    // `meta.only_paths` must carry the USER-SUPPLIED paths unchanged
    // (Ruby `json_meta` emits `@options[:only_paths]` verbatim). The
    // rebased list is for filter matching only. Caught as Ohm's P2
    // on PR #87 round 2.
    let filter_note = build_filter_note(cli.diff_base.as_deref(), only_paths_raw.as_deref());
    let filtered = cli.diff_base.is_some() || only_paths_raw.is_some();
    let repo_display = path.display().to_string();
    let only_slice: Option<&[String]> = only_paths_raw.as_deref();

    // `--min-files` advisory: Ruby `score_group` adds `small_repo`
    // when ANY non-empty scored group is below the threshold —
    // percentile reliability is per-section, not per-run, because
    // Ruby + JS score in separate populations. A 12 Ruby + 12 JS
    // repo with --min-files 20 trips the warning even though the
    // combined count is 24. Caught as Ohm's P2 on PR #88 — prior
    // revision used the combined count. Emit at most one warning
    // (dedup is implicit — Warnings.normalize does it in Ruby; our
    // Vec<WarningPayload> normalizes by uniq-by-code at JSON emit).
    let small_sections: Vec<(&str, usize)> = [
        ("ruby", ruby_total_analyzed),
        ("javascript", js_total_analyzed),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0 && *n < cli.min_files)
    .collect();
    if !small_sections.is_empty() {
        for (lang, n) in &small_sections {
            eprintln!(
                "Warning: only {n} {lang} files found. Percentile ranks are unreliable at this scale. Results are advisory only."
            );
        }
        warnings.push(WarningPayload {
            code: "small_repo".into(),
            message: "repo has fewer files than --min-files; results may be noisy".into(),
        });
    }
    output::emit(
        cli.output,
        Sections {
            ruby: &ruby_rows,
            js: &js_rows,
            repo: &repo_display,
            churn_days: cli.churn_days,
            analyzed_at: &analyzed_at,
            analyzed_date: &analyzed_date,
            warnings: &warnings,
            file_count: ruby_total_analyzed + js_total_analyzed,
            files_skipped: ruby_skipped + js_skipped,
            default_excluded_count,
            formula,
            weights: &weights,
            filter_note: filter_note.as_deref(),
            diff_base: cli.diff_base.as_deref(),
            only_paths: only_slice,
            filtered,
            ruby_coverage_available: coverage_available,
            js_coverage_available: false,
        },
    );

    Ok(())
}

/// Build the Ruby `filter_note` string — attached below the markdown
/// blockquote and the table header when a filter is active. Preserves
/// the exact Ruby wording from `lib/stud_finder/cli.rb`.
fn build_filter_note(diff_base: Option<&str>, only_paths: Option<&[String]>) -> Option<String> {
    if let Some(base) = diff_base {
        return Some(format!(
            "Filtered to files changed vs {base} (ranks are against the full repo)."
        ));
    }
    if only_paths.is_some() {
        return Some("Filtered to --only paths (ranks are against the full repo).".to_string());
    }
    None
}

/// Walk the sorted rows and assign 1-based ranks. Must be called
/// AFTER the primary sort and BEFORE any filters so filtered
/// output preserves the Ruby-contract rank values.
fn assign_ranks(rows: &mut [ScoredFile]) {
    for (i, r) in rows.iter_mut().enumerate() {
        r.rank = i + 1;
    }
}

/// Minimal RFC3339-shaped UTC timestamp for `meta.analyzed_at`.
/// Matches Ruby `Time.now.utc.iso8601` for the dotless-second form
/// (2026-11-13T03:14:05Z). Not a dependency on chrono — this is one
/// field used in one place.
fn current_iso8601_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = unix_epoch_to_utc_components(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert Unix epoch seconds to UTC (year, month, day, hour, minute, second).
/// Standard civil-from-days algorithm — valid for epochs well beyond 2100.
fn unix_epoch_to_utc_components(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;

    // Howard Hinnant's civil_from_days, shifted to a 0000-03-01 era.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (if m <= 2 { y + 1 } else { y }) as i32;
    (year, m, d, hour, minute, second)
}

/// Parse Ruby's `--only PATHS` comma-separated list, matching
/// `value.split(',').map(&:strip).reject(&:empty?)` from
/// `lib/stud_finder/cli.rb`. An all-empty input (`""` or `","`)
/// returns `Vec::new()` — Ruby accepts the empty list and the
/// downstream filter emits zero rows with a `diff_filter_empty`
/// warning. Caught as Ohm non-blocking on PR #87 (parse_only_paths
/// contract divergence).
fn parse_only_paths(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Rebase repo-root-relative filter paths to the analysis root,
/// matching Ruby `rebase_to_analysis_root` in cli.rb. When the scan
/// root is a subdirectory of the git repo, the prefix is stripped
/// so filter paths compare equal to the scan-root-relative row
/// keys. A scan rooted at the repo root (or non-git) returns paths
/// unchanged.
fn rebase_filter_paths(paths: &[String], scan_root: &Path) -> Vec<String> {
    let toplevel = match git_toplevel(scan_root) {
        Some(t) => t,
        None => return paths.to_vec(),
    };
    let scan_abs = match std::fs::canonicalize(scan_root) {
        Ok(p) => p,
        Err(_) => return paths.to_vec(),
    };
    if scan_abs == toplevel {
        return paths.to_vec();
    }
    let Ok(prefix_path) = scan_abs.strip_prefix(&toplevel) else {
        return paths.to_vec();
    };
    let prefix = format!("{}/", prefix_path.display());
    if prefix == "./" || prefix.is_empty() {
        return paths.to_vec();
    }
    // `strip_prefix` removes the prefix EXACTLY once, matching Ruby
    // `delete_prefix`. Prior revision used `trim_start_matches`,
    // which strips repeated instances — a path like
    // `lib/lib/foo.rb` under scan root `lib/` would collapse to
    // `foo.rb` instead of `lib/foo.rb`. Caught as Ohm's P2 on
    // PR #87 round 2.
    paths
        .iter()
        .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
        .collect()
}

fn git_toplevel(scan_root: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(scan_root)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = std::str::from_utf8(&output.stdout).ok()?.trim();
    std::fs::canonicalize(s).ok()
}

/// Parser for `--churn-days`: rejects zero to match Ruby
/// `validate_threshold!` which raises `--churn-days must be
/// positive` on <= 0. Caught as Ohm's P2 on PR #87.
fn parse_positive_churn_days(s: &str) -> Result<u64, String> {
    let n: u64 = s
        .parse()
        .map_err(|_| "--churn-days must be a positive integer".to_string())?;
    if n == 0 {
        return Err("--churn-days must be positive".to_string());
    }
    Ok(n)
}

/// Generic positive-u64 parser (for `--js-timeout`).
fn parse_positive_u64(s: &str) -> Result<u64, String> {
    let n: u64 = s
        .parse()
        .map_err(|_| "value must be a positive integer".to_string())?;
    if n == 0 {
        return Err("value must be positive".to_string());
    }
    Ok(n)
}

/// Parse Ruby's `--weights` DSL: `fan_in:F,fan_out:O,complexity:C,
/// churn:H,coverage:V[,interaction:I][,coupling:P]`. Required 5 keys
/// must appear, extras must be in the optional set, values must be
/// floats in `[0.0, 1.0]`. Missing `interaction` defaults to 0.0;
/// missing `coupling` defaults to 0.05 — matches Ruby `parse_weights`
/// at `lib/stud_finder/cli.rb:322`.
/// Walk `std::env::args()` and apply weight flags in the order
/// they appear. Returns `(RawWeights, custom_applied)` — the
/// boolean is true when any of the three flags was present.
///
/// Semantics match Ruby `OptionParser`:
/// - `--weights W` REPLACES the entire raw-weights hash with the
///   parsed one.
/// - `--interaction-weight N` / `--coupling-weight N` overwrite
///   only that key on whatever hash is currently in place.
/// - Later flags beat earlier flags verbatim.
///
/// Caught as Ohm's P2 on PR #90 (pre-fix clap-derive version
/// always layered the per-key flags last, regardless of argv order).
fn apply_weight_overrides_in_argv_order() -> Result<(sf_score::RawWeights, bool), String> {
    let argv: Vec<String> = std::env::args().collect();
    let mut raw = sf_score::RawWeights::default();
    let mut any = false;
    let mut i = 0usize;
    while i < argv.len() {
        let arg = &argv[i];
        // Support `--flag value` and `--flag=value` for each of the
        // three weight flags.
        let (key, value): (Option<&str>, Option<String>) =
            if let Some(rest) = arg.strip_prefix("--weights=") {
                (Some("weights"), Some(rest.to_string()))
            } else if arg == "--weights" {
                let v = argv.get(i + 1).cloned();
                i += 1;
                (Some("weights"), v)
            } else if let Some(rest) = arg.strip_prefix("--interaction-weight=") {
                (Some("interaction"), Some(rest.to_string()))
            } else if arg == "--interaction-weight" {
                let v = argv.get(i + 1).cloned();
                i += 1;
                (Some("interaction"), v)
            } else if let Some(rest) = arg.strip_prefix("--coupling-weight=") {
                (Some("coupling"), Some(rest.to_string()))
            } else if arg == "--coupling-weight" {
                let v = argv.get(i + 1).cloned();
                i += 1;
                (Some("coupling"), v)
            } else {
                (None, None)
            };
        if let (Some(k), Some(v)) = (key, value) {
            match k {
                "weights" => {
                    raw = parse_weights_flag(&v)?;
                    any = true;
                }
                "interaction" => {
                    let n: f64 = parse_unit_float(&v)?;
                    raw.interaction = n;
                    any = true;
                }
                "coupling" => {
                    let n: f64 = parse_unit_float(&v)?;
                    raw.coupling = n;
                    any = true;
                }
                _ => unreachable!(),
            }
        }
        i += 1;
    }
    Ok((raw, any))
}

fn parse_weights_flag(s: &str) -> Result<sf_score::RawWeights, String> {
    let mut map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for entry in s.split(',') {
        let mut parts = entry.splitn(2, ':');
        let key = parts.next().unwrap_or("").trim().to_string();
        let raw = parts.next().unwrap_or("").trim();
        if key.is_empty() || raw.is_empty() {
            return Err("Error: invalid weights format.".to_string());
        }
        let v: f64 = raw
            .parse()
            .map_err(|_| "Error: weight values must be floats.".to_string())?;
        map.insert(key, v);
    }
    let required = ["fan_in", "fan_out", "complexity", "churn", "coverage"];
    let optional = ["interaction", "coupling"];
    if required.iter().any(|k| !map.contains_key(*k)) {
        return Err(
            "Error: weights must include fan_in, fan_out, complexity, churn, and coverage."
                .to_string(),
        );
    }
    let allowed: std::collections::HashSet<&str> =
        required.iter().chain(optional.iter()).copied().collect();
    if map.keys().any(|k| !allowed.contains(k.as_str())) {
        return Err(
            "Error: weights must include fan_in, fan_out, complexity, churn, and coverage."
                .to_string(),
        );
    }
    // Ruby `Float(raw)` rejects `NaN` / `Infinity` strings with
    // ArgumentError → the parse_weights rescue branch surfaces as
    // "weight values must be floats." Match that semantic by
    // treating non-finite weights as parse errors, not range
    // errors. Caught as Ohm's P1 on PR #90.
    if map.values().any(|v| !v.is_finite()) {
        return Err("Error: weight values must be floats.".to_string());
    }
    if map.values().any(|v| *v < 0.0 || *v > 1.0) {
        return Err("Error: weight values must be between 0.0 and 1.0.".to_string());
    }
    Ok(sf_score::RawWeights {
        fan_in: map["fan_in"],
        fan_out: map["fan_out"],
        complexity: map["complexity"],
        churn: map["churn"],
        coverage: map["coverage"],
        interaction: *map.get("interaction").unwrap_or(&0.0),
        coupling: *map.get("coupling").unwrap_or(&0.05),
    })
}

/// Parser for weight-shaped Floats in `[0.0, 1.0]`. Matches Ruby
/// `weight values must be between 0.0 and 1.0.` phrasing.
fn parse_unit_float(s: &str) -> Result<f64, String> {
    let n: f64 = s.parse().map_err(|_| "value must be a float".to_string())?;
    if !(0.0..=1.0).contains(&n) {
        return Err("value must be between 0.0 and 1.0".to_string());
    }
    Ok(n)
}

/// Generic positive-usize parser.
fn parse_positive_usize(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| "value must be a positive integer".to_string())?;
    if n == 0 {
        return Err("value must be positive".to_string());
    }
    Ok(n)
}

/// Parser for `--trunk-threshold` / `--branch-threshold`. Ruby
/// `validate_threshold!` allows percentile values in `0..=100` and
/// requires branch < trunk (checked later at run time). Reject
/// out-of-range values with Ruby's exact phrasing: "`<flag>` must
/// be between 0 and 100".
fn parse_percentile_threshold(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| "threshold must be an integer between 0 and 100".to_string())?;
    if n > 100 {
        return Err("threshold must be between 0 and 100".to_string());
    }
    Ok(n)
}

fn parse_positive_top(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| "--top must be a positive integer".to_string())?;
    if n == 0 {
        // Matches Ruby `--top must be positive` validation. Avoids the
        // "0 = unlimited" ambiguity that masks a caller passing 0 by
        // mistake.
        return Err("--top must be positive".to_string());
    }
    Ok(n)
}

fn sort_rows(rows: &mut [ScoredFile]) {
    rows.sort_by(|a, b| {
        a.class
            .rank()
            .cmp(&b.class.rank())
            .then_with(|| {
                b.composite
                    .partial_cmp(&a.composite)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Returns the set of SCAN-ROOT-RELATIVE paths that changed between
/// `base` and HEAD. Uses `--relative` so paths are emitted relative
/// to the `-C` directory (= the scan root), matching the identity the
/// CLI uses for scored rows — fixes nested-scan drops where the scan
/// root is a subdirectory of the repo (Ohm's P1 on PR #82). `-z`
/// makes the stream NUL-delimited so paths with tabs, newlines, or
/// non-ASCII bytes come through unquoted — git's default output
/// quotes such paths, which broke the previous lossy-UTF8 split.
///
/// Returns `Err` on missing git, non-repo, bad ref, any other git
/// failure, or non-UTF8 path bytes we can't form a `PathBuf` from.
/// An empty but successful diff returns `Ok(empty set)`, NOT no-
/// filter — matches Ruby `Diff` semantics so "nothing changed" means
/// zero scored rows, not the whole repo.
fn diff_names(repo_root: &Path, base: &str) -> Result<HashSet<PathBuf>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--name-only")
        .arg("-z")
        .arg("--relative")
        .arg(format!("{base}...HEAD"))
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(o) => o,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err("git not found in PATH".to_string());
        }
        Err(err) => return Err(err.to_string()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    // `-z` splits records on NUL, no trailing NUL required but
    // typically present. Each record is the raw path bytes — no git
    // quoting applied. Skip empties from a trailing NUL.
    let mut changed: HashSet<PathBuf> = HashSet::new();
    for chunk in output.stdout.split(|&b| b == 0u8) {
        if chunk.is_empty() {
            continue;
        }
        // Our file-set is keyed by `PathBuf`s built from `&str` via
        // walkdir -> strip_prefix. A non-UTF8 path on disk would
        // already fail that pipeline on any platform, so refuse
        // non-UTF8 here too rather than let a mismatched pair
        // silently drop a row.
        match std::str::from_utf8(chunk) {
            Ok(s) => {
                changed.insert(PathBuf::from(s));
            }
            Err(_) => {
                return Err(format!(
                    "git diff emitted a path with non-UTF8 bytes ({} bytes); unsupported",
                    chunk.len()
                ));
            }
        }
    }
    Ok(changed)
}

fn build_js_signals(
    root: &Path,
    files: &[FileRef],
) -> (DependencyGraph, HashMap<PathBuf, usize>, usize) {
    let mut graph = DependencyGraph::new();
    let mut complexity: HashMap<PathBuf, usize> = HashMap::new();
    let mut skipped = 0usize;

    // Pre-flight parse each JS file so we know which ones to EXCLUDE
    // from the scored population. Matches Ohm's exclusion-not-
    // imputation policy from PR #78, now applied to JS: a file that
    // fails to parse must not be registered + must not sink its
    // neighbours' percentiles. Fixes Ohm's parse-error zero-imputation
    // callout on PR #81.
    let mut included: Vec<(&Path, String, usize)> = Vec::new();
    for file in files.iter().filter(|f| f.language == Language::JavaScript) {
        let abs = root.join(&file.path);
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("stud-finder: read {}: {}", abs.display(), err);
                skipped += 1;
                continue;
            }
        };
        let report = sf_js::max_complexity(&source, Some(file.path.as_path()));
        if !report.parsed_cleanly {
            eprintln!(
                "stud-finder: {} had parse errors; excluding the file from the JS scored population",
                file.path.display()
            );
            skipped += 1;
            continue;
        }
        included.push((file.path.as_path(), source, report.complexity));
    }

    // Register only the Included files, then wire path-exact edges.
    for (path, _, _) in &included {
        graph.register_file(path.to_path_buf());
    }
    for (path, source, cx) in &included {
        complexity.insert(path.to_path_buf(), *cx);
        let refs = sf_js::extract_refs(path, source);
        for r in refs {
            if let Some(resolved) = resolve_js_module(&graph, &r.target) {
                // Path-exact edge — bypasses sf-core's Zeitwerk
                // basename resolver so `a/index.js` and `b/index.js`
                // stay distinct. Fixes Ohm's P1 on PR #81.
                graph.add_path_edge(path.to_path_buf(), resolved);
            }
        }
    }

    (graph, complexity, skipped)
}

const JS_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

/// Resolves a target produced by `sf_js::extract_refs` against the
/// files registered in `graph`. Order (Ohm's P1 on PRs #81):
///   1. If the target already carries ANY extension (JS or not),
///      accept ONLY a registered file at that exact path. Never
///      rewrite the suffix — `./foo.js` with only `foo.ts` registered
///      produces no edge, and `./foo.json` cannot be pulled into
///      `foo.ts`.
///   2. Extensionless target: try each JS extension suffix.
///   3. Still no match on an extensionless target: try
///      `<target>/index.<ext>` for each extension.
fn resolve_js_module(graph: &DependencyGraph, target: &Path) -> Option<PathBuf> {
    let has_any_ext = target.extension().is_some();

    if has_any_ext {
        // Explicit extension — any extension — is respected exactly.
        // No suffix substitution, no index fallback.
        if graph.files().any(|f| f == target) {
            return Some(target.to_path_buf());
        }
        return None;
    }

    // Extensionless target: try each JS extension.
    for ext in JS_EXTS {
        let candidate = target.with_extension(ext);
        if graph.files().any(|f| f == candidate.as_path()) {
            return Some(candidate);
        }
    }
    // Directory-style: `<target>/index.<ext>`.
    for ext in JS_EXTS {
        let candidate = target.join(format!("index.{ext}"));
        if graph.files().any(|f| f == candidate.as_path()) {
            return Some(candidate);
        }
    }
    None
}

/// Ruby `FileCollector::DEFAULT_EXCLUDES` verbatim — glob patterns
/// dropped before the user's `--exclude` list. Keep in sync with
/// `lib/stud_finder/file_collector.rb`.
const DEFAULT_EXCLUDES: &[&str] = &[
    "db/schema.rb",
    "db/migrate/**",
    "node_modules/**",
    "**/node_modules/**",
    "vendor/**",
    "**/*.min.js",
    "tmp/**",
    "log/**",
    "coverage/**",
    "spec/**",
    "test/**",
    "__tests__/**",
    "**/__tests__/**",
    "**/*.test.js",
    "**/*.test.ts",
    "**/*.test.jsx",
    "**/*.test.tsx",
    "**/*.spec.js",
    "**/*.spec.ts",
    "**/*.spec.jsx",
    "**/*.spec.tsx",
];

pub struct CollectResult {
    pub files: Vec<FileRef>,
    pub default_excluded: usize,
    pub custom_excluded: usize,
}

fn collect(root: &Path, user_excludes: &[String]) -> CollectResult {
    let mut files = Vec::new();
    let mut default_excluded = 0usize;
    let mut custom_excluded = 0usize;
    for result in WalkDir::new(root) {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "stud-finder: skip {}: {}",
                    err.path()
                        .map(Path::display)
                        .map_or_else(|| "<unknown>".to_string(), |p| p.to_string()),
                    err
                );
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let abs_path = entry.path();
        let ext = abs_path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let language = match ext {
            "rb" => Language::Ruby,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Language::JavaScript,
            _ => continue,
        };
        let relative = abs_path
            .strip_prefix(root)
            .unwrap_or(abs_path)
            .to_path_buf();
        let relative_str = relative.to_string_lossy();

        // Default-rule globs first (matches Ruby precedence) — also
        // mark auto-generated files as default-excluded.
        if excluded_by_patterns(&relative_str, DEFAULT_EXCLUDES) || is_auto_generated(abs_path) {
            default_excluded += 1;
            continue;
        }
        // User `--exclude` patterns.
        if !user_excludes.is_empty() && user_excludes.iter().any(|p| glob_match(p, &relative_str)) {
            custom_excluded += 1;
            continue;
        }

        files.push(FileRef {
            path: relative,
            language,
        });
    }
    CollectResult {
        files,
        default_excluded,
        custom_excluded,
    }
}

fn excluded_by_patterns(relative: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| glob_match(p, relative))
}

/// Ruby `File.fnmatch(pattern, relative, FNM_PATHNAME | FNM_DOTMATCH)`
/// plus Ruby's `globstar_directory_match?` and the suffix-`/**`
/// prefix fallback from `FileCollector#glob_match?`.
///
/// Uses `glob::Pattern::matches_with` with
/// `require_literal_separator = true` so `*` does NOT cross `/` —
/// Ruby's `FNM_PATHNAME` semantics. Prior revision used
/// `MatchOptions::new()` whose default `require_literal_separator`
/// is false, so `lib/*.rb` matched `lib/deep/keep.rb`. Caught as
/// Ohm's P1 on PR #87.
///
/// Two Ruby-specific extensions handle:
///   - `**/foo/**` → match when `relative` is `foo/...` or
///     `.../foo/...`
///   - `foo/**` → match when `relative` starts with `foo/`
fn glob_match(pattern: &str, relative: &str) -> bool {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    if let Ok(p) = glob::Pattern::new(pattern) {
        if p.matches_with(relative, options) {
            return true;
        }
    }
    if let Some(dir) = pattern
        .strip_prefix("**/")
        .and_then(|s| s.strip_suffix("/**"))
    {
        if relative.starts_with(&format!("{dir}/")) || relative.contains(&format!("/{dir}/")) {
            return true;
        }
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        if relative.starts_with(&format!("{prefix}/")) {
            return true;
        }
    }
    false
}

/// Ruby `auto_generated?` — scan the first non-blank line and
/// check for the regex `/\A\s*#\s*This file is auto-generated/i`.
/// Prior revision tested a literal `# this file is auto-generated`
/// prefix, so `#This ...` (no space) and `#   This ...` (multiple
/// spaces) slipped through. Ruby's `\s*` matches zero or more
/// whitespace after the `#`. Caught as Ohm's P2 on PR #87.
fn is_auto_generated(abs: &Path) -> bool {
    let Ok(contents) = std::fs::read(abs) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&contents) else {
        return false;
    };
    const SENTINEL: &str = "this file is auto-generated";
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut rest = line.trim_start();
        let Some(after_hash) = rest.strip_prefix('#') else {
            return false;
        };
        rest = after_hash.trim_start();
        // Compare by BYTES — slicing `&str[..SENTINEL.len()]` would
        // panic on a non-char-boundary (e.g. a leading `é` would
        // straddle byte 27). Ruby's regex matches literal ASCII, so
        // a byte-level ascii-case comparison is sufficient and
        // doesn't crash on Unicode header text. Caught as Ohm's P1
        // on PR #87 round 2.
        return rest
            .as_bytes()
            .get(..SENTINEL.len())
            .is_some_and(|bytes| bytes.eq_ignore_ascii_case(SENTINEL.as_bytes()));
    }
    false
}

fn build_signals(
    root: &Path,
    files: &[FileRef],
) -> (DependencyGraph, HashMap<PathBuf, usize>, usize) {
    let mut graph = DependencyGraph::new();
    let mut complexity: HashMap<PathBuf, usize> = HashMap::new();
    let mut skipped = 0usize;
    // Read + pre-flight-parse every Ruby file. Files with prism parse
    // errors are EXCLUDED from the scored population entirely — not
    // just from the complexity map. Imputation would otherwise leave a
    // parse-failed file with complexity defaulted to 0 but still
    // present in the fan_in/fan_out graph, pulling down the complexity
    // percentile of healthy neighbours. JS/TS files are skipped
    // outright: no extractor this slice.
    for file in files.iter().filter(|f| f.language == Language::Ruby) {
        let abs = root.join(&file.path);
        let source = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("stud-finder: read {}: {}", abs.display(), err);
                skipped += 1;
                continue;
            }
        };
        match analyze_ruby_source(&source) {
            Analysis::Included(refs, cx) => {
                graph.register_file(file.path.clone());
                for r in refs {
                    graph.add_reference(file.path.clone(), r.name);
                }
                complexity.insert(file.path.clone(), cx);
            }
            Analysis::Excluded => {
                eprintln!(
                    "stud-finder: {} had parse errors; excluding the file from the scored population",
                    file.path.display()
                );
                skipped += 1;
            }
        }
    }
    (graph, complexity, skipped)
}

/// Outcome of a per-file Ruby pre-flight. `Included` carries the refs
/// and the file-level complexity; `Excluded` is for files prism
/// couldn't parse cleanly — those files leave the scored population
/// entirely, matching Ohm's exclusion-not-imputation policy on PR #78.
enum Analysis {
    Included(Vec<sf_ruby::RubyRef>, usize),
    Excluded,
}

fn analyze_ruby_source(source: &str) -> Analysis {
    let report = sf_ruby::max_complexity(source);
    if !report.parsed_cleanly {
        return Analysis::Excluded;
    }
    let refs = sf_ruby::extract_refs(Path::new(""), source);
    Analysis::Included(refs, report.complexity)
}

/// Load a coverage report when `--coverage <path>` is supplied.
/// Errors are non-fatal — they print to stderr and the run proceeds
/// with an empty coverage map (coverage + interaction drop out of
/// the composite). Missing files reported by sf-coverage are logged
/// as a single informational line, matching Ruby CLI behaviour.
fn load_coverage(
    path: Option<&Path>,
    root: &Path,
    files: &[&Path],
) -> Option<HashMap<PathBuf, f64>> {
    // `None` → user didn't pass --coverage; coverage + interaction
    // stay out of the composite. `Some(_)` activates both signals
    // even if the loaded map is empty (Ruby
    // `coverage_available? = !@coverage.nil?`). On load failure we
    // also return `None` so the run degrades cleanly instead of
    // activating an all-risk-1.0 column that would mislead.
    let path = path?;
    match sf_coverage::load(path, root, files) {
        Ok(report) => {
            if !report.missing_files.is_empty() {
                eprintln!(
                    "stud-finder: {} files in the dependency graph had no coverage entry",
                    report.missing_files.len()
                );
            }
            for warning in &report.warnings {
                match warning {
                    sf_coverage::CoverageWarning::UnmatchedReportedPath(p) => eprintln!(
                        "stud-finder: coverage report path `{p}` did not match any Ruby file"
                    ),
                }
            }
            Some(report.coverage_pct)
        }
        Err(err) => {
            eprintln!(
                "stud-finder: failed to load coverage report `{}`: {err}",
                path.display()
            );
            None
        }
    }
}

/// Non-blank line count per file. Matches Ruby
/// `LocCounter#non_blank_lines`: a line counts when it has any
/// character outside Ruby `String#strip`'s whitespace set. That
/// set is BROADER than Rust's `u8::is_ascii_whitespace` — Ruby
/// includes NUL (`\0`) and vertical tab (`\x0B`), Rust's built-in
/// check does not. Using Rust's set caused Ohm's PR #83 defects
/// where a NUL-only or VT-only line counted in Rust but not in
/// Ruby.
///
/// **Invalid UTF-8:** Ruby's `strip` raises either
/// `ArgumentError: invalid byte sequence in UTF-8` (which
/// LocCounter catches and returns 0 for) or — on Ruby 3.3 — an
/// unrescued `Encoding::CompatibilityError` that escapes the
/// method entirely. We mimic the rescued case uniformly: any
/// non-UTF-8 file returns 0 for the whole file. That is a
/// **documented divergence** from the unrescued-error path; Rust
/// is strictly more robust (0 vs crash). Verified against Ruby
/// 3.3.13 in a `ruby:3.3.13-slim` container during PR #83.
///
/// **Unreadable paths:** also 0, so the row still lands in the
/// output rather than being dropped (Ruby `rescue Encoding::* =>
/// 0`).
fn count_loc(root: &Path, files: &[&Path]) -> HashMap<PathBuf, usize> {
    files
        .iter()
        .map(|rel| {
            let abs = root.join(rel);
            let count = match std::fs::read(&abs) {
                Ok(bytes) => count_non_blank_lines(&bytes),
                Err(_) => 0,
            };
            (rel.to_path_buf(), count)
        })
        .collect()
}

/// Returns `true` iff `b` is in Ruby `String#strip`'s whitespace
/// set: NUL (0x00), HT (0x09), LF (0x0A), VT (0x0B), FF (0x0C),
/// CR (0x0D), SPACE (0x20). Rust's built-in
/// `u8::is_ascii_whitespace` omits NUL and VT.
fn is_ruby_strip_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x20)
}

fn count_non_blank_lines(bytes: &[u8]) -> usize {
    // Ruby's rescue path returns 0 on invalid UTF-8. Mimicking
    // that at the whole-file level matches the captured branch of
    // LocCounter#non_blank_lines and improves on the unrescued
    // Encoding::CompatibilityError branch (Rust returns 0 instead
    // of crashing the whole run).
    if std::str::from_utf8(bytes).is_err() {
        return 0;
    }
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| line.iter().any(|b| !is_ruby_strip_whitespace(*b)))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_ruby_source_is_included_with_refs_and_complexity() {
        let src = r#"
            class Order
              def process(x)
                if x > 0
                  Payment.capture
                end
              end
            end
        "#;
        match analyze_ruby_source(src) {
            Analysis::Included(refs, cx) => {
                assert!(refs.iter().any(|r| r.name == "Payment"));
                assert!(cx >= 2, "if adds +1, so complexity >= 2; got {cx}");
            }
            Analysis::Excluded => panic!("clean source should be Included"),
        }
    }

    #[test]
    fn parse_failing_ruby_source_is_excluded() {
        // Ohm's P1-on-PR#78-review-2 regression: a parse-failed file
        // must not enter the scored population at all — no refs, no
        // complexity, no graph registration. Returning `Excluded`
        // guarantees `build_signals` skips every per-file step for
        // this source.
        let src = "class Foo\n  this is: not; valid! ruby !!\nend\n";
        assert!(
            matches!(analyze_ruby_source(src), Analysis::Excluded),
            "a file with prism parse errors must produce Excluded"
        );
    }

    #[test]
    fn exclusion_keeps_neighbour_scores_independent_of_broken_file() {
        // End-to-end: a graph built from a mix of clean + broken sources
        // (via `analyze_ruby_source` + manual `DependencyGraph` wiring,
        // the same shape `build_signals` uses) produces IDENTICAL
        // ScoredFile rows for the clean files as a graph built from the
        // clean files alone. Imputation (register broken + default
        // complexity to 0) would shift neighbour percentiles.
        use sf_core::DependencyGraph;
        use sf_score::{Scorer, Signals};

        let clean_a = "class A\n  def go(x)\n    if x then B.do end\n  end\nend\n";
        let clean_b = "class B\n  def hi\n    C.m\n  end\nend\n";
        let clean_c = "class C\n  def m; 1; end\nend\n";
        let broken = "class Broken\n  this is: not; valid! ruby !!\nend\n";

        let include = |graph: &mut DependencyGraph,
                       cx: &mut HashMap<PathBuf, usize>,
                       path: &str,
                       source: &str| {
            if let Analysis::Included(refs, c) = analyze_ruby_source(source) {
                let p = PathBuf::from(path);
                graph.register_file(p.clone());
                for r in refs {
                    graph.add_reference(p.clone(), r.name);
                }
                cx.insert(p, c);
            }
        };

        let (mut g_mixed, mut cx_mixed) = (DependencyGraph::new(), HashMap::new());
        include(&mut g_mixed, &mut cx_mixed, "a.rb", clean_a);
        include(&mut g_mixed, &mut cx_mixed, "b.rb", clean_b);
        include(&mut g_mixed, &mut cx_mixed, "c.rb", clean_c);
        // Broken file is offered but must be dropped by Analysis::Excluded.
        include(&mut g_mixed, &mut cx_mixed, "broken.rb", broken);

        let (mut g_clean, mut cx_clean) = (DependencyGraph::new(), HashMap::new());
        include(&mut g_clean, &mut cx_clean, "a.rb", clean_a);
        include(&mut g_clean, &mut cx_clean, "b.rb", clean_b);
        include(&mut g_clean, &mut cx_clean, "c.rb", clean_c);

        let scorer = Scorer::new();
        let mixed_rows = scorer.score_with_signals(Signals {
            graph: &g_mixed,
            complexity: cx_mixed,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
            loc: HashMap::new(),
        });
        let clean_rows = scorer.score_with_signals(Signals {
            graph: &g_clean,
            complexity: cx_clean,
            churn_commits: HashMap::new(),
            churn_lines: HashMap::new(),
            coupling: HashMap::new(),
            coverage: None,
            loc: HashMap::new(),
        });

        assert_eq!(
            mixed_rows.len(),
            clean_rows.len(),
            "the parse-failed file must not appear in the scored rows"
        );
        for clean_row in &clean_rows {
            let mixed_row = mixed_rows
                .iter()
                .find(|r| r.path == clean_row.path)
                .unwrap_or_else(|| {
                    panic!(
                        "missing row for {} in the mixed graph",
                        clean_row.path.display()
                    )
                });
            assert!(
                (mixed_row.composite - clean_row.composite).abs() < 1e-9,
                "exclusion must not shift the composite of clean files; {} drifted {} vs {}",
                clean_row.path.display(),
                mixed_row.composite,
                clean_row.composite
            );
            assert_eq!(mixed_row.class, clean_row.class);
        }
    }

    #[test]
    fn count_loc_matches_ruby_non_blank_lines_semantics() {
        // Verified case-by-case against Ruby 3.3.13 in a
        // `ruby:3.3.13-slim` container during PR #83 review.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let mk = |rel: &str, body: &[u8]| {
            let full = root.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, body).unwrap();
        };
        // Ordinary file: 3 non-blank lines (2nd and 4th are blank/ws).
        mk("a.rb", b"class A\n\n  def f; end\n   \t  \nend\n");
        // All blank -> 0.
        mk("blank.rb", b"\n\n  \n");
        // No trailing newline -> last line still counts.
        mk("no_newline.rb", b"a = 1");
        // NUL-only middle line must NOT count — Ruby `strip`
        // includes `\0` in its whitespace set (Rust's
        // `is_ascii_whitespace` does not). Expected Ruby count: 2.
        // Caught by Ohm's PR #83 review.
        mk("nul_line.rb", b"a\n\x00\x00\x00\nb\n");
        // Vertical-tab-only middle line must NOT count — Ruby
        // `strip` includes `\x0B`; Rust's `is_ascii_whitespace`
        // does not. Expected Ruby count: 2. Also caught by Ohm.
        mk("vtab_line.rb", b"a\n\x0B\x0B\n b\n");
        // Fixture A — invalid UTF-8 whose iteration raises
        // `ArgumentError: invalid byte sequence in UTF-8` which
        // `LocCounter#non_blank_lines` rescues → returns 0 for
        // the whole file. We match that branch. (Note: whether
        // Ruby picks `ArgumentError` vs `Encoding::CompatibilityError`
        // is NOT determined by mid-line-vs-trailing position per
        // se — it's about which Ruby operation encounters the
        // bad bytes first. Per Ohm's non-blocking naming note on
        // PR #83; the fixtures still cover both outcomes.)
        mk("rescued_invalid_utf8.rb", b"ok\n\xff\n");
        // Fixture B — invalid UTF-8 whose iteration raises an
        // UNRESCUED `Encoding::CompatibilityError` escaping
        // `non_blank_lines` entirely. Rust deliberately diverges:
        // returns 0 instead of propagating, keeping the run
        // robust. Documented divergence in the `count_loc`
        // docstring.
        mk("unrescued_invalid_utf8.rb", b"hello \xff world");
        // Unreadable path: missing file -> 0, row still present.
        let files: Vec<&Path> = [
            "a.rb",
            "blank.rb",
            "no_newline.rb",
            "nul_line.rb",
            "vtab_line.rb",
            "rescued_invalid_utf8.rb",
            "unrescued_invalid_utf8.rb",
            "ghost.rb",
        ]
        .iter()
        .map(Path::new)
        .collect();
        let loc = count_loc(root, &files);
        assert_eq!(loc.get(Path::new("a.rb")).copied(), Some(3));
        assert_eq!(loc.get(Path::new("blank.rb")).copied(), Some(0));
        assert_eq!(loc.get(Path::new("no_newline.rb")).copied(), Some(1));
        assert_eq!(
            loc.get(Path::new("nul_line.rb")).copied(),
            Some(2),
            "NUL-only lines must not count — Ruby `strip` set includes `\\0`"
        );
        assert_eq!(
            loc.get(Path::new("vtab_line.rb")).copied(),
            Some(2),
            "VT-only lines must not count — Ruby `strip` set includes `\\v`"
        );
        assert_eq!(
            loc.get(Path::new("rescued_invalid_utf8.rb")).copied(),
            Some(0),
            "fixture that trips Ruby's rescued ArgumentError must also return 0 in Rust"
        );
        assert_eq!(
            loc.get(Path::new("unrescued_invalid_utf8.rb")).copied(),
            Some(0),
            "fixture that trips Ruby's unrescued Encoding::CompatibilityError — Rust diverges to 0 (robust) instead of crashing"
        );
        assert_eq!(
            loc.get(Path::new("ghost.rb")).copied(),
            Some(0),
            "unreadable path must land as 0, not be dropped — Ruby `rescue Encoding::* => 0`"
        );
    }

    #[test]
    fn is_ruby_strip_whitespace_matches_the_full_ruby_set() {
        // Ruby's `String#strip` whitespace set, verified against
        // Ruby 3.3.13: NUL, HT, LF, VT, FF, CR, SPACE. Nothing else.
        for b in [0x00u8, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20] {
            assert!(is_ruby_strip_whitespace(b), "0x{b:02X} should be ws");
        }
        // A sampling of non-whitespace bytes that must NOT count.
        for b in [0x01u8, 0x08, 0x0E, 0x1F, 0x21, 0x41, 0x7E, 0x7F, 0xFF] {
            assert!(
                !is_ruby_strip_whitespace(b),
                "0x{b:02X} must not be treated as ws"
            );
        }
    }
}

// Slice #10b: the inline `print_table` / `print_js_table` /
// `width_of` helpers were removed. Table rendering now lives in
// `output::emit` next to the other emitters, using the Ruby
// `emit_table` layout (section headers, column widths, footer).
