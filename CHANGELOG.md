# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - Unreleased

### Changed

- BREAKING: `class` (`trunk`/`branch`/`leaf`) is now driven by composite `score`, not fan-in percentile only. Class reflects overall risk, not just coupling role. `--trunk-threshold` and `--branch-threshold` still take percentile values from 0-100, but now threshold composite `score` instead of `fan_in_pct`; defaults remain 85/50.

## [0.1.0] - 2026-07-03

### Added

- Initial RubyGems release of `stud-finder`, a CLI that ranks files by five risk signals: fan-in (blast radius), fan-out, cyclomatic complexity, git churn, and test coverage.
- Temporal coupling analysis for identifying files that change together.
- Diff mode for scoring only files changed in a pull request while preserving repo-relative scores.
- Ruby and JavaScript/TypeScript support.
- Table, JSON, CSV, and Markdown output formats.
- Lexical constant resolution for Ruby fan-in analysis, including the Task 1 fix from PR #33.
