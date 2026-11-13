# stud-finder — Rust port (work in progress)

This directory holds the Rust port of stud-finder, living on the `rust`
branch. The Ruby implementation at the repo root is unchanged and
remains the authoritative CLI while this port is built out.

## Goal

Data-parity with the Ruby CLI's score / class / evidence model, at
daemon-fast speed on both whole-repo and PR-diff scans. The CLI surface
and output shapes (table / json / markdown / csv) stay the same so
existing consumers of stud-finder's JSON do not have to change.

## Status

Scaffold only. `sf-cli` walks a path and prints how many Ruby and
JavaScript files it saw. No signals, no scores yet.

## Layout

```
rust/
  Cargo.toml          workspace manifest
  ATTRIBUTION.md      provenance for code adapted from saugatkhadka/ripple
  crates/
    sf-core/          shared data model, dependency-graph primitives
    sf-cli/           the `stud-finder` binary
```

More crates (`sf-ruby`, `sf-js`, `sf-history`, `sf-coverage`, `sf-score`)
land as each signal lights up end to end.

## Build

```
cd rust
cargo build
cargo run --bin stud-finder -- ..
```
