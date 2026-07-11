# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - Unreleased

### Added

- Absolute floors in classification: raw complexity >= 15 or raw fan_in >= 25 can no longer classify as leaf; these files escalate to branch only, never downgrade.
- Temporal coupling now contributes to composite score at a nominal `0.05` weight when coupling data is available.
- Added `--coupling-weight N` and optional `coupling:P` in `--weights`, defaulting to `0.05`.
- Insufficient-dispersion warnings in `analysis.warnings` when a signal percentile-rank collapses to all 0.0 despite non-zero raw values; score is unchanged.
- Added insufficient-dispersion warnings for temporal coupling.

### Changed

- BREAKING: `class` (`trunk`/`branch`/`leaf`) is now driven by composite `score`, not fan-in percentile only. Class reflects overall risk, not just coupling role. `--trunk-threshold` and `--branch-threshold` still take percentile values from 0-100, but now threshold composite `score` instead of `fan_in_pct`; defaults remain 85/50.
- BREAKING: Composite scoring now uses a uniform rebalance: `fan_in` changes from `0.25` to `0.20`, `coverage` changes from `0.15` to `0.10`, and a new `0.10 × fan_in_pct × coverage_risk_pct` interaction term is added with no global divisor; four-factor scores also shift intentionally because the reduced fan-in weight flows through no-coverage renormalization.
- BREAKING: Churn composite (`churn_pct`) is now percentile-ranked after averaging commit-count and lines-changed percentiles. The previous triangular distribution gave churn about half the variance/effective weight; re-ranking restores the intended contribution. Scores shift, especially for high-churn files.
- BREAKING: Numeric default weights are now `fan_in: 0.19`, `fan_out: 0.095`, `complexity: 0.2375`, `churn: 0.2375`, `coverage: 0.095`, `interaction: 0.095`, and `coupling: 0.05`; base-four ratios are preserved, so four-factor scoring without coupling data keeps the same effective weights as before.
- BREAKING: Row output now includes `evidence` immediately after `score`, a 0.0-1.0 metadata confidence value based on age, commit count, and explicit coverage-data presence. Output sorting now keys on `(class_rank, score)`, so trunks rank above branches above leaves and `--top N` no longer drops newness-escalated `trunk_adjacent` files behind high-score branches. Gate consumers should threshold `class` for verdicts and `evidence` for confidence, not raw `score`.

## [0.1.0] - 2026-07-03

### Added

- Initial RubyGems release of `stud-finder`, a CLI that ranks files by five risk signals: fan-in (blast radius), fan-out, cyclomatic complexity, git churn, and test coverage.
- Temporal coupling analysis for identifying files that change together.
- Diff mode for scoring only files changed in a pull request while preserving repo-relative scores.
- Ruby and JavaScript/TypeScript support.
- Table, JSON, CSV, and Markdown output formats.
- Lexical constant resolution for Ruby fan-in analysis, including the Task 1 fix from PR #33.
