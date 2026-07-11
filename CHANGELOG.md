# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-07-11

### Fixed

- `--interaction-weight N` now works correctly: setting only the interaction weight no longer triggers `weights must sum to 1.0` or `coverage weight must be 0.0` errors; it behaves identically to `--coupling-weight N` (bounds-checked inline, then renormalized by the scorer).
- Absolute-floor escalations now carry an `escalation` marker in the output row: `complexity_floor` when raw complexity ≥ 15 triggered the leaf→branch escalation, `fan_in_floor` when raw fan_in ≥ 25 triggered it. Consumers can now distinguish a floor-escalated branch from a threshold-classified branch without parsing the score. Newness escalations (`recency_floor`, `trunk_adjacent`) continue to take precedence.
- Formula label is now consistent across all output surfaces. The canonical labels are `5-factor + coupling`, `5-factor`, `4-factor + coupling`, and `4-factor`; the previous mismatch (stderr reported `6-factor formula` while JSON meta reported `5-factor + coupling`) is resolved.

### Added

- Absolute floors in classification: raw complexity >= 15 or raw fan_in >= 25 can no longer classify as leaf; these files escalate to branch only, never downgrade.
- Temporal coupling now contributes to composite score at a nominal `0.05` weight when coupling data is available.
- Added `--coupling-weight N` and optional `coupling:P` in `--weights`, defaulting to `0.05`.
- Insufficient-dispersion warnings in `analysis.warnings` when a signal percentile-rank collapses to all 0.0 despite non-zero raw values; score is unchanged.
- Added insufficient-dispersion warnings for temporal coupling.

### Changed

- BREAKING: `class` (`trunk`/`branch`/`leaf`) is now driven by composite `score`, not fan-in percentile only. Class reflects overall risk, not just coupling role. `--trunk-threshold` and `--branch-threshold` still take integer values 1-99, but now threshold composite `score` instead of `fan_in_pct`; defaults remain 85/50.
- BREAKING: Composite scoring now uses a uniform rebalance: `fan_in` changes from `0.25` to `0.20`, `coverage` changes from `0.15` to `0.10`, and a new `0.10 × fan_in_pct × coverage_risk_pct` interaction term is added with no global divisor; four-factor scores also shift intentionally because the reduced fan-in weight flows through no-coverage renormalization.
- BREAKING: Churn composite (`churn_pct`) is now percentile-ranked after averaging commit-count and lines-changed percentiles. The previous triangular distribution gave churn about half the variance/effective weight; re-ranking restores the intended contribution. Scores shift, especially for high-churn files.
- BREAKING: Numeric default weights are now `fan_in: 0.19`, `fan_out: 0.095`, `complexity: 0.2375`, `churn: 0.2375`, `coverage: 0.095`, `interaction: 0.095`, and `coupling: 0.05`; base-four ratios are preserved, so four-factor scoring without coupling data keeps the same effective weights as before.
- BREAKING: Row output now includes `evidence` immediately after `score`, a 0.0-1.0 metadata confidence value based on age, commit count, and explicit coverage-data presence. Output sorting now keys on `(class_rank, score)`, so trunks rank above branches above leaves and `--top N` no longer drops newness-escalated `trunk_adjacent` files behind high-score branches. Gate consumers should threshold `class` for verdicts and `evidence` for confidence, not raw `score`.

### Docs

- Docs surface reduced to `README.md`, `SIGNALS.md`, and `CHANGELOG.md`. `PRODUCT.md` is renamed to `SIGNALS.md` and stripped of weights/roadmap so it only holds signal theory. `VISION.md` and `TRD.md` are removed — their content was drifting out of sync with the code faster than it was being read. `stud-finder --help` and the JSON output are the authoritative CLI/schema references.

## [0.1.0] - 2026-07-03

### Added

- Initial `stud-finder` cut: a CLI that ranks files by five risk signals: fan-in (blast radius), fan-out, cyclomatic complexity, git churn, and test coverage.
- Temporal coupling analysis for identifying files that change together.
- Diff mode for scoring only files changed in a pull request while preserving repo-relative scores.
- Ruby and JavaScript/TypeScript support.
- Table, JSON, CSV, and Markdown output formats.
- Lexical constant resolution for Ruby fan-in analysis, including the Task 1 fix from PR #33.
